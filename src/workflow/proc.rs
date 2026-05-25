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
use colored::Colorize;
use indicatif::ProgressBar;
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
	/// Number of tool calls observed on the JSONL stream for this step.
	pub tool_count: u64,
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
	event_prefix: Option<&str>,
	spinner: Option<&ProgressBar>,
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
				match &msg {
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
					ServerMessage::ToolUse(_) => {
						stats.tool_count += 1;
					}
					_ => {}
				}
				if let Some(sp) = spinner {
					if let Some(line) = render_event_oneline(&msg) {
						sp.set_message(line);
					}
				} else if let Some(prefix) = event_prefix {
					render_event(prefix, &msg);
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

/// Render one JSONL stream event as a single compact line suitable for a
/// spinner message (no newlines, fits typical terminal width). Returns
/// `None` for events that shouldn't update the spinner (e.g. Assistant /
/// Cost / Thinking).
fn render_event_oneline(msg: &ServerMessage) -> Option<String> {
	let line = match msg {
		ServerMessage::ToolUse(p) => {
			let head = format!(
				"{arrow} {tool} {sep} {server}",
				arrow = "▸".bright_cyan(),
				tool = p.tool.bright_cyan(),
				sep = "·".bright_black(),
				server = p.server.bright_blue(),
			);
			let params = compact_params(&p.params);
			if params.is_empty() {
				head
			} else {
				let joined = params
					.iter()
					.map(|(k, v)| format!("{}={}", k.bright_black(), v))
					.collect::<Vec<_>>()
					.join(", ");
				// Truncate based on visible chars, not ANSI bytes. `truncate` is
				// char-aware but doesn't strip color codes, so the visible cap
				// is approximate — fine for terminal width budgeting.
				format!("{head}  {joined}")
			}
		}
		ServerMessage::Skill(p) => format!(
			"{glyph} skill {action} {name}",
			glyph = "▪".bright_yellow(),
			action = p.action.bright_black(),
			name = p.name.bright_yellow(),
		),
		ServerMessage::Status(p) => {
			let one = p.message.lines().next().unwrap_or("").trim();
			if one.is_empty() {
				return None;
			}
			format!(
				"{glyph} {msg}",
				glyph = "·".bright_black(),
				msg = truncate(one, 100).bright_black(),
			)
		}
		ServerMessage::McpNotification(p) => format!(
			"{glyph} {srv} {sep} {method}",
			glyph = "◆".bright_blue(),
			srv = p.server.bright_blue(),
			sep = "·".bright_black(),
			method = p.method.bright_black(),
		),
		ServerMessage::Error(p) => format!(
			"{glyph} {msg}",
			glyph = "✗".bright_red(),
			msg = truncate(&p.message, 120).red(),
		),
		_ => return None,
	};
	Some(line)
}

/// Render one JSONL stream event as a single compact stderr line under
/// the current step's rail. `prefix` already carries the rail glyph and
/// indentation; we just append the event-specific bit.
fn render_event(prefix: &str, msg: &ServerMessage) {
	match msg {
		ServerMessage::ToolUse(p) => {
			eprintln!(
				"{prefix}{arrow} {tool} {sep} {server}",
				arrow = "▸".bright_cyan(),
				tool = p.tool.bright_cyan(),
				sep = "·".bright_black(),
				server = p.server.bright_blue(),
			);
			// One line per param under the tool header — matches the
			// `│   key value` style of the in-session tool preview block.
			for (key, val) in compact_params(&p.params) {
				eprintln!("{prefix}   {} {}", key.bright_black(), val);
			}
		}
		ServerMessage::Skill(p) => {
			eprintln!(
				"{prefix}{glyph} skill {action} {name}",
				glyph = "▪".bright_yellow(),
				action = p.action.bright_black(),
				name = p.name.bright_yellow(),
			);
		}
		ServerMessage::Status(p) => {
			let one = p.message.lines().next().unwrap_or("").trim();
			if !one.is_empty() {
				eprintln!(
					"{prefix}{glyph} {msg}",
					glyph = "·".bright_black(),
					msg = truncate(one, 100).bright_black(),
				);
			}
		}
		ServerMessage::McpNotification(p) => {
			eprintln!(
				"{prefix}{glyph} {srv} {sep} {method}",
				glyph = "◆".bright_blue(),
				srv = p.server.bright_blue(),
				sep = "·".bright_black(),
				method = p.method.bright_black(),
			);
		}
		ServerMessage::Error(p) => {
			eprintln!(
				"{prefix}{glyph} {msg}",
				glyph = "✗".bright_red(),
				msg = truncate(&p.message, 200).red(),
			);
		}
		_ => {}
	}
}

/// Compact-format every non-empty param of a tool call as `(key, value)`
/// pairs preserving the JSON object's iteration order. Empty strings,
/// nulls, and empty containers are skipped. Each value is rendered as a
/// short single-line form (`"text"`, `42`, `true`, `[N items]`,
/// `{N keys}`) so both the spinner one-liner and the railed multi-line
/// view can share the same source of truth.
fn compact_params(params: &serde_json::Value) -> Vec<(String, String)> {
	let Some(obj) = params.as_object() else {
		return Vec::new();
	};
	obj.iter()
		.filter_map(|(k, v)| format_value_short(v).map(|s| (k.clone(), s)))
		.collect()
}

/// Render one JSON value as a short single-line string, or `None` if
/// the value carries no information worth showing (null / empty).
fn format_value_short(v: &serde_json::Value) -> Option<String> {
	match v {
		serde_json::Value::Null => None,
		serde_json::Value::Bool(b) => Some(b.to_string()),
		serde_json::Value::Number(n) => Some(n.to_string()),
		serde_json::Value::String(s) => {
			let s = s.trim();
			if s.is_empty() {
				None
			} else {
				Some(format!("\"{}\"", truncate(s, 60)))
			}
		}
		serde_json::Value::Array(arr) => {
			if arr.is_empty() {
				None
			} else if arr.len() <= 2 {
				let inner: Vec<String> = arr.iter().filter_map(format_value_short).collect();
				if inner.is_empty() {
					None
				} else {
					Some(format!("[{}]", inner.join(", ")))
				}
			} else {
				Some(format!("[{} items]", arr.len()))
			}
		}
		serde_json::Value::Object(o) => {
			if o.is_empty() {
				None
			} else {
				Some(format!("{{{} keys}}", o.len()))
			}
		}
	}
}

fn truncate(s: &str, n: usize) -> String {
	let one_line = s.replace('\n', " ");
	if one_line.chars().count() <= n {
		one_line
	} else {
		let head: String = one_line.chars().take(n.saturating_sub(1)).collect();
		format!("{head}…")
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
