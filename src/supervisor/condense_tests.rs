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

fn specs(v: &[&str]) -> Vec<String> {
	v.iter().map(|s| s.to_string()).collect()
}

fn adaptive_config(enabled: bool) -> crate::supervisor::CondenseConfig {
	crate::supervisor::CondenseConfig {
		enabled: true,
		adaptive: enabled,
		tokens_threshold: 5_000,
		model: "test:model".to_string(),
	}
}

#[test]
fn adaptive_threshold_starts_neutral_and_moves_with_realized_savings() {
	let mut strong = AdaptiveThresholdState::new(5_000, "test:model");
	assert_eq!(strong.threshold(), 5_000);
	strong.observe(10_000, 10_000);
	assert!(strong.threshold() < 5_000);

	let mut weak = AdaptiveThresholdState::new(5_000, "test:model");
	weak.observe(10_000, 0);
	assert!(weak.threshold() > 5_000);

	let mut neutral = AdaptiveThresholdState::new(5_000, "test:model");
	neutral.observe(10_000, 5_000);
	assert_eq!(neutral.threshold(), 5_000);
}

#[test]
fn adaptive_threshold_cannot_escape_half_to_double_baseline() {
	let mut state = AdaptiveThresholdState::new(5_000, "test:model");
	for _ in 0..1_000 {
		state.observe(10_000, 10_000);
	}
	assert!((2_500..=10_000).contains(&state.threshold()));

	for _ in 0..2_000 {
		state.observe(10_000, 0);
	}
	assert!((2_500..=10_000).contains(&state.threshold()));
}

#[test]
fn skipped_baseline_candidates_relax_a_raised_threshold_for_reprobe() {
	let mut state = AdaptiveThresholdState::new(5_000, "test:model");
	state.observe(10_000, 0);
	let raised = state.threshold();
	assert!(raised > 5_000);
	state.relax_toward_baseline();
	assert!(state.threshold() < raised);
}

#[tokio::test]
async fn adaptive_runtime_is_session_scoped_and_disabled_mode_stays_fixed() {
	let adaptive = adaptive_config(true);
	let fixed = adaptive_config(false);
	let first = "condense-adaptive-session-a".to_string();
	let second = "condense-adaptive-session-b".to_string();

	crate::session::context::with_session_id(first.clone(), async {
		assert_eq!(adaptive_threshold(&adaptive), 5_000);
		assert!(observe_adaptive_round(&adaptive, 10_000, 10_000) < 5_000);
		assert_eq!(adaptive_threshold(&fixed), 5_000);
	})
	.await;
	crate::session::context::with_session_id(second.clone(), async {
		assert_eq!(adaptive_threshold(&adaptive), 5_000);
	})
	.await;

	clear_for_session(&first);
	clear_for_session(&second);
}

#[test]
fn ranges_parse_single_span_open_and_invalid() {
	let r = parse_ranges(&specs(&["3", "5-7", "9-"]), 10).unwrap();
	assert_eq!(r, vec![(3, 3), (5, 7), (9, 10)]);
	// One malformed range invalidates the entire response; it is never
	// silently discarded while other selections are applied.
	assert!(parse_ranges(&specs(&["3", "junk"]), 10).is_none());
	assert!(parse_ranges(&specs(&["junk", "0"]), 10).is_none());
	assert!(parse_ranges(&specs(&["11-20"]), 10).is_none()); // beyond max
	assert!(parse_ranges(&specs(&["9-20"]), 10).is_none());
	assert!(parse_ranges(&specs(&["12-4"]), 10).is_none());
}

#[test]
fn ranges_merge_overlapping_and_adjacent() {
	let r = parse_ranges(&specs(&["1-3", "3-5", "6-8", "20-25"]), 30).unwrap();
	assert_eq!(r, vec![(1, 8), (20, 25)]);
}

#[test]
fn reconstruct_keeps_lines_verbatim_with_gap_markers() {
	let lines = vec!["a", "b", "c", "d", "e", "f"];
	let (body, kept) = reconstruct(&lines, &[(2, 3), (5, 5)], 6);
	assert_eq!(kept, 3);
	assert_eq!(
		body,
		"[... 1 lines omitted]\nb\nc\n[... 1 lines omitted]\ne\n[... 1 lines omitted]"
	);
}

#[test]
fn reconstruct_counts_capped_tail_the_model_never_saw() {
	let lines = vec!["a", "b"]; // capped view of a 10-line original
	let (body, kept) = reconstruct(&lines, &[(1, 2)], 10);
	assert_eq!(kept, 2);
	assert!(body.ends_with("[... 8 lines omitted]"));
}

#[test]
fn response_parses_fenced_and_bare_json() {
	let fenced =
		"rationale line\n```json\n{\"results\":[{\"id\":\"t1\",\"verdict\":\"keep\"}]}\n```";
	let p = parse_response(fenced).unwrap();
	assert_eq!(p.results[0].id, "t1");
	assert_eq!(p.results[0].verdict, "keep");

	let bare = "{\"results\":[{\"id\":\"t2\",\"verdict\":\"extract\",\"lines\":[\"1-4\"]}]}";
	let p = parse_response(bare).unwrap();
	assert_eq!(p.results[0].lines, vec!["1-4"]);
	assert!(parse_response("no json here").is_none());
}

#[test]
fn full_numbered_view_aligns_width() {
	let text = (0..10)
		.map(|i| i.to_string())
		.collect::<Vec<_>>()
		.join("\n");
	let view = build_numbered_view(&text, 1_000, "");
	assert!(!view.partial);
	assert_eq!(view.visible_ranges, vec![(1, 10)]);
	assert!(view.body.starts_with(" 1| 0"));
	assert!(view.body.ends_with("10| 9"));
}

#[test]
fn sampled_view_finds_middle_focus_and_tail_diagnostics() {
	let mut lines = (1..=1_000)
		.map(|i| format!("ordinary output {i}"))
		.collect::<Vec<_>>();
	lines[499] = "needle_symbol exact declaration".into();
	lines[998] = "fatal: build failed with exit code 9".into();
	let view = build_numbered_view(&lines.join("\n"), 500, "needle_symbol");
	assert!(view.partial);
	assert!(view.body.contains("500| needle_symbol exact declaration"));
	assert!(view
		.body
		.contains("999| fatal: build failed with exit code 9"));
	assert!(view
		.visible_ranges
		.iter()
		.any(|(s, e)| *s <= 500 && *e >= 500));
	assert!(view
		.visible_ranges
		.iter()
		.any(|(s, e)| *s <= 999 && *e >= 999));
}

#[test]
fn huge_line_preview_stays_one_selectable_record() {
	let content = "x".repeat(20_000);
	let view = build_numbered_view(&content, 128, "");
	assert_eq!(view.visible_ranges, vec![(1, 1)]);
	assert!(view.body.starts_with("1| "));
	assert!(view.body.contains("line preview clipped"));
	assert!(!view.body.ends_with('x'));
}

fn view(partial: bool, visible_ranges: Vec<(usize, usize)>) -> NumberedView {
	NumberedView {
		body: String::new(),
		visible_ranges,
		total_lines: 6,
		partial,
	}
}

fn entry(verdict: &str, lines: &[&str]) -> Entry {
	Entry {
		id: "t1".into(),
		verdict: verdict.into(),
		lines: specs(lines),
	}
}

#[test]
fn unseen_lines_are_clipped_away_not_smuggled_in() {
	let view = view(true, vec![(1, 2), (5, 6)]);
	// The model wrote one sweeping range over two islands: keep both islands,
	// drop the gap it never read.
	assert_eq!(
		clip_to_visible(vec![(1, 5)], &view),
		vec![(1, 2), (5, 5)],
		"a bridging range must survive as its visible parts"
	);
	assert_eq!(clip_to_visible(vec![(3, 4)], &view), Vec::new());
}

#[tokio::test]
async fn one_bad_verdict_does_not_cost_the_other_results() {
	crate::session::context::with_session_id("condense-test".into(), async {
		let ok = McpToolResult::success("shell".into(), "t1".into(), "a\nb\nc\nd\ne\nf".into());
		let original = ok.extract_content();
		let partial = view(true, vec![(1, 2), (5, 6)]);

		let kept = apply_verdict(&entry("extract", &["1-2", "5"]), &ok, &original, &partial)
			.expect("a valid selection applies");
		assert!(kept.starts_with("a\nb\n"));
		assert!(kept.contains(CONDENSE_NOTICE_TAG));

		// Each of these leaves ITS OWN result inline; the valid one above still
		// applied. Previously any single one of them voided the whole round.
		assert!(apply_verdict(&entry("keep", &[]), &ok, &original, &partial).is_none());
		assert!(
			apply_verdict(&entry("extract", &["1", "junk"]), &ok, &original, &partial).is_none()
		);
		assert!(apply_verdict(&entry("extract", &["3-4"]), &ok, &original, &partial).is_none());
		assert!(apply_verdict(&entry("nonsense", &[]), &ok, &original, &partial).is_none());
		// Nothing was inspected outside the islands, so "none of it matters" is
		// not a claim the model is allowed to make.
		assert!(apply_verdict(&entry("replace", &[]), &ok, &original, &partial).is_none());

		let error = McpToolResult::error("shell".into(), "t1".into(), "fatal".into());
		let full = view(false, vec![(1, 6)]);
		assert!(apply_verdict(&entry("replace", &[]), &error, "fatal", &full).is_none());
	})
	.await;
}

#[test]
fn duplicate_ids_are_dropped_rather_than_guessed() {
	let response = CondenseResponse {
		results: vec![
			entry("keep", &[]),
			entry("replace", &[]),
			Entry {
				id: "t2".into(),
				verdict: "keep".into(),
				lines: Vec::new(),
			},
		],
	};
	let entries = unambiguous_entries(&response);
	assert!(!entries.contains_key("t1"));
	assert!(entries.contains_key("t2"));
}

#[test]
fn diagnostics_are_retained_with_context() {
	let lines = vec!["a", "b", "fatal: nope", "d", "e", "f"];
	assert_eq!(diagnostic_ranges(&lines), vec![(1, 5)]);
}

#[test]
fn truncation_notice_is_never_selected_away() {
	let notice = format!(
		"{}: showing only the first tokens",
		crate::utils::truncation::TRUNCATION_NOTICE_TAG
	);
	let lines = vec!["a", "b", notice.as_str(), "  /tmp/spill.txt"];
	assert_eq!(truncation_notice_range(&lines), vec![(3, 4)]);
	assert!(truncation_notice_range(&["a", "b"]).is_empty());
}

#[test]
fn structured_results_are_not_flattened_for_condensation() {
	let plain = McpToolResult::success("tool".into(), "plain".into(), "text".into());
	assert!(is_plain_text_result(&plain));
	let structured = McpToolResult::success_with_metadata(
		"tool".into(),
		"rich".into(),
		"text".into(),
		serde_json::json!({"important": true}),
	);
	assert!(!is_plain_text_result(&structured));
}

/// One condensable scenario: what the agent was doing, what came back, and
/// the facts the agent still needs afterwards.
struct Scenario {
	name: &'static str,
	task: &'static str,
	tool: &'static str,
	args: serde_json::Value,
	output: String,
	is_error: bool,
	/// Substrings that MUST survive condensation — cutting one of these is
	/// the failure mode that costs the agent a whole recovery round.
	must_keep: &'static [&'static str],
}

fn repo_file(relative: &str) -> String {
	std::fs::read_to_string(format!("{}/{relative}", env!("CARGO_MANIFEST_DIR")))
		.unwrap_or_else(|e| panic!("fixture {relative}: {e}"))
}

fn build_log() -> String {
	let mut lines: Vec<String> = (1..=240)
		.map(|i| format!("   Compiling crate_number_{i} v0.{i}.3"))
		.collect();
	lines.push("error[E0308]: mismatched types".into());
	lines.push("   --> src/supervisor/condense.rs:412:17".into());
	lines.push("    |".into());
	lines.push("412 |         let kept: usize = ranges.len() as u64;".into());
	lines.push(
		"    |                   -----   ^^^^^^^^^^^^^^^^^^^ expected `usize`, found `u64`".into(),
	);
	lines.push("error: could not compile `octomind` (lib) due to 1 previous error".into());
	lines.join("\n")
}

fn unrelated_listing() -> String {
	(1..=600)
		.map(|i| format!("./vendor/assets/icons/glyph-{i:04}.svg"))
		.collect::<Vec<_>>()
		.join("\n")
}

/// Live end-to-end check of the condenser against the configured model.
/// Ignored by default (network + credentials); run with:
///   cargo test --lib supervisor::condense::tests::live -- --ignored --nocapture
#[tokio::test]
#[ignore = "live: calls the configured condense model"]
async fn live_condense_eval() {
	let config = crate::config::Config::load().expect("config loads");
	let scenarios = vec![
			Scenario {
				name: "source read, narrow task",
				task: "Goal: fix a wrong notice string.\nCurrent request: the truncation notice for oversized tool results reads badly — reword the text produced in handle_large_tool_results.",
				tool: "view",
				args: serde_json::json!({"path": "src/session/chat/response/tool_execution.rs"}),
				output: repo_file("src/session/chat/response/tool_execution.rs"),
				is_error: false,
				must_keep: &["handle_large_tool_results"],
			},
			Scenario {
				name: "failing build",
				task: "Goal: get the crate compiling.\nCurrent request: the build is broken, fix it.",
				tool: "shell",
				args: serde_json::json!({"command": "cargo build"}),
				output: build_log(),
				is_error: true,
				must_keep: &["error[E0308]", "condense.rs:412", "expected `usize`, found `u64`"],
			},
			Scenario {
				name: "irrelevant listing",
				task: "Goal: get the crate compiling.\nCurrent request: the build is broken, fix it.",
				tool: "shell",
				args: serde_json::json!({"command": "find ./vendor -name '*.svg'"}),
				output: unrelated_listing(),
				is_error: false,
				must_keep: &[],
			},
		];

	let mut total_before = 0usize;
	let mut total_after = 0usize;
	let mut report = Vec::new();
	for scenario in &scenarios {
		let result = if scenario.is_error {
			McpToolResult::error(scenario.tool.into(), "t1".into(), scenario.output.clone())
		} else {
			McpToolResult::success(scenario.tool.into(), "t1".into(), scenario.output.clone())
		};
		let call = McpToolCall {
			tool_name: scenario.tool.into(),
			parameters: scenario.args.clone(),
			tool_id: "t1".into(),
		};
		let sizes = vec![estimate_tokens(&scenario.output)];
		let (candidates, user) = build_request(
			std::slice::from_ref(&result),
			std::slice::from_ref(&call),
			&[0],
			&sizes,
			scenario.task,
			"",
			"",
		);
		let (_tx, rx) = tokio::sync::watch::channel(false);
		let response = crate::supervisor::learning::extract::call_learning_llm(
			&config,
			&config.supervisor.condense.model.clone(),
			SYSTEM_PROMPT.to_string(),
			user,
			crate::supervisor::stats::CallKind::Condense,
			rx,
		)
		.await
		.unwrap_or_else(|e| panic!("[{}] condense call failed: {e}", scenario.name));

		let parsed = parse_response(&response)
			.unwrap_or_else(|| panic!("[{}] unparseable: {response}", scenario.name));
		let entries = unambiguous_entries(&parsed);
		let entry = entries
			.get("t1")
			.unwrap_or_else(|| panic!("[{}] no verdict for t1", scenario.name));
		let condensed =
			crate::session::context::with_session_id("condense-live-eval".into(), async {
				apply_verdict(entry, &result, &scenario.output, &candidates[0].view)
			})
			.await;

		let before = sizes[0];
		let after = condensed
			.as_ref()
			.map_or(before, |content| estimate_tokens(content));
		let body = condensed.as_deref().unwrap_or(&scenario.output);
		for needle in scenario.must_keep {
			assert!(
				body.contains(needle),
				"[{}] verdict {} dropped required evidence {needle:?}",
				scenario.name,
				entry.verdict
			);
		}
		total_before += before;
		total_after += after;
		report.push(format!(
			"{:<24} {:>8} → {:<8} {:>4}% cut  verdict={}",
			scenario.name,
			before,
			after,
			100 - (after * 100 / before.max(1)),
			entry.verdict
		));
	}

	for line in &report {
		println!("{line}");
	}
	println!(
		"TOTAL {total_before} → {total_after} ({}% cut)",
		100 - (total_after * 100 / total_before.max(1))
	);
	// The whole point of the mechanic. A prompt that drifts back to keeping
	// everything passes every other test in this file and fails here.
	assert!(
		total_after * 2 < total_before,
		"condenser saved less than half of {total_before} tokens across scenarios"
	);
}

#[test]
fn bounded_context_preserves_both_ends() {
	let text = format!("HEAD {} TAIL", "middle ".repeat(2_000));
	let bounded = truncate_preserving_edges(&text, 100);
	assert!(bounded.starts_with("HEAD"));
	assert!(bounded.ends_with("TAIL"));
	assert!(bounded.contains("middle omitted for condenser budget"));
}
