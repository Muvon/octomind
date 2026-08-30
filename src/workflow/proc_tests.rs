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
		serde_json::from_str(r#"{"type":"assistant","content":"hi","session_id":"s"}"#).unwrap();
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

#[tokio::test]
async fn run_step_forwards_skills_and_classifies_nonzero() {
	let args = RunStepArgs {
		role: "assistant".to_string(),
		prompt: "do the thing".to_string(),
		session_name: None,
		model: None,
		workdir: None,
		skills: Some(vec!["rust".to_string()]),
		capabilities: None,
		timeout_secs: 0,
		event_prefix: None,
		spinner: None,
		wf_start: Instant::now(),
		prior_cost: 0.0,
		prior_tools: 0,
	};
	let outcome = run_step(args).await;
	assert!(
		matches!(outcome, RunOutcome::NonZero { .. }),
		"forwarding OCTOMIND_SKILLS must not change the classification"
	);
}

#[tokio::test]
async fn run_step_missing_workdir_classifies_spawn_error() {
	let args = RunStepArgs {
		role: "assistant".to_string(),
		prompt: "do the thing".to_string(),
		session_name: None,
		model: None,
		workdir: Some(std::path::PathBuf::from("/definitely/not/a/dir-12345")),
		skills: None,
		capabilities: None,
		timeout_secs: 0,
		event_prefix: None,
		spinner: None,
		wf_start: Instant::now(),
		prior_cost: 0.0,
		prior_tools: 0,
	};
	let outcome = run_step(args).await;
	let RunOutcome::SpawnError { source, .. } = outcome else {
		panic!("expected SpawnError, got {outcome:?}");
	};
	assert!(
		source.to_string().contains("spawn failed"),
		"got: {source}"
	);
}

#[tokio::test]
async fn run_step_clears_spinner_on_failure() {
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
		spinner: Some(ProgressBar::new_spinner()),
		wf_start: Instant::now(),
		prior_cost: 0.0,
		prior_tools: 0,
	};
	let outcome = run_step(args).await;
	assert!(
		matches!(outcome, RunOutcome::NonZero { .. }),
		"the spinner is cleared on failure and the outcome still classifies"
	);
}

#[tokio::test]
async fn send_done_without_workdir_is_best_effort_ok() {
	send_done("wf-no-workdir", None)
		.await
		.expect("best-effort /done without a workdir returns Ok");
}
