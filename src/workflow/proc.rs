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
	/// Optional model override forwarded as `--model` to the subprocess.
	pub model: Option<String>,
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
	if let Some(m) = &model {
		cmd.arg("--model").arg(m);
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
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn truncate_keeps_short_text_and_flattens_newlines() {
		assert_eq!(truncate("abc", 10), "abc");
		assert_eq!(truncate("a\nb\nc", 10), "a b c");
		// Exactly at the cap is not truncated.
		assert_eq!(truncate("12345", 5), "12345");
	}

	#[test]
	fn truncate_counts_chars_not_bytes() {
		// 6 multi-byte chars capped at 4 → 3 kept + ellipsis. A byte-based
		// slice here would panic on a char boundary.
		assert_eq!(truncate("привет", 4), "при…");
		let out = truncate("日本語テキスト", 3);
		assert_eq!(out.chars().count(), 3);
		assert!(out.ends_with('…'));
	}

	#[test]
	fn truncate_zero_cap_is_ellipsis_only() {
		assert_eq!(truncate("anything", 0), "…");
	}

	#[test]
	fn truncate_tail_keeps_the_end() {
		assert_eq!(truncate_tail("  short  ", 800), "short");
		// The tail is what matters — panics land at the end of stderr.
		let long: String = std::iter::repeat_n('x', 100)
			.chain(['E', 'N', 'D'])
			.collect();
		let tail = truncate_tail(&long, 5);
		assert_eq!(tail, "…xxEND");
	}

	#[test]
	fn truncate_tail_counts_chars_not_bytes() {
		let s = "日本語テキストです";
		let tail = truncate_tail(s, 4);
		assert_eq!(tail, "…ストです");
	}

	#[test]
	fn fmt_dur_compact_pads_seconds_after_a_minute() {
		assert_eq!(fmt_dur_compact(Duration::from_secs(0)), "0s");
		assert_eq!(fmt_dur_compact(Duration::from_secs(59)), "59s");
		assert_eq!(fmt_dur_compact(Duration::from_secs(60)), "1m00s");
		assert_eq!(fmt_dur_compact(Duration::from_secs(125)), "2m05s");
		assert_eq!(fmt_dur_compact(Duration::from_secs(3600)), "60m00s");
	}

	#[test]
	fn format_value_short_drops_uninformative_values() {
		assert_eq!(format_value_short(&json!(null)), None);
		assert_eq!(format_value_short(&json!("")), None);
		assert_eq!(format_value_short(&json!("   ")), None);
		assert_eq!(format_value_short(&json!([])), None);
		assert_eq!(format_value_short(&json!({})), None);
		// An array whose every element is uninformative carries nothing either.
		assert_eq!(format_value_short(&json!([null, null])), None);
	}

	#[test]
	fn format_value_short_renders_scalars_and_containers() {
		assert_eq!(format_value_short(&json!(true)).unwrap(), "true");
		assert_eq!(format_value_short(&json!(42)).unwrap(), "42");
		assert_eq!(format_value_short(&json!(" hi ")).unwrap(), "\"hi\"");
		assert_eq!(
			format_value_short(&json!(["a", "b"])).unwrap(),
			"[\"a\", \"b\"]"
		);
		// Three or more elements collapse to a count.
		assert_eq!(format_value_short(&json!([1, 2, 3])).unwrap(), "[3 items]");
		assert_eq!(
			format_value_short(&json!({"a": 1, "b": 2})).unwrap(),
			"{2 keys}"
		);
	}

	#[test]
	fn format_value_short_truncates_long_strings() {
		let long = "y".repeat(200);
		let out = format_value_short(&json!(long)).unwrap();
		// 60 visible chars (59 + ellipsis) inside quotes.
		assert_eq!(out.chars().count(), 62);
		assert!(out.starts_with('"') && out.ends_with('"'));
	}

	#[test]
	fn compact_params_skips_empty_and_ignores_non_objects() {
		let params = json!({
			"path": "src/main.rs",
			"empty": "",
			"nothing": null,
			"lines": 12,
		});
		let pairs = compact_params(&params);
		assert_eq!(pairs.len(), 2);
		assert!(pairs.contains(&("path".to_string(), "\"src/main.rs\"".to_string())));
		assert!(pairs.contains(&("lines".to_string(), "12".to_string())));

		assert!(compact_params(&json!("not an object")).is_empty());
		assert!(compact_params(&json!(null)).is_empty());
	}

	// ── JSONL stream folding ───────────────────────────────────────────────

	fn fold_lines(lines: &[&str]) -> StepStats {
		let mut stats = StepStats::default();
		for line in lines {
			fold_stream_line(line, &mut stats);
		}
		stats
	}

	#[test]
	fn fold_stream_line_accumulates_assistant_output_with_newlines() {
		let stats = fold_lines(&[
			r#"{"type":"assistant","content":"part one","session_id":"s"}"#,
			"   ",
			r#"{"type":"assistant","content":"part two","session_id":"s"}"#,
		]);
		assert_eq!(stats.output, "part one\npart two");
	}

	#[test]
	fn fold_stream_line_snapshots_cumulative_cost_fields() {
		let stats = fold_lines(&[
			r#"{"type":"cost","session_tokens":100,"session_cost":0.5,"input_tokens":60,"output_tokens":40,"cache_read_tokens":7,"cache_write_tokens":3,"reasoning_tokens":11,"session_id":"s"}"#,
			r#"{"type":"cost","session_tokens":250,"session_cost":1.25,"input_tokens":150,"output_tokens":100,"cache_read_tokens":9,"cache_write_tokens":5,"reasoning_tokens":13,"session_id":"s"}"#,
		]);
		assert_eq!(stats.total_tokens, 250);
		assert!((stats.cost - 1.25).abs() < f64::EPSILON);
		assert_eq!(stats.input_tokens, 150);
		assert_eq!(stats.output_tokens, 100);
		assert_eq!(stats.cache_read_tokens, 9);
		assert_eq!(stats.cache_write_tokens, 5);
		assert_eq!(stats.reasoning_tokens, 13);
	}

	#[test]
	fn fold_stream_line_counts_tool_uses_and_only_failed_results() {
		let stats = fold_lines(&[
			r#"{"type":"tool_use","tool":"read","tool_id":"t1","server":"core","params":{},"session_id":"s"}"#,
			r#"{"type":"tool_use","tool":"write","tool_id":"t2","server":"core","params":{},"session_id":"s"}"#,
			r#"{"type":"tool_result","tool":"read","tool_id":"t1","server":"core","content":"ok","success":true,"session_id":"s"}"#,
			r#"{"type":"tool_result","tool":"write","tool_id":"t2","server":"core","content":"boom","success":false,"session_id":"s"}"#,
		]);
		assert_eq!(stats.tool_count, 2);
		assert_eq!(stats.tool_failed, 1);
	}

	#[test]
	fn fold_stream_line_skips_blank_and_malformed_lines() {
		let mut stats = StepStats::default();
		assert!(fold_stream_line("", &mut stats).is_none());
		assert!(fold_stream_line("not json at all", &mut stats).is_none());
		assert!(fold_stream_line("{\"type\":\"status\",\"message\":\"hi\"}", &mut stats).is_some());
		assert_eq!(stats.output, "");
		assert_eq!(stats.tool_count, 0);
	}

	#[test]
	fn fold_stream_line_ignores_non_stat_events() {
		let stats = fold_lines(&[
			r#"{"type":"thinking","content":"hmm","session_id":"s"}"#,
			r#"{"type":"status","message":"working"}"#,
			r#"{"type":"error","message":"boom"}"#,
		]);
		assert_eq!(stats.output, "");
		assert_eq!(stats.cost, 0.0);
		assert_eq!(stats.tool_count, 0);
		assert_eq!(stats.tool_failed, 0);
	}

	// ── event rendering ───────────────────────────────────────────────────

	#[test]
	fn render_event_oneline_covers_live_variants_and_skips_quiet_ones() {
		let tool_use: ServerMessage = serde_json::from_str(
			r#"{"type":"tool_use","tool":"read","tool_id":"t1","server":"core","params":{"path":"src/main.rs"},"session_id":"s"}"#,
		)
		.unwrap();
		let line = render_event_oneline(&tool_use).expect("tool use renders");
		assert!(line.contains("read"));
		assert!(line.contains("core"));
		assert!(line.contains("src/main.rs"));

		let bare: ServerMessage = serde_json::from_str(
			r#"{"type":"tool_use","tool":"list","tool_id":"t1","server":"core","params":{},"session_id":"s"}"#,
		)
		.unwrap();
		assert!(
			render_event_oneline(&bare).is_some(),
			"param-less tool use still renders"
		);

		let skill: ServerMessage = serde_json::from_str(
			r#"{"type":"skill","action":"activate","name":"rust","session_id":"s"}"#,
		)
		.unwrap();
		assert!(render_event_oneline(&skill)
			.expect("skill renders")
			.contains("rust"));

		let status: ServerMessage =
			serde_json::from_str(r#"{"type":"status","message":"compiling crate\nmore detail"}"#)
				.unwrap();
		assert!(render_event_oneline(&status)
			.expect("status renders")
			.contains("compiling crate"));

		let blank_status: ServerMessage =
			serde_json::from_str(r#"{"type":"status","message":"   "}"#).unwrap();
		assert!(render_event_oneline(&blank_status).is_none());

		let notification: ServerMessage = serde_json::from_str(
			r#"{"type":"mcp_notification","server":"db","method":"notifications/progress","params":{}}"#,
		)
		.unwrap();
		assert!(render_event_oneline(&notification)
			.expect("notification renders")
			.contains("db"));

		let error: ServerMessage =
			serde_json::from_str(r#"{"type":"error","message":"gateway 502"}"#).unwrap();
		assert!(render_event_oneline(&error)
			.expect("error renders")
			.contains("gateway 502"));

		// Quiet events never touch the spinner.
		let assistant: ServerMessage =
			serde_json::from_str(r#"{"type":"assistant","content":"hi","session_id":"s"}"#)
				.unwrap();
		assert!(render_event_oneline(&assistant).is_none());
		let cost: ServerMessage = serde_json::from_str(
			r#"{"type":"cost","session_tokens":1,"session_cost":0.0,"input_tokens":1,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"reasoning_tokens":0,"session_id":"s"}"#,
		)
		.unwrap();
		assert!(render_event_oneline(&cost).is_none());
	}

	#[test]
	fn fmt_aggregate_shows_time_cost_and_tools() {
		let agg = fmt_aggregate(Duration::from_secs(5), 0.25, 3);
		assert!(agg.contains("5s"));
		assert!(agg.contains("0.2500"));
		assert!(agg.contains('3'));
	}

	#[test]
	fn render_event_prints_every_live_variant_without_panic() {
		// Smoke: the railed renderer must handle every variant; quiet ones are
		// silently skipped, live ones print under the prefix.
		let events = [
			r#"{"type":"tool_use","tool":"read","tool_id":"t1","server":"core","params":{"path":"x"},"session_id":"s"}"#,
			r#"{"type":"skill","action":"use","name":"rust","session_id":"s"}"#,
			r#"{"type":"status","message":"working"}"#,
			r#"{"type":"mcp_notification","server":"db","method":"notifications/message","params":{}}"#,
			r#"{"type":"error","message":"boom"}"#,
			r#"{"type":"assistant","content":"quiet","session_id":"s"}"#,
		];
		for raw in events {
			let msg: ServerMessage = serde_json::from_str(raw).unwrap();
			render_event("  │ ", &msg);
		}
	}

	// ── subprocess lifecycle ──────────────────────────────────────────────

	#[tokio::test]
	async fn run_step_classifies_nonzero_exit_from_test_binary() {
		let args = RunStepArgs {
			role: "assistant".to_string(),
			prompt: "do the thing".to_string(),
			session_name: None,
			model: None,
			workdir: None,
			skills: None,
			capabilities: None,
			timeout_secs: 0,
			event_prefix: None,
			spinner: None,
			wf_start: Instant::now(),
			prior_cost: 0.0,
			prior_tools: 0,
		};
		// current_exe() under `cargo test` is the test binary itself; the
		// libtest harness rejects `--format jsonl` and exits non-zero without
		// touching the network or any real model.
		let outcome = run_step(args).await;
		let RunOutcome::NonZero {
			stats,
			code,
			stderr_tail,
		} = outcome
		else {
			panic!("expected NonZero, got {outcome:?}");
		};
		assert!(
			code.is_some_and(|c| c != 0),
			"libtest arg error must exit non-zero"
		);
		assert!(
			!stderr_tail.is_empty(),
			"diagnostic stderr must be captured"
		);
		assert!(stats.output.is_empty(), "no assistant events can arrive");
		assert_eq!(stats.tool_count, 0);
	}

	#[tokio::test]
	async fn run_step_with_full_args_and_timeout_wrapper_classifies_nonzero() {
		let workdir = tempfile::tempdir().expect("temp workdir");
		let args = RunStepArgs {
			role: "assistant".to_string(),
			prompt: "do the thing".to_string(),
			session_name: Some("wf-proc-test".to_string()),
			model: Some("ollama:fake-model".to_string()),
			workdir: Some(workdir.path().to_path_buf()),
			skills: Some(Vec::new()),
			capabilities: Some(vec!["cap-a".to_string()]),
			timeout_secs: 30,
			event_prefix: Some("  │ ".to_string()),
			spinner: None,
			wf_start: Instant::now(),
			prior_cost: 0.0,
			prior_tools: 0,
		};
		let outcome = run_step(args).await;
		assert!(
			matches!(outcome, RunOutcome::NonZero { .. }),
			"libtest arg error must classify as NonZero"
		);
	}

	#[tokio::test]
	async fn send_done_is_best_effort_and_returns_ok() {
		let dir = tempfile::tempdir().expect("temp workdir");
		send_done("__no_such_session", Some(dir.path()))
			.await
			.expect("best-effort /done always returns Ok");
	}
}
