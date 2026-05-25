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

//! Subprocess runner for one step: spawn `octomind run --format jsonl`,
//! stream `ServerMessage` events, accumulate assistant text + costs.

use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::websocket::ServerMessage;

/// Result of one `octomind run` invocation.
#[derive(Debug, Clone, Default)]
pub struct StepStats {
	pub output: String,
	pub duration: Duration,
	pub cost: f64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub total_tokens: u64,
}

/// Outcome categories surfaced to the executor (retry/timeout/etc).
#[derive(Debug)]
pub enum RunOutcome {
	Ok(StepStats),
	Empty(StepStats),
	NonZero { stats: StepStats, code: Option<i32> },
	Timeout(Duration),
	SpawnError(anyhow::Error),
}

/// Invoke `octomind run` with `prompt` on stdin, optional `--name` to
/// resume or create a named session, and `--format jsonl`.
///
/// `timeout_secs == 0` disables the timeout.
pub async fn run_step(
	role: &str,
	prompt: &str,
	session_name: Option<&str>,
	timeout_secs: u64,
) -> RunOutcome {
	let started = Instant::now();
	let exe = match std::env::current_exe() {
		Ok(p) => p,
		Err(e) => return RunOutcome::SpawnError(e.into()),
	};

	let mut cmd = Command::new(&exe);
	cmd.arg("run")
		.arg(role)
		.arg("--format")
		.arg("jsonl")
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null());
	if let Some(name) = session_name {
		cmd.arg("--name").arg(name);
	}
	cmd.kill_on_drop(true);

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => return RunOutcome::SpawnError(anyhow!("spawn failed: {e}")),
	};

	// Write the prompt to stdin and close it.
	if let Some(mut stdin) = child.stdin.take() {
		let payload = prompt.to_string();
		tokio::spawn(async move {
			let _ = stdin.write_all(payload.as_bytes()).await;
			let _ = stdin.shutdown().await;
		});
	}

	let stdout = child.stdout.take().expect("stdout piped");
	let reader = BufReader::new(stdout);

	let collect = async {
		let mut stats = StepStats::default();
		let mut lines = reader.lines();
		while let Ok(Some(line)) = lines.next_line().await {
			let trimmed = line.trim();
			if trimmed.is_empty() {
				continue;
			}
			if let Ok(msg) = serde_json::from_str::<ServerMessage>(trimmed) {
				match msg {
					ServerMessage::Assistant(p) => {
						if !stats.output.is_empty() {
							stats.output.push('\n');
						}
						stats.output.push_str(&p.content);
					}
					ServerMessage::Cost(c) => {
						stats.cost = c.session_cost;
						stats.input_tokens = c.input_tokens;
						stats.output_tokens = c.output_tokens;
						stats.total_tokens = c.session_tokens;
					}
					_ => {}
				}
			}
		}

		let status = child.wait().await.context("wait failed")?;
		stats.duration = started.elapsed();
		Ok::<_, anyhow::Error>((status, stats))
	};

	let result = if timeout_secs == 0 {
		collect.await
	} else {
		match tokio::time::timeout(Duration::from_secs(timeout_secs), collect).await {
			Ok(r) => r,
			Err(_) => {
				// Best-effort kill; ignore errors (process may have just exited).
				return RunOutcome::Timeout(started.elapsed());
			}
		}
	};

	match result {
		Ok((status, stats)) => {
			if !status.success() {
				RunOutcome::NonZero {
					stats,
					code: status.code(),
				}
			} else if stats.output.trim().is_empty() {
				RunOutcome::Empty(stats)
			} else {
				RunOutcome::Ok(stats)
			}
		}
		Err(e) => RunOutcome::SpawnError(e),
	}
}

/// Send `/done` to a named session so its context is compressed before
/// the next run. Best-effort: errors are logged-and-swallowed by the
/// caller (executor) — a failed `/done` should not abort the workflow.
pub async fn send_done(session_name: &str) -> Result<()> {
	let exe = std::env::current_exe().context("current_exe")?;
	let mut child = Command::new(exe)
		.arg("run")
		.arg("--name")
		.arg(session_name)
		.arg("--format")
		.arg("jsonl")
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.context("spawn /done failed")?;

	if let Some(mut stdin) = child.stdin.take() {
		stdin.write_all(b"/done\n").await.ok();
		stdin.shutdown().await.ok();
	}
	let _ = child.wait().await;
	Ok(())
}
