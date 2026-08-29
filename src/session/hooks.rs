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

use crate::config::guardrails::{
	role_matches, target_matches, CompiledHook, CompiledValidator, HookOn,
};
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
			if let Some(binding) = &hook.evolution {
				if crate::supervisor::learning::evolution::binding_is_shadow(
					&binding.id,
					binding.shadow,
				) {
					crate::supervisor::learning::evolution::mark_shadow_match(&binding.id);
					continue;
				}
				crate::supervisor::learning::evolution::mark_behavior_used(session_id, &binding.id);
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
	// Reap the child if we drop it on timeout; tokio does not kill on drop.
	cmd.kill_on_drop(true);

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
	// Write the payload from a detached task so wait_with_output() below can
	// drain the child's stdout/stderr concurrently. A blocking write here
	// deadlocks once the payload (full untruncated tool result) exceeds the
	// ~64KB pipe buffer and the script doesn't drain stdin — and because the
	// write sits OUTSIDE the timeout, the turn would hang forever.
	if let Some(mut stdin) = child.stdin.take() {
		tokio::spawn(async move {
			let _ = stdin.write_all(&payload_bytes).await;
			let _ = stdin.shutdown().await;
		});
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

// ---------------------------------------------------------------------------
// Post-turn validators
// ---------------------------------------------------------------------------

/// Evaluate `[[validator]]` rules at end of assistant turn and inject any
/// non-zero-exit script stdouts (wrapped in `<validation validator="…">…
/// </validation>`) into the session inbox. Performant short-circuit order:
///
///   1. role filter (cheap string match) — skip cold validators first
///   2. `when` filter against `call_log[cursor..]` — skip when no history match
///   3. `match` regex against assistant message text — skip when content miss
///
/// Only validators that pass all filters spawn their script and advance
/// their cursor to `call_log.len()`.
pub async fn run_turn_validators(session_id: &SessionId, role: &str, assistant_text: &str) {
	let Some(rules) = crate::session::guardrails::get_rules(session_id) else {
		return;
	};
	if rules.validators.is_empty() {
		return;
	}

	// Snapshot call log once — read-locked briefly, cloned for evaluation
	// outside any lock so spawned scripts can't deadlock the session.
	let call_log = crate::session::guardrails::get_call_log(session_id);
	let call_log_len = call_log.len();
	let workdir = crate::session::context::get_current_workdir(session_id)
		.or_else(|| std::env::current_dir().ok())
		.unwrap_or_default();

	let mut tasks: Vec<tokio::task::JoinHandle<Option<InboxMessage>>> = Vec::new();
	let mut advanced: Vec<String> = Vec::new();

	for v in &rules.validators {
		// 1. Role filter (cheapest)
		if !role_matches(&v.roles, role) {
			continue;
		}

		// 2. `when` over call_log slice since this validator's last run
		let cursor = crate::session::guardrails::validator_cursor(session_id, &v.name);
		let slice_start = cursor.min(call_log_len);
		if !when_satisfied(v, &call_log[slice_start..]) {
			continue;
		}

		// 3. `match` regex on assistant message text
		if let Some(re) = &v.match_regex {
			if !re.is_match(assistant_text) {
				continue;
			}
			if let Some(binding) = &v.evolution {
				if crate::supervisor::learning::evolution::binding_is_shadow(
					&binding.id,
					binding.shadow,
				) {
					crate::supervisor::learning::evolution::mark_shadow_match(&binding.id);
					continue;
				}
				crate::supervisor::learning::evolution::mark_behavior_used(session_id, &binding.id);
			}
		}

		// Build triggered_by list once per validator: calls in the slice that
		// satisfied any `+used` target. When `when_used` is empty the validator
		// fires on absence/always, so triggered_by includes everything in the
		// slice (useful context: "what did happen since last run").
		let triggered_by: Vec<serde_json::Value> = call_log[slice_start..]
			.iter()
			.filter(|(cap, params)| {
				v.when_used.is_empty()
					|| v.when_used
						.iter()
						.any(|t| target_matches(t, cap.as_deref(), params))
			})
			.map(|(cap, params)| json!({ "capability": cap, "params": params }))
			.collect();

		let name = v.name.clone();
		let script = v.script.clone();
		let workdir = workdir.clone();
		let role = role.to_string();
		let assistant_text = assistant_text.to_string();

		tasks.push(tokio::spawn(async move {
			run_one_validator(name, script, workdir, role, assistant_text, triggered_by).await
		}));
		advanced.push(v.name.clone());
	}

	// Advance cursors immediately (don't wait for script to finish — the
	// validator "ran" once it spawned; exit code only affects whether we
	// inject, not whether the cursor moves).
	for name in &advanced {
		crate::session::guardrails::set_validator_cursor(session_id, name, call_log_len);
	}

	let outputs = futures::future::join_all(tasks).await;
	for out in outputs.into_iter().flatten().flatten() {
		crate::session::inbox::push_inbox_message_for_session(session_id, out);
	}
}

/// Evaluate `+used / -unused` against a slice of the call log.
fn when_satisfied(v: &CompiledValidator, slice: &[(Option<String>, serde_json::Value)]) -> bool {
	let used_ok = v.when_used.iter().all(|t| {
		slice
			.iter()
			.any(|(cap, params)| target_matches(t, cap.as_deref(), params))
	});
	if !used_ok {
		return false;
	}
	let unused_ok = v.when_unused.iter().all(|t| {
		!slice
			.iter()
			.any(|(cap, params)| target_matches(t, cap.as_deref(), params))
	});
	unused_ok
}

async fn run_one_validator(
	name: String,
	script: PathBuf,
	workdir: std::path::PathBuf,
	role: String,
	assistant_text: String,
	triggered_by: Vec<serde_json::Value>,
) -> Option<InboxMessage> {
	let script_path = if script.is_absolute() {
		script.clone()
	} else {
		workdir.join(&script)
	};

	let payload = json!({
		"validator": name,
		"role": role,
		"assistant_text": assistant_text,
		"triggered_by": triggered_by,
	});
	let payload_bytes = match serde_json::to_vec(&payload) {
		Ok(b) => b,
		Err(e) => {
			crate::log_debug!("validator `{}`: serialize payload failed: {}", name, e);
			return None;
		}
	};

	let mut cmd = Command::new(&script_path);
	cmd.current_dir(&workdir);
	cmd.env("OCTOMIND_VALIDATOR", &name);
	cmd.env("OCTOMIND_ROLE", &role);
	cmd.env("OCTOMIND_WORKDIR", workdir.display().to_string());
	cmd.stdin(std::process::Stdio::piped());
	cmd.stdout(std::process::Stdio::piped());
	cmd.stderr(std::process::Stdio::piped());
	// Reap the child if we drop it on timeout; tokio does not kill on drop.
	cmd.kill_on_drop(true);

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			eprintln!(
				"validator `{}`: spawn `{}` failed: {}",
				name,
				script_path.display(),
				e
			);
			return None;
		}
	};
	// Detached write so wait_with_output() drains stdout/stderr concurrently —
	// see run_one_hook for why a blocking write deadlocks on large payloads.
	if let Some(mut stdin) = child.stdin.take() {
		tokio::spawn(async move {
			let _ = stdin.write_all(&payload_bytes).await;
			let _ = stdin.shutdown().await;
		});
	}

	let output = match tokio::time::timeout(
		Duration::from_secs(HOOK_TIMEOUT_SECS),
		child.wait_with_output(),
	)
	.await
	{
		Ok(Ok(o)) => o,
		Ok(Err(e)) => {
			eprintln!("validator `{}` wait failed: {}", name, e);
			return None;
		}
		Err(_) => {
			eprintln!(
				"validator `{}` timed out after {}s",
				name, HOOK_TIMEOUT_SECS
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
			"validator `{}` stderr: {}",
			name,
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	if stdout.is_empty() {
		return None;
	}
	let wrapped = format!(
		"<validation validator=\"{}\">\n{}\n</validation>",
		name, stdout
	);
	Some(InboxMessage {
		source: InboxSource::GuardValidator { name },
		content: wrapped,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::guardrails::Target;
	use regex::Regex;

	fn target(capability: &str, arg_name: Option<&str>, regex: Option<&str>) -> Target {
		Target {
			capability: capability.to_string(),
			arg_name: arg_name.map(|s| s.to_string()),
			regex: regex.map(|r| Regex::new(r).expect("valid regex")),
		}
	}

	fn hook(on: HookOn, trigger: Option<Target>, result: Option<&str>) -> CompiledHook {
		CompiledHook {
			trigger,
			result_regex: result.map(|r| Regex::new(r).expect("valid regex")),
			on,
			script: PathBuf::from("hook.sh"),
			evolution: None,
		}
	}

	#[test]
	fn on_filter_gates_by_tool_outcome() {
		let params = json!({});
		let any = hook(HookOn::Any, None, None);
		assert!(hook_matches(&any, Some("shell"), &params, "out", true));
		assert!(hook_matches(&any, Some("shell"), &params, "out", false));

		let success = hook(HookOn::Success, None, None);
		assert!(hook_matches(&success, Some("shell"), &params, "out", true));
		assert!(!hook_matches(
			&success,
			Some("shell"),
			&params,
			"out",
			false
		));

		let error = hook(HookOn::Error, None, None);
		assert!(!hook_matches(&error, Some("shell"), &params, "out", true));
		assert!(hook_matches(&error, Some("shell"), &params, "out", false));
	}

	#[test]
	fn trigger_narrows_to_one_capability() {
		let h = hook(HookOn::Any, Some(target("shell", None, None)), None);
		let params = json!({"command": "ls"});
		assert!(hook_matches(&h, Some("shell"), &params, "out", true));
		assert!(!hook_matches(&h, Some("read"), &params, "out", true));
		// An uncapability-mapped tool never matches a targeted hook.
		assert!(!hook_matches(&h, None, &params, "out", true));
	}

	#[test]
	fn trigger_can_match_on_an_argument() {
		let h = hook(
			HookOn::Any,
			Some(target("shell", Some("command"), Some("^git push"))),
			None,
		);
		assert!(hook_matches(
			&h,
			Some("shell"),
			&json!({"command": "git push origin"}),
			"",
			true
		));
		assert!(!hook_matches(
			&h,
			Some("shell"),
			&json!({"command": "git status"}),
			"",
			true
		));
	}

	#[test]
	fn result_regex_filters_on_output_text() {
		let h = hook(HookOn::Any, None, Some("(?i)panic"));
		let params = json!({});
		assert!(hook_matches(
			&h,
			Some("shell"),
			&params,
			"thread 'main' PANICKED",
			true
		));
		assert!(!hook_matches(&h, Some("shell"), &params, "all good", true));
	}

	#[test]
	fn all_configured_filters_must_agree() {
		let h = hook(
			HookOn::Error,
			Some(target("shell", None, None)),
			Some("denied"),
		);
		let params = json!({});
		// Every filter satisfied.
		assert!(hook_matches(
			&h,
			Some("shell"),
			&params,
			"permission denied",
			false
		));
		// Right tool + text, wrong outcome.
		assert!(!hook_matches(
			&h,
			Some("shell"),
			&params,
			"permission denied",
			true
		));
		// Right outcome + text, wrong tool.
		assert!(!hook_matches(
			&h,
			Some("read"),
			&params,
			"permission denied",
			false
		));
		// Right tool + outcome, wrong text.
		assert!(!hook_matches(&h, Some("shell"), &params, "ok", false));
	}

	fn validator(when_used: Vec<Target>, when_unused: Vec<Target>) -> CompiledValidator {
		CompiledValidator {
			name: "v".to_string(),
			match_regex: None,
			when_used,
			when_unused,
			roles: Vec::new(),
			script: PathBuf::from("v.sh"),
			evolution: None,
		}
	}

	#[test]
	fn when_satisfied_is_vacuously_true_with_no_conditions() {
		assert!(when_satisfied(&validator(vec![], vec![]), &[]));
	}

	#[test]
	fn when_used_requires_every_target_in_the_slice() {
		let v = validator(
			vec![target("edit", None, None), target("shell", None, None)],
			vec![],
		);
		let only_edit = [(Some("edit".to_string()), json!({}))];
		assert!(!when_satisfied(&v, &only_edit));

		let both = [
			(Some("edit".to_string()), json!({})),
			(Some("shell".to_string()), json!({})),
		];
		assert!(when_satisfied(&v, &both));
	}

	#[test]
	fn when_unused_requires_the_target_to_be_absent() {
		let v = validator(vec![], vec![target("shell", None, None)]);
		assert!(when_satisfied(&v, &[(Some("edit".to_string()), json!({}))]));
		assert!(!when_satisfied(
			&v,
			&[(Some("shell".to_string()), json!({}))]
		));
	}

	#[test]
	fn used_and_unused_are_combined() {
		// "edited a file but never ran the tests"
		let v = validator(
			vec![target("edit", None, None)],
			vec![target("shell", Some("command"), Some("cargo test"))],
		);
		let edited_only = [(Some("edit".to_string()), json!({"path": "a.rs"}))];
		assert!(when_satisfied(&v, &edited_only));

		let edited_and_tested = [
			(Some("edit".to_string()), json!({"path": "a.rs"})),
			(
				Some("shell".to_string()),
				json!({"command": "cargo test --lib"}),
			),
		];
		assert!(!when_satisfied(&v, &edited_and_tested));
	}
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod run_tests;
