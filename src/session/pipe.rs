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

//! Pre-model pipe execution.
//!
//! Runs a matching `[[pipe]]` from `.agents/guardrails.toml` on user input
//! before the model sees it. Non-zero exit is a hard stop — the message is
//! not sent to the model.

use crate::config::guardrails::{role_matches, CompiledPipe, PipeWhen};
use crate::session::context::SessionId;
use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const PIPE_TIMEOUT_SECS: u64 = 300;

/// Run a matching `[[pipe]]` on user input before the model sees it.
///
/// Filter evaluation order (cheapest first):
///   1. `roles` — string match
///   2. `when` — session state check (first message vs any)
///   3. `match` — regex on message text
///
/// At most one pipe may match; multiple matches are an error.
/// Returns `Ok(Some(transformed_input))` when a pipe ran, `Ok(None)` when
/// no pipe matched, or `Err` on hard-stop (non-zero exit, timeout, spawn
/// failure, multiple matches).
pub async fn run_pipe(
	session_id: &SessionId,
	role: &str,
	input: &str,
	first_message_processed: bool,
) -> Result<Option<String>> {
	// Increment message counter for SESSION_MESSAGE_COUNT env var.
	// Always increment — the counter tracks total user messages regardless
	// of whether any pipe matches.
	let message_count = crate::session::guardrails::increment_message_count(session_id);

	let Some(rules) = crate::session::guardrails::get_rules(session_id) else {
		return Ok(None);
	};
	if rules.pipes.is_empty() {
		return Ok(None);
	}

	// Find matching pipes (cheapest filter first).
	let mut matched: Vec<&CompiledPipe> = Vec::new();
	for pipe in &rules.pipes {
		// 1. Role filter
		if !role_matches(&pipe.roles, role) {
			continue;
		}
		// 2. When filter
		match pipe.when {
			PipeWhen::First if first_message_processed => continue,
			_ => {}
		}
		// 3. Match regex on user message text
		if let Some(re) = &pipe.match_regex {
			if !re.is_match(input) {
				continue;
			}
		}
		matched.push(pipe);
	}

	if matched.is_empty() {
		return Ok(None);
	}
	if matched.len() > 1 {
		let names: Vec<&str> = matched.iter().map(|p| p.name.as_str()).collect();
		return Err(anyhow!(
			"Multiple [[pipe]] entries matched: {} — only one may match per message",
			names.join(", ")
		));
	}

	let pipe = matched[0];
	let run_count = crate::session::guardrails::increment_pipe_run_count(session_id, &pipe.name);

	let workdir = crate::session::context::get_current_workdir(session_id)
		.or_else(|| std::env::current_dir().ok())
		.unwrap_or_default();

	let script_path = if pipe.command.is_absolute() {
		pipe.command.clone()
	} else {
		workdir.join(&pipe.command)
	};

	let mut cmd = Command::new(&script_path);
	cmd.current_dir(&workdir);
	cmd.env("OCTOMIND_ROLE", role);
	cmd.env("OCTOMIND_WORKDIR", workdir.display().to_string());
	cmd.env("PIPE_NAME", &pipe.name);
	cmd.env("PIPE_RUN_COUNT", run_count.to_string());
	cmd.env("SESSION_MESSAGE_COUNT", message_count.to_string());
	cmd.stdin(std::process::Stdio::piped());
	cmd.stdout(std::process::Stdio::piped());
	cmd.stderr(std::process::Stdio::piped());

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			return Err(anyhow!(
				"Pipe '{}' failed to spawn '{}': {}",
				pipe.name,
				script_path.display(),
				e
			));
		}
	};

	if let Some(mut stdin) = child.stdin.take() {
		let _ = stdin.write_all(input.as_bytes()).await;
		let _ = stdin.shutdown().await;
	}

	let output = match tokio::time::timeout(
		Duration::from_secs(PIPE_TIMEOUT_SECS),
		child.wait_with_output(),
	)
	.await
	{
		Ok(Ok(o)) => o,
		Ok(Err(e)) => {
			return Err(anyhow!("Pipe '{}' wait failed: {}", pipe.name, e));
		}
		Err(_) => {
			return Err(anyhow!(
				"Pipe '{}' timed out after {}s",
				pipe.name,
				PIPE_TIMEOUT_SECS
			));
		}
	};

	if !output.stderr.is_empty() {
		crate::log_debug!(
			"Pipe '{}' stderr: {}",
			pipe.name,
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
		let detail = if !stderr.is_empty() {
			stderr
		} else if !stdout.is_empty() {
			stdout
		} else {
			format!("exit code {}", output.status.code().unwrap_or(-1))
		};
		return Err(anyhow!("Pipe '{}' rejected input: {}", pipe.name, detail));
	}

	let stdout = String::from_utf8_lossy(&output.stdout).to_string();
	Ok(Some(stdout))
}
