// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Post-result hook execution.
//!
//! Runs after a batch of tool calls completes. For each (call, result) pair
//! and each matching `[[hook]]` rule, spawns the configured script with the
//! tool context piped on stdin. Non-zero exit → script stdout is injected
//! into the session inbox as a user message; zero exit → no-op.
//!
//! Skips guardrail-blocked tools — their synthetic `[guardrail]` result is
//! not a real result and should not trigger validators.

use crate::config::guardrails::{target_matches, CompiledHook, HookOn};
use crate::mcp::{McpToolCall, McpToolResult};
use crate::session::context::SessionId;
use crate::session::inbox::{InboxMessage, InboxSource};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const HOOK_TIMEOUT_SECS: u64 = 300;

/// Evaluate hooks for a batch of (call, result) pairs and inject any
/// non-zero-exit script stdouts into the session inbox.
///
/// `blocked` is parallel to `calls`/`results`; `true` entries are skipped
/// (those results were synthesized by guardrails, not produced by a tool).
pub async fn run_hooks(
	session_id: &SessionId,
	config: &crate::config::Config,
	calls: &[McpToolCall],
	results: &[McpToolResult],
	blocked: &[bool],
) {
	let Some(rules) = crate::session::guardrails::get_rules(session_id) else {
		return;
	};
	if rules.hooks.is_empty() {
		return;
	}

	let workdir = crate::session::context::get_current_workdir(session_id)
		.or_else(|| std::env::current_dir().ok())
		.unwrap_or_default();

	let mut tasks: Vec<tokio::task::JoinHandle<Option<InboxMessage>>> = Vec::new();

	for (i, call) in calls.iter().enumerate() {
		if blocked.get(i).copied().unwrap_or(false) {
			continue;
		}
		let Some(result) = results.get(i) else {
			continue;
		};

		let server = crate::mcp::tool_map::get_server_for_tool(&call.tool_name)
			.map(|s| s.name().to_string());
		let capability = crate::session::guardrails::resolve_capability(
			session_id,
			config,
			server.as_deref(),
			&call.tool_name,
		);
		let result_text = result.extract_content();
		let success = !result.is_error();

		for hook in &rules.hooks {
			if !hook_matches(
				hook,
				capability.as_deref(),
				&call.parameters,
				&result_text,
				success,
			) {
				continue;
			}
			let script = hook.script.clone();
			let workdir = workdir.clone();
			let call_clone = call.clone();
			let result_text_clone = result_text.clone();
			let capability_clone = capability.clone();

			tasks.push(tokio::spawn(async move {
				run_one_hook(
					script,
					workdir,
					call_clone,
					capability_clone,
					result_text_clone,
					success,
				)
				.await
			}));
		}
	}

	let outputs = futures::future::join_all(tasks).await;
	for out in outputs.into_iter().flatten().flatten() {
		crate::session::inbox::push_inbox_message_for_session(session_id, out);
	}
}

fn hook_matches(
	hook: &CompiledHook,
	capability: Option<&str>,
	params: &serde_json::Value,
	result_text: &str,
	success: bool,
) -> bool {
	match hook.on {
		HookOn::Success if !success => return false,
		HookOn::Error if success => return false,
		_ => {}
	}
	if let Some(trigger) = &hook.trigger {
		if !target_matches(trigger, capability, params) {
			return false;
		}
	}
	if let Some(re) = &hook.result_regex {
		if !re.is_match(result_text) {
			return false;
		}
	}
	true
}

/// Spawn one hook script, wait up to the timeout, return an inbox message
/// when the script exits non-zero with non-empty stdout. `None` otherwise.
async fn run_one_hook(
	script: PathBuf,
	workdir: std::path::PathBuf,
	call: McpToolCall,
	capability: Option<String>,
	result_text: String,
	success: bool,
) -> Option<InboxMessage> {
	let script_path = if script.is_absolute() {
		script.clone()
	} else {
		workdir.join(&script)
	};
	let script_display = script.display().to_string();

	let payload = json!({
		"capability": capability,
		"tool": call.tool_name,
		"tool_id": call.tool_id,
		"params": call.parameters,
		"result": result_text,
		"success": success,
	});
	let payload_bytes = match serde_json::to_vec(&payload) {
		Ok(b) => b,
		Err(e) => {
			crate::log_debug!("guardrail hook: serialize payload failed: {}", e);
			return None;
		}
	};

	let mut cmd = Command::new(&script_path);
	cmd.current_dir(&workdir);
	cmd.env("OCTOMIND_CAPABILITY", capability.as_deref().unwrap_or(""));
	cmd.env("OCTOMIND_TOOL", &call.tool_name);
	cmd.env("OCTOMIND_SUCCESS", if success { "1" } else { "0" });
	cmd.env("OCTOMIND_WORKDIR", workdir.display().to_string());
	cmd.stdin(std::process::Stdio::piped());
	cmd.stdout(std::process::Stdio::piped());
	cmd.stderr(std::process::Stdio::piped());

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			eprintln!(
				"guardrail hook: spawn `{}` failed: {}",
				script_path.display(),
				e
			);
			return None;
		}
	};
	if let Some(mut stdin) = child.stdin.take() {
		let _ = stdin.write_all(&payload_bytes).await;
		let _ = stdin.shutdown().await;
	}

	let output = match tokio::time::timeout(
		Duration::from_secs(HOOK_TIMEOUT_SECS),
		child.wait_with_output(),
	)
	.await
	{
		Ok(Ok(o)) => o,
		Ok(Err(e)) => {
			eprintln!("guardrail hook `{}` wait failed: {}", script_display, e);
			return None;
		}
		Err(_) => {
			eprintln!(
				"guardrail hook `{}` timed out after {}s",
				script_display, HOOK_TIMEOUT_SECS
			);
			return None;
		}
	};

	if output.status.success() {
		return None;
	}
	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	if !output.stderr.is_empty() {
		crate::log_debug!(
			"guardrail hook `{}` stderr: {}",
			script_display,
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	if stdout.is_empty() {
		return None;
	}
	Some(InboxMessage {
		source: InboxSource::GuardrailHook {
			script: script_display,
		},
		content: stdout,
	})
}
