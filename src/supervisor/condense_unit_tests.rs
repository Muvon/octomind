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

//! Branch-level unit tests for the condenser's pure helpers. Complements the
//! behavioral tests in `mod tests` and the gated-pipeline e2e tests: every test
//! here pins one input→output edge of a primitive in isolation.

use super::*;

fn specs(v: &[&str]) -> Vec<String> {
	v.iter().map(|s| s.to_string()).collect()
}

fn entry(verdict: &str, lines: &[&str]) -> Entry {
	Entry {
		id: "t1".into(),
		verdict: verdict.into(),
		lines: specs(lines),
	}
}

fn full_view(total_lines: usize) -> NumberedView {
	NumberedView {
		body: String::new(),
		visible_ranges: vec![(1, total_lines)],
		total_lines,
		partial: false,
	}
}

fn close(a: f64, b: f64) -> bool {
	(a - b).abs() < 1e-9
}

fn config(
	adaptive: bool,
	tokens_threshold: usize,
	_model: &str,
) -> crate::supervisor::CondenseConfig {
	crate::supervisor::CondenseConfig {
		enabled: true,
		adaptive,
		tokens_threshold,
	}
}

// ---------------------------------------------------------------------------
// AdaptiveThresholdState
// ---------------------------------------------------------------------------

#[test]
fn new_state_carries_neutral_prior() {
	let state = AdaptiveThresholdState::new(5_000);
	assert_eq!(state.baseline, 5_000);
	assert!(close(state.savings_ewma, ADAPTIVE_TARGET_SAVINGS));
	assert!(close(state.multiplier(), 1.0));
	assert_eq!(state.threshold(), 5_000);
}

#[test]
fn matches_requires_same_baseline() {
	let state = AdaptiveThresholdState::new(5_000);
	assert!(state.matches(&config(true, 5_000, "m")));
	assert!(!state.matches(&config(true, 8_000, "m")));
}

#[test]
fn multiplier_maps_savings_log_linearly_and_clamps() {
	let mut state = AdaptiveThresholdState::new(1_000);
	for (savings, expected) in [(0.5, 1.0), (0.0, 2.0), (1.0, 0.5)] {
		state.savings_ewma = savings;
		assert!(close(state.multiplier(), expected), "q={savings}");
	}
	// Out-of-domain savings saturate at the multiplier bounds instead of
	// extrapolating: the controller must stay bounded by construction.
	state.savings_ewma = -1.0;
	assert!(close(state.multiplier(), ADAPTIVE_MAX_MULTIPLIER));
	state.savings_ewma = 2.0;
	assert!(close(state.multiplier(), ADAPTIVE_MIN_MULTIPLIER));
}

#[test]
fn threshold_rounds_and_clamps_odd_baselines() {
	let mut state = AdaptiveThresholdState::new(5);
	state.savings_ewma = 1.0; // 5 * 0.5 = 2.5 → rounds to 3, floor is div_ceil(5,2)=3
	assert_eq!(state.threshold(), 3);
	state.savings_ewma = 0.0; // 5 * 2 = 10
	assert_eq!(state.threshold(), 10);

	let mut one = AdaptiveThresholdState::new(1);
	one.savings_ewma = 1.0; // 0.5 → rounds to 1, clamped to [1, 2]
	assert_eq!(one.threshold(), 1);
	one.savings_ewma = 0.0;
	assert_eq!(one.threshold(), 2);
}

#[test]
fn observe_ignores_zero_attempted_and_clamps_saved_to_attempted() {
	let mut state = AdaptiveThresholdState::new(5_000);
	state.observe(0, 9_999);
	assert!(close(state.savings_ewma, ADAPTIVE_TARGET_SAVINGS));

	state.observe(1_000, 1_000);
	assert!(close(state.savings_ewma, 0.625)); // 0.5 + 0.25 * (1.0 - 0.5)

	let mut clamped = AdaptiveThresholdState::new(5_000);
	clamped.observe(100, 100_000); // saved > attempted is nonsense, not 100%+ savings
	let mut honest = AdaptiveThresholdState::new(5_000);
	honest.observe(100, 100);
	assert!(close(clamped.savings_ewma, honest.savings_ewma));
}

#[test]
fn relax_moves_toward_neutral_from_both_sides_and_holds_at_neutral() {
	let mut high = AdaptiveThresholdState::new(5_000);
	high.savings_ewma = 1.0;
	high.relax_toward_baseline();
	assert!(close(high.savings_ewma, 0.95)); // 1.0 + 0.1 * (0.5 - 1.0)

	let mut low = AdaptiveThresholdState::new(5_000);
	low.savings_ewma = 0.0;
	low.relax_toward_baseline();
	assert!(close(low.savings_ewma, 0.05));

	let mut neutral = AdaptiveThresholdState::new(5_000);
	neutral.relax_toward_baseline();
	assert!(close(neutral.savings_ewma, ADAPTIVE_TARGET_SAVINGS));
}

// ---------------------------------------------------------------------------
// Adaptive runtime registry
// ---------------------------------------------------------------------------

#[test]
fn adaptive_runtime_without_session_returns_the_configured_baseline() {
	let cfg = config(true, 5_000, "m");
	assert_eq!(adaptive_threshold(&cfg), 5_000);
	assert_eq!(observe_adaptive_round(&cfg, 10_000, 10_000), 5_000);
	assert_eq!(relax_adaptive_threshold(&cfg), 5_000);
}

#[tokio::test]
async fn adaptive_runtime_resets_state_when_the_config_changes() {
	let session = "condense-unit-cfg-change".to_string();
	let a = config(true, 5_000, "model-a");
	let b = config(true, 8_000, "model-b");
	crate::session::context::with_session_id(session.clone(), async {
		let raised = observe_adaptive_round(&a, 10_000, 0);
		assert!(raised > 5_000, "a weak round raises the trigger");
		// A different baseline/model is a different controller: the stale
		// raised state must not leak into the new configuration.
		assert_eq!(adaptive_threshold(&b), 8_000);
		assert!(observe_adaptive_round(&b, 10_000, 10_000) < 8_000);
		assert!(relax_adaptive_threshold(&a) > 0);
	})
	.await;
	clear_for_session(&session);
}

// ---------------------------------------------------------------------------
// parse_ranges / merge_ranges
// ---------------------------------------------------------------------------

#[test]
fn ranges_parse_rejects_empty_specs_zero_and_zero_max() {
	assert!(parse_ranges(&specs(&[]), 10).is_none());
	assert!(parse_ranges(&specs(&["0"]), 10).is_none());
	assert!(parse_ranges(&specs(&["0-3"]), 10).is_none());
	assert!(parse_ranges(&specs(&["1"]), 0).is_none());
	// Whitespace around the numbers is tolerated.
	assert_eq!(
		parse_ranges(&specs(&[" 3 - 5 "]), 10).unwrap(),
		vec![(3, 5)]
	);
	// Open-ended at the exact last line collapses to that line.
	assert_eq!(parse_ranges(&specs(&["10-"]), 10).unwrap(), vec![(10, 10)]);
	// Duplicate selections merge instead of double-counting.
	assert_eq!(
		parse_ranges(&specs(&["2", "2-3"]), 10).unwrap(),
		vec![(2, 3)]
	);
}

#[test]
fn merge_ranges_sorts_unions_and_keeps_one_line_gaps_separate() {
	assert_eq!(merge_ranges(vec![]), Vec::<(usize, usize)>::new());
	assert_eq!(
		merge_ranges(vec![(5, 6), (1, 2), (2, 3)]),
		vec![(1, 3), (5, 6)],
		"unsorted overlapping input still unions"
	);
	assert_eq!(merge_ranges(vec![(1, 10), (3, 5)]), vec![(1, 10)]);
	assert_eq!(
		merge_ranges(vec![(1, 2), (4, 5)]),
		vec![(1, 2), (4, 5)],
		"a one-line gap is real evidence, not an overlap"
	);
}

// ---------------------------------------------------------------------------
// reconstruct
// ---------------------------------------------------------------------------

#[test]
fn reconstruct_full_range_is_verbatim_with_no_markers() {
	let lines = vec!["a", "b", "c", "d", "e", "f"];
	let (body, kept) = reconstruct(&lines, &[(1, 6)], 6);
	assert_eq!(body, "a\nb\nc\nd\ne\nf");
	assert_eq!(kept, 6);
	assert!(!body.contains("[..."));
}

#[test]
fn reconstruct_with_no_ranges_marks_everything_omitted() {
	let lines = vec!["a", "b", "c"];
	let (body, kept) = reconstruct(&lines, &[], 3);
	assert_eq!(body, "[... 3 lines omitted]");
	assert_eq!(kept, 0);
}

#[test]
fn reconstruct_range_from_line_one_has_no_leading_marker() {
	let lines = vec!["a", "b", "c", "d"];
	let (body, kept) = reconstruct(&lines, &[(1, 1), (3, 3)], 4);
	assert_eq!(body, "a\n[... 1 lines omitted]\nc\n[... 1 lines omitted]");
	assert_eq!(kept, 2);
}

// ---------------------------------------------------------------------------
// build_numbered_view / format_ranges / indices_to_ranges
// ---------------------------------------------------------------------------

#[test]
fn empty_content_yields_an_empty_view() {
	let view = build_numbered_view("", 1_000, "focus");
	assert_eq!(view.body, "");
	assert!(view.visible_ranges.is_empty());
	assert_eq!(view.total_lines, 0);
	assert!(!view.partial);
}

#[test]
fn format_ranges_renders_singles_and_spans() {
	assert_eq!(format_ranges(&[]), Vec::<String>::new());
	assert_eq!(
		format_ranges(&[(3, 3), (1, 5)]),
		vec!["3".to_string(), "1-5".to_string()]
	);
}

#[test]
fn indices_to_ranges_groups_consecutive_runs() {
	assert_eq!(indices_to_ranges(&[]), Vec::<(usize, usize)>::new());
	assert_eq!(indices_to_ranges(&[0, 1, 2]), vec![(1, 3)]);
	assert_eq!(indices_to_ranges(&[0, 2, 5]), vec![(1, 1), (3, 3), (6, 6)]);
	assert_eq!(indices_to_ranges(&[2, 3, 5, 6, 7]), vec![(3, 4), (6, 8)]);
}

// ---------------------------------------------------------------------------
// unambiguous_entries / is_plain_text_result / set_content
// ---------------------------------------------------------------------------

#[test]
fn unique_ids_are_all_indexed_and_empty_stays_empty() {
	let response = CondenseResponse {
		results: vec![
			Entry {
				id: "t1".into(),
				verdict: "keep".into(),
				lines: Vec::new(),
			},
			Entry {
				id: "t2".into(),
				verdict: "extract".into(),
				lines: specs(&["1-2"]),
			},
			Entry {
				id: "t3".into(),
				verdict: "replace".into(),
				lines: Vec::new(),
			},
		],
	};
	let entries = unambiguous_entries(&response);
	assert_eq!(entries.len(), 3);
	assert_eq!(entries["t2"].verdict, "extract");

	assert!(unambiguous_entries(&CondenseResponse {
		results: Vec::new()
	})
	.is_empty());
}

#[test]
fn plain_text_check_accepts_null_structured_and_empty_content_but_not_rich_blocks() {
	let mut null_structured = McpToolResult::success("tool".into(), "t".into(), "text".into());
	null_structured.result.structured_content = Some(serde_json::Value::Null);
	assert!(is_plain_text_result(&null_structured));

	let mut empty = McpToolResult::success("tool".into(), "t".into(), "text".into());
	empty.result.content = Vec::new();
	assert!(is_plain_text_result(&empty));

	let mut mixed = McpToolResult::success("tool".into(), "t".into(), "text".into());
	mixed
		.result
		.content
		.push(rmcp::model::ContentBlock::image("aGk=", "image/png"));
	assert!(!is_plain_text_result(&mixed));
}

#[test]
fn set_content_replaces_text_and_preserves_the_error_flag() {
	let mut ok = McpToolResult::success("tool".into(), "t".into(), "old".into());
	set_content(&mut ok, "new body".into());
	assert_eq!(ok.extract_content(), "new body");
	assert!(!ok.is_error());

	let mut failed = McpToolResult::error("tool".into(), "t".into(), "old".into());
	set_content(&mut failed, "new body".into());
	assert_eq!(failed.extract_content(), "new body");
	assert!(
		failed.is_error(),
		"a condensed failing tool must stay an error"
	);
}

// ---------------------------------------------------------------------------
// diagnostic_ranges / focus_terms / compact_args / estimate_tokens
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_context_clamps_at_edges_and_merges_nearby_windows() {
	assert_eq!(diagnostic_ranges(&["a", "b"]), Vec::<(usize, usize)>::new());

	// Hit on the first line: the ±2 context cannot underflow below line 1.
	assert_eq!(
		diagnostic_ranges(&["error: first", "b", "c", "d"]),
		vec![(1, 3)]
	);

	// Hit on the last line: the window stops at the end of the file.
	assert_eq!(
		diagnostic_ranges(&["a", "b", "c", "d", "e", "panic: last"]),
		vec![(4, 6)]
	);

	// Two hits four lines apart: their context windows overlap and merge.
	assert_eq!(
		diagnostic_ranges(&["error: a", "b", "c", "failed here", "e", "f"]),
		vec![(1, 6)]
	);
}

#[test]
fn focus_terms_keep_paths_intact_and_drop_stopwords_short_and_duplicates() {
	assert!(focus_terms("").is_empty());
	assert_eq!(
		focus_terms("Fix this error in src/main.rs"),
		vec![
			"fix".to_string(),
			"error".to_string(),
			"src/main.rs".to_string()
		]
	);
	// Punctuation splits terms; digits are alphanumeric and survive.
	assert_eq!(focus_terms("a,b 123"), vec!["123".to_string()]);
	// Duplicates collapse to one term.
	assert_eq!(focus_terms("error error"), vec!["error".to_string()]);
}

#[test]
fn compact_args_passes_small_values_through_verbatim() {
	assert_eq!(compact_args(&serde_json::json!({"a": 1})), r#"{"a":1}"#);
	assert_eq!(compact_args(&serde_json::Value::Null), "null");
	assert_eq!(compact_args(&serde_json::json!("str")), r#""str""#);
	assert_eq!(compact_args(&serde_json::json!([1, 2])), "[1,2]");
}

#[test]
fn compact_args_elides_the_middle_of_oversized_values() {
	let value = serde_json::json!({ "data": "x".repeat(3_000) });
	let s = value.to_string();
	assert!(s.len() > ARGS_CAP_CHARS);
	let out = compact_args(&value);
	assert!(out.contains("…[args middle omitted]…"));
	assert!(out.starts_with(&s[..100]), "head survives verbatim");
	assert!(out.ends_with(&s[s.len() - 100..]), "tail survives verbatim");
	assert!(out.len() <= ARGS_CAP_CHARS + 64, "bounded around the cap");
}

#[test]
fn token_estimate_is_zero_for_empty_and_grows_with_text() {
	assert_eq!(estimate_tokens(""), 0);
	let short = estimate_tokens("one line");
	let long = estimate_tokens(&"word ".repeat(1_000));
	assert!(short > 0);
	assert!(long > short);
}

// ---------------------------------------------------------------------------
// apply_verdict
// ---------------------------------------------------------------------------

#[test]
fn extract_without_a_session_fails_closed_rather_than_losing_the_original() {
	// No session context → no spill file → the verdict must be refused:
	// replacing inline content with no on-disk copy is never acceptable.
	let ok = McpToolResult::success("shell".into(), "t1".into(), "a\nb\nc\nd\ne\nf".into());
	let original = ok.extract_content();
	assert!(apply_verdict(&entry("extract", &["1-2"]), &ok, &original, &full_view(6)).is_none());
}

#[tokio::test]
async fn extract_selecting_everything_is_treated_as_keep() {
	crate::session::context::with_session_id("condense-unit-extract-all".to_string(), async {
		let ok = McpToolResult::success("shell".into(), "t1".into(), "a\nb\nc\nd\ne\nf".into());
		let original = ok.extract_content();
		assert!(
			apply_verdict(&entry("extract", &["1-6"]), &ok, &original, &full_view(6)).is_none(),
			"keeping every line is a keep, not a condensation"
		);
	})
	.await;
}

#[tokio::test]
async fn extract_protects_diagnostics_the_selection_missed() {
	crate::session::context::with_session_id("condense-unit-extract-diag".to_string(), async {
		let mut lines: Vec<String> = (1..=10).map(|i| format!("l{i}")).collect();
		lines[7] = "error: boom".into();
		let ok = McpToolResult::success("shell".into(), "t1".into(), lines.join("\n"));
		let original = ok.extract_content();
		let out = apply_verdict(&entry("extract", &["1-2"]), &ok, &original, &full_view(10))
			.expect("a partial selection applies");
		assert!(out.starts_with("l1\nl2\n"));
		assert!(out.contains("[... 3 lines omitted]"));
		assert!(
			out.contains("error: boom"),
			"diagnostics survive a selection that missed them"
		);
		assert!(out.contains(CONDENSE_NOTICE_TAG));
		assert!(out.contains("kept 7 of 10 original lines"));
	})
	.await;
}

#[tokio::test]
async fn extract_retains_a_truncation_notice_and_its_spill_pointer() {
	crate::session::context::with_session_id("condense-unit-extract-notice".to_string(), async {
		let notice = format!(
			"{}: showing only the first tokens",
			crate::utils::truncation::TRUNCATION_NOTICE_TAG
		);
		let lines = [
			"l1".to_string(),
			"l2".to_string(),
			"l3".to_string(),
			"l4".to_string(),
			"l5".to_string(),
			notice,
			"  /tmp/octomind-spill/earlier.txt".to_string(),
			"l8".to_string(),
		];
		let ok = McpToolResult::success("shell".into(), "t1".into(), lines.join("\n"));
		let original = ok.extract_content();
		let out = apply_verdict(&entry("extract", &["1-2"]), &ok, &original, &full_view(8))
			.expect("a partial selection applies");
		assert!(
			out.contains(crate::utils::truncation::TRUNCATION_NOTICE_TAG),
			"the notice naming the earlier spill is never cut away"
		);
		assert!(out.contains("/tmp/octomind-spill/earlier.txt"));
		assert!(out.contains("kept 5 of 8 original lines"));
	})
	.await;
}

#[tokio::test]
async fn replace_on_a_full_success_view_writes_the_factual_notice() {
	crate::session::context::with_session_id("condense-unit-replace".to_string(), async {
		let ok = McpToolResult::success("shell".into(), "t1".into(), "a\nb\nc\nd\ne\nf".into());
		let original = ok.extract_content();
		let out = apply_verdict(&entry("replace", &[]), &ok, &original, &full_view(6))
			.expect("replace applies");
		assert!(out.starts_with(CONDENSE_NOTICE_TAG));
		assert!(out.contains("6-line"));
		assert!(out.contains("`shell`"));
		assert!(
			out.contains("octomind-spill"),
			"the spill path is named for recovery"
		);
		assert!(out.contains("not merely to recover omitted text"));
	})
	.await;
}

// ---------------------------------------------------------------------------
// build_request
// ---------------------------------------------------------------------------

#[test]
fn build_request_orders_candidates_and_shapes_the_payload() {
	let big = McpToolResult::success(
		"shell".into(),
		"t0".into(),
		(0..3_000)
			.map(|i| format!("line {i}"))
			.collect::<Vec<_>>()
			.join("\n"),
	);
	let failing = McpToolResult::error(
		"view".into(),
		"t1".into(),
		(0..200)
			.map(|i| format!("row {i}"))
			.collect::<Vec<_>>()
			.join("\n"),
	);
	let small = McpToolResult::success("grep".into(), "t2".into(), "one\nmatch".into());
	let results = vec![big, failing, small];
	let calls = vec![
		McpToolCall {
			tool_name: "shell".into(),
			parameters: serde_json::json!({"command": "ls"}),
			tool_id: "t0".into(),
		},
		McpToolCall {
			tool_name: "view".into(),
			parameters: serde_json::json!({"path": "src/main.rs"}),
			tool_id: "t1".into(),
		},
		// t2 has no matching call: its arguments default to empty.
	];
	let sizes: Vec<usize> = results
		.iter()
		.map(|r| estimate_tokens(&r.extract_content()))
		.collect();
	let sizable = vec![0, 1, 2];
	let (candidates, user) = build_request(&results, &calls, &sizable, &sizes, "", "", "");

	assert_eq!(
		candidates
			.iter()
			.map(|c| c.result_index)
			.collect::<Vec<_>>(),
		vec![0, 1, 2],
		"candidates come back in result order"
	);
	assert_eq!(candidates[0].view.total_lines, 3_000);

	let payload: serde_json::Value = serde_json::from_str(&user).expect("payload is valid JSON");
	assert_eq!(payload["results_considered"], 3);
	assert_eq!(
		payload["candidate_output_tokens"],
		sizes.iter().sum::<usize>() as u64
	);
	assert!(
		payload["task_context"]
			.as_str()
			.is_some_and(|t| t.contains("task context unavailable")),
		"an empty task degrades to the conservative placeholder"
	);
	assert_eq!(payload["results"][0]["id"], "t0");
	assert_eq!(payload["results"][0]["tool"], "shell");
	assert_eq!(payload["results"][0]["status"], "ok");
	assert_eq!(payload["results"][0]["arguments"], r#"{"command":"ls"}"#);
	assert_eq!(payload["results"][1]["status"], "error");
	assert_eq!(payload["results"][2]["arguments"], "");
	assert!(payload["results"][0]["numbered_output"]
		.as_str()
		.is_some_and(|b| !b.is_empty()));
}

#[test]
fn build_request_truncates_to_the_safe_batch_size_biggest_first() {
	let results: Vec<McpToolResult> = (0..40)
		.map(|i| {
			McpToolResult::success(
				"shell".into(),
				format!("t{i}"),
				format!("filler output number {:02} padding", i),
			)
		})
		.collect();
	let sizes: Vec<usize> = results
		.iter()
		.map(|r| estimate_tokens(&r.extract_content()))
		.collect();
	let sizable: Vec<usize> = (0..40).collect();
	let (candidates, user) = build_request(&results, &[], &sizable, &sizes, "task", "", "");

	assert_eq!(candidates.len(), MAX_RESULTS_PER_REQUEST);
	let indices: Vec<usize> = candidates.iter().map(|c| c.result_index).collect();
	assert_eq!(indices.first(), Some(&0));
	assert_eq!(indices.last(), Some(&(MAX_RESULTS_PER_REQUEST - 1)));
	let payload: serde_json::Value = serde_json::from_str(&user).unwrap();
	assert_eq!(payload["results_considered"], MAX_RESULTS_PER_REQUEST);
}

// ---------------------------------------------------------------------------
// truncate_preserving_edges / suffix_to_tokens / parse_response
// ---------------------------------------------------------------------------

#[test]
fn tiny_budgets_fall_back_to_plain_prefix_truncation() {
	let text = "word ".repeat(1_000);
	assert_eq!(truncate_preserving_edges(&text, 0), "");
	assert_eq!(truncate_preserving_edges("", 100), "");

	// A budget that cannot fit head + marker + tail degrades to a plain
	// prefix instead of emitting a marker with nothing around it.
	let out = truncate_preserving_edges(&text, 3);
	assert!(!out.contains("middle omitted"));
	assert_eq!(out, truncate_to_tokens(&text, 3));
	assert!(estimate_tokens(&out) <= 3);
}

#[test]
fn suffix_to_tokens_returns_whole_text_under_budget_and_a_suffix_over() {
	let text = "HEAD middle TAIL";
	assert_eq!(suffix_to_tokens(text, 0), "");
	assert_eq!(suffix_to_tokens(text, 10_000), text);

	let long = "token ".repeat(500);
	let suffix = suffix_to_tokens(&long, 5);
	assert!(!suffix.is_empty());
	assert!(
		long.ends_with(suffix),
		"the suffix is a real tail of the text"
	);
	assert!(estimate_tokens(suffix) <= 5);
}

#[test]
fn response_parse_rejects_reversed_braces_unterminated_fences_and_bad_json() {
	assert!(
		parse_response("}{").is_none(),
		"closing before opening is not JSON"
	);
	assert!(
		parse_response("```json\n{\"results\":[]}").is_none(),
		"an unterminated fence has no extractable block"
	);
	assert!(parse_response("{not json}").is_none());
}

// ---------------------------------------------------------------------------
// Adaptive round observation with the mechanic switched off.
// ---------------------------------------------------------------------------

#[test]
fn observe_and_relax_return_the_configured_baseline_when_adaptive_is_off() {
	let fixed = config(false, 5_000, "ollama:m");
	assert_eq!(observe_adaptive_round(&fixed, 10_000, 9_000), 5_000);
	assert_eq!(relax_adaptive_threshold(&fixed), 5_000);
}

// ---------------------------------------------------------------------------
// build_numbered_view: sampling edges.
// ---------------------------------------------------------------------------

#[test]
fn diagnostics_on_the_first_and_last_lines_survive_a_sampled_view() {
	let mut lines: Vec<String> = (1..=300)
		.map(|i| format!("plain filler line {i}"))
		.collect();
	lines[0] = "error: boom at startup".to_string();
	lines[299] = "error: deprecation at the end".to_string();
	let content = lines.join("\n");

	let view = build_numbered_view(&content, 250, "");

	assert!(
		view.partial,
		"300 lines under a 250-token budget must sample"
	);
	assert!(
		view.body.contains("error: boom at startup"),
		"a diagnostic on line 1 is queued with its context window"
	);
	assert!(
		view.body.contains("error: deprecation at the end"),
		"a diagnostic on the last line is queued with its context window"
	);
	assert!(
		view.body.contains("plain filler line 298"),
		"the diagnostic carries its context window"
	);
}

#[test]
fn a_view_over_budget_drops_lines_until_the_rendered_body_fits() {
	let content = (1..=120)
		.map(|i| format!("filler line {i} of the oversized result"))
		.collect::<Vec<_>>()
		.join("\n");

	let view = build_numbered_view(&content, 60, "");

	assert!(view.partial);
	assert!(
		estimate_tokens(&view.body) <= 60,
		"the rendered body itself must respect the budget: {}",
		estimate_tokens(&view.body)
	);
	assert!(
		view.visible_ranges.len() < 120,
		"lines were dropped to fit, not merely clipped mid-record"
	);
}

#[test]
fn a_huge_single_line_shrinks_its_preview_until_the_record_fits() {
	let line: String = "x".repeat(400);
	let view = build_numbered_view(&line, 8, "");

	assert!(!view.partial, "one line of one is still the whole result");
	assert!(
		view.body.starts_with('1'),
		"the record keeps its original line number"
	);
	assert!(
		view.body.chars().count() < line.chars().count(),
		"the preview is clipped rather than the record dropped"
	);
	assert!(
		view.body.contains("line preview clipped"),
		"the record is clipped in place, never dropped: {}",
		view.body
	);
}

// ---------------------------------------------------------------------------
// render_numbered_selection: gap markers at both ends.
// ---------------------------------------------------------------------------

#[test]
fn selection_markers_name_every_omitted_span_at_both_edges() {
	let lines: Vec<&str> = vec!["one", "two", "three", "four", "five", "six"];

	let leading = render_numbered_selection(&lines, &[5], 6, 64);
	assert!(
		leading.starts_with("[… original lines 1-5 not shown in this view …]"),
		"a selection starting mid-result announces the head it dropped: {leading}"
	);
	assert!(leading.contains("6| six"));

	let trailing = render_numbered_selection(&lines, &[0], 6, 64);
	assert!(trailing.contains("1| one"));
	assert!(
		trailing.ends_with("[… original lines 2-6 not shown in this view …]"),
		"a selection ending early announces the tail it dropped: {trailing}"
	);

	let middle = render_numbered_selection(&lines, &[1, 4], 6, 64);
	assert!(middle.contains("[… original lines 3-4 not shown in this view …]"));
}

// ---------------------------------------------------------------------------
// truncate_preserving_edges: the tail-shrink loop.
// ---------------------------------------------------------------------------

#[test]
fn edge_truncation_shrinks_the_tail_until_the_combined_view_fits() {
	let text: String = (1..=120)
		.map(|i| format!("token-heavy line {i} with padding words"))
		.collect::<Vec<_>>()
		.join("\n");
	assert!(estimate_tokens(&text) > 60);

	for max_tokens in 14..=40 {
		let out = truncate_preserving_edges(&text, max_tokens);
		assert!(
			out.contains("[… middle omitted for condenser budget …]"),
			"both ends are kept behind one explicit marker at budget {max_tokens}"
		);
		assert!(
			estimate_tokens(&out) <= max_tokens,
			"budget {max_tokens} exceeded: {}",
			estimate_tokens(&out)
		);
		assert!(
			out.starts_with("token"),
			"the head survives at budget {max_tokens}"
		);
	}
	// At a budget where the tail allowance spans whole lines the final line
	// survives; at tiny budgets the tail shrinks to a couple of tokens and
	// cannot be expected to carry it.
	let roomy = truncate_preserving_edges(&text, 40);
	assert!(roomy.contains("line 120"), "the tail survives: {roomy}");
}
