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
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::websocket::ServerMessage;

/// Result of one `octomind run` invocation.
///
/// NOTE: `cost` and the four token totals are CUMULATIVE session figures as
/// reported by the subprocess's final `cost` event (session.info.total_*). For
/// a `session = "continue"` step resumed across loop iterations/retries each
/// invocation reports the running total, so the executor folds per-step deltas
/// before summing (see `Executor::fold_stats`). `tool_count`/`tool_failed`/
/// `duration` are per-invocation (counted from this subprocess's own stream).
#[derive(Debug, Clone, Default)]
pub struct StepStats {
	pub output: String,
	pub duration: Duration,
	pub cost: f64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub total_tokens: u64,
	pub cache_read_tokens: u64,
	pub cache_write_tokens: u64,
	pub reasoning_tokens: u64,
	/// Number of `ToolUse` events observed on the JSONL stream for this step.
	pub tool_count: u64,
	/// Of those, how many corresponding `ToolResult` events reported success=false.
	/// Fails are counted as they arrive, so a step that crashes mid-execution
	/// still reports the fails seen up to that point.
	pub tool_failed: u64,
}

/// Outcome categories surfaced to the executor (retry/timeout/etc).
#[derive(Debug)]
pub enum RunOutcome {
	Ok(StepStats),
	Empty(StepStats),
	/// `stderr_tail` is a truncated tail of the subprocess's stderr, captured
	/// for diagnostics — the child may die before emitting a structured
	/// `ServerMessage::Error` on stdout (startup failure, panic, upstream
	/// gateway error, etc.), in which case this is the only clue available.
	NonZero {
		stats: StepStats,
		code: Option<i32>,
		stderr_tail: String,
	},
	Timeout(Duration),
	/// `stderr_tail` is empty when the failure happened before the child was
	/// spawned (e.g. `current_exe()` lookup failed).
	SpawnError {
		source: anyhow::Error,
		stderr_tail: String,
	},
}

/// Max chars of stderr kept for diagnostics — enough to show a panic
/// message or an upstream HTTP error without dumping a full log.
const STDERR_TAIL_CHARS: usize = 800;

/// Bundled arguments for [`run_step`].
pub struct RunStepArgs {
	pub role: String,
	pub prompt: String,
	pub session_name: Option<String>,
	/// Optional complete model-profile override forwarded to the subprocess.
	pub model: crate::config::ModelProfileOverride,
	/// Absolute working directory for the subprocess. None = inherit cwd.
	pub workdir: Option<PathBuf>,
	/// Forwarded as `OCTOMIND_SKILLS` env var (comma-joined). None = unset.
	pub skills: Option<Vec<String>>,
	/// Forwarded as `OCTOMIND_CAPABILITIES` env var (comma-joined). None = unset.
	pub capabilities: Option<Vec<String>>,
	pub timeout_secs: u64,
	pub event_prefix: Option<String>,
	pub spinner: Option<ProgressBar>,
	pub wf_start: Instant,
	pub prior_cost: f64,
	pub prior_tools: u64,
}

/// Invoke `octomind run` with `prompt` on stdin, optional `--name` to
/// resume or create a named session, and `--format jsonl`.
///
/// `timeout_secs == 0` disables the timeout.
pub async fn run_step(args: RunStepArgs) -> RunOutcome {
	let RunStepArgs {
		role,
		prompt,
		session_name,
		model,
		workdir,
		skills,
		capabilities,
		timeout_secs,
		event_prefix,
		spinner,
		wf_start,
		prior_cost,
		prior_tools,
	} = args;

	let started = Instant::now();
	let exe = match std::env::current_exe() {
		Ok(p) => p,
		Err(e) => {
			return RunOutcome::SpawnError {
				source: e.into(),
				stderr_tail: String::new(),
			}
		}
	};

	let mut cmd = Command::new(&exe);
	cmd.arg("run")
		.arg(role)
		.arg("--format")
		.arg("jsonl")
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());
	if let Some(name) = session_name.as_deref() {
		cmd.arg("--name").arg(name);
	}
	if let Some(m) = &model.model {
		cmd.arg("--model").arg(m);
	}
	if let Some(value) = model.reasoning_effort {
		cmd.arg("--reasoning-effort").arg(value.as_str());
	}
	if let Some(value) = model.max_tokens {
		cmd.arg("--max-tokens").arg(value.to_string());
	}
	if let Some(value) = model.temperature {
		cmd.arg("--temperature").arg(value.to_string());
	}
	if let Some(value) = model.top_p {
		cmd.arg("--top-p").arg(value.to_string());
	}
	if let Some(value) = model.top_k {
		cmd.arg("--top-k").arg(value.to_string());
	}
	if let Some(value) = model.max_retries {
		cmd.arg("--max-retries").arg(value.to_string());
	}
	if let Some(value) = model.retry_timeout {
		cmd.arg("--retry-timeout").arg(value.to_string());
	}
	if let Some(value) = model.request_timeout_seconds {
		cmd.arg("--request-timeout-seconds").arg(value.to_string());
	}
	if let Some(dir) = &workdir {
		cmd.current_dir(dir);
	}
	if let Some(skills) = &skills {
		if !skills.is_empty() {
			cmd.env("OCTOMIND_SKILLS", skills.join(","));
		}
	}
	if let Some(capabilities) = &capabilities {
		if !capabilities.is_empty() {
			cmd.env("OCTOMIND_CAPABILITIES", capabilities.join(","));
		}
	}
	cmd.kill_on_drop(true);

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			return RunOutcome::SpawnError {
				source: anyhow!("spawn failed: {e}"),
				stderr_tail: String::new(),
			}
		}
	};

	// Write the prompt to stdin and close it.
	if let Some(mut stdin) = child.stdin.take() {
		let payload = prompt;
		tokio::spawn(async move {
			let _ = stdin.write_all(payload.as_bytes()).await;
			let _ = stdin.shutdown().await;
		});
	}

	let stdout = child.stdout.take().expect("stdout piped");
	let reader = BufReader::new(stdout);

	// Read stderr concurrently with stdout so neither pipe's OS buffer can
	// fill up and block the child — only used for diagnostics on failure,
	// the stdout JSONL stream remains the sole data contract.
	let stderr = child.stderr.take().expect("stderr piped");
	let stderr_task = tokio::spawn(async move {
		let mut buf = String::new();
		let mut lines = BufReader::new(stderr).lines();
		while let Ok(Some(line)) = lines.next_line().await {
			if !buf.is_empty() {
				buf.push('\n');
			}
			buf.push_str(&line);
		}
		buf
	});

	let collect = async {
		let mut stats = StepStats::default();
		let mut lines = reader.lines();
		while let Ok(Some(line)) = lines.next_line().await {
			if let Some(msg) = fold_stream_line(&line, &mut stats) {
				if let Some(sp) = &spinner {
					if let Some(line) = render_event_oneline(&msg) {
						let agg = fmt_aggregate(
							wf_start.elapsed(),
							prior_cost + stats.cost,
							prior_tools + stats.tool_count,
						);
						sp.set_message(format!("{line}   {agg}"));
					}
				} else if let Some(prefix) = event_prefix.as_deref() {
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
				if let Some(sp) = &spinner {
					sp.finish_and_clear();
				}
				stderr_task.abort();
				return RunOutcome::Timeout(started.elapsed());
			}
		}
	};

	// By the time `collect` resolves, the child has exited (or `wait`
	// failed), so its stderr pipe has already hit EOF — this returns
	// promptly rather than blocking on more output.
	let stderr_text = stderr_task.await.unwrap_or_default();

	if let Some(sp) = &spinner {
		sp.finish_and_clear();
	}

	match result {
		Ok((status, stats)) => {
			if !status.success() {
				RunOutcome::NonZero {
					stats,
					code: status.code(),
					stderr_tail: truncate_tail(&stderr_text, STDERR_TAIL_CHARS),
				}
			} else if stats.output.trim().is_empty() {
				RunOutcome::Empty(stats)
			} else {
				RunOutcome::Ok(stats)
			}
		}
		Err(e) => RunOutcome::SpawnError {
			source: e,
			stderr_tail: truncate_tail(&stderr_text, STDERR_TAIL_CHARS),
		},
	}
}

/// Parse one JSONL stream line and fold it into the running step stats.
/// Returns the parsed event so the caller can render it; blank and
/// unparseable lines are skipped silently (the subprocess's stdout contract
/// is best-effort JSONL).
fn fold_stream_line(line: &str, stats: &mut StepStats) -> Option<ServerMessage> {
	let trimmed = line.trim();
	if trimmed.is_empty() {
		return None;
	}
	let msg = serde_json::from_str::<ServerMessage>(trimmed).ok()?;
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
			stats.cache_read_tokens = c.cache_read_tokens;
			stats.cache_write_tokens = c.cache_write_tokens;
			stats.reasoning_tokens = c.reasoning_tokens;
		}
		ServerMessage::ToolUse(_) => {
			stats.tool_count += 1;
		}
		ServerMessage::ToolResult(p) if !p.success => {
			stats.tool_failed += 1;
		}
		_ => {}
	}
	Some(msg)
}

/// Workflow-level aggregate footer for the spinner: total elapsed time,
/// running cost so far, and total tools (current step + everything
/// finished before it). Dimmed so it doesn't fight with the live event.
fn fmt_aggregate(wf_elapsed: Duration, total_cost: f64, total_tools: u64) -> String {
	let bullet = "·".bright_black();
	format!(
		"{b} {dur} {b} ${cost:.4} {b} {tools_glyph}{tools}",
		b = bullet,
		dur = fmt_dur_compact(wf_elapsed).bright_black(),
		cost = total_cost,
		tools_glyph = "⚒".bright_black(),
		tools = total_tools.to_string().bright_black(),
	)
}

fn fmt_dur_compact(d: Duration) -> String {
	let secs = d.as_secs();
	if secs < 60 {
		format!("{secs}s")
	} else {
		let m = secs / 60;
		let s = secs % 60;
		format!("{m}m{s:02}s")
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
				eprintln!("{prefix}  {} {}", key.bright_black(), val);
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

/// Keep the last `max_chars` of `s` (panics/fatal errors usually land at
/// the end of stderr), prefixed with `…` if anything was cut.
fn truncate_tail(s: &str, max_chars: usize) -> String {
	let s = s.trim();
	let count = s.chars().count();
	if count <= max_chars {
		s.to_string()
	} else {
		let kept: String = s.chars().skip(count - max_chars).collect();
		format!("…{kept}")
	}
}

/// Send `/done` to a named session so its context is compressed before
/// the next run. Runs in the step's `workdir` so the resumed session
/// sees the same project context as the step itself. Best-effort:
/// errors are logged-and-swallowed by the caller (executor) — a failed
/// `/done` should not abort the workflow.
pub async fn send_done(session_name: &str, workdir: Option<&Path>) -> Result<()> {
	let exe = std::env::current_exe().context("current_exe")?;
	let mut cmd = Command::new(exe);
	cmd.arg("run")
		.arg("--name")
		.arg(session_name)
		.arg("--format")
		.arg("jsonl")
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null());
	if let Some(dir) = workdir {
		cmd.current_dir(dir);
	}
	let mut child = cmd.spawn().context("spawn /done failed")?;

	if let Some(mut stdin) = child.stdin.take() {
		stdin.write_all(b"/done\n").await.ok();
		stdin.shutdown().await.ok();
	}
	let _ = child.wait().await;
	Ok(())
}

#[cfg(test)]
#[path = "proc_tests.rs"]
mod tests;
