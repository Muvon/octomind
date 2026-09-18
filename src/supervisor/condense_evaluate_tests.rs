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

//! The condense seam: chunking, the scored state, verdicts derived from
//! chunk probabilities, and the round-level fallback to the supervisor-model
//! condenser. Every provider interaction goes through the in-process fake;
//! `install_fake_evaluation` holds `ENV_LOCK`, so these tests never take it
//! themselves.

use super::*;
use crate::config::McpServerConfig;
use crate::mcp::{McpToolCall, McpToolResult};
use crate::session::chat::test_support::{
	evaluate_counter, fake_provider_config, final_response, install_fake_evaluation, nouls,
	spawn_stub, FakeEvaluationStep,
};
use crate::supervisor::evaluate::{Seam, CONDENSE_CHUNK_TOKENS};

fn config(condense: bool) -> Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.condense.enabled = true;
	config.supervisor.condense.tokens_threshold = 10;
	config.supervisor.evaluate.condense = condense;
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config
}

fn body(lines: usize) -> String {
	(1..=lines)
		.map(|i| format!("payload line number {i} with some filler text"))
		.collect::<Vec<_>>()
		.join("\n")
}

fn call(id: &str) -> McpToolCall {
	McpToolCall {
		tool_name: "shell".to_string(),
		parameters: serde_json::json!({"cmd": "cat big.txt"}),
		tool_id: id.to_string(),
	}
}

fn ok(id: &str, text: &str) -> McpToolResult {
	McpToolResult::success("shell".to_string(), id.to_string(), text.to_string())
}

fn candidate(index: usize, content: &str) -> Candidate {
	let total_lines = content.lines().count();
	Candidate {
		result_index: index,
		view: NumberedView {
			body: String::new(),
			visible_ranges: vec![(1, total_lines)],
			total_lines,
			partial: false,
		},
	}
}

/// Noul answers `k0..kn` with `probability(slot)`.
fn chunk_answers(count: usize, probability: impl Fn(usize) -> f64) -> FakeEvaluationStep {
	let pairs: Vec<(String, f64)> = (0..count)
		.map(|slot| {
			(
				crate::supervisor::evaluate::condense_question_id(slot),
				probability(slot),
			)
		})
		.collect();
	let borrowed: Vec<(&str, f64)> = pairs.iter().map(|(id, p)| (id.as_str(), *p)).collect();
	FakeEvaluationStep::Answers(nouls(&borrowed))
}

async fn score(config: &Config, results: &[McpToolResult], session: &str) -> Option<Vec<Outcome>> {
	let calls: Vec<McpToolCall> = results.iter().map(|r| call(&r.tool_id)).collect();
	let candidates: Vec<Candidate> = results
		.iter()
		.enumerate()
		.map(|(i, r)| candidate(i, &r.extract_content()))
		.collect();
	crate::session::context::with_session_id(session.to_string(), async {
		evaluate_candidates(
			config,
			results,
			&calls,
			&candidates,
			"inspect the payload",
			"reading big.txt",
		)
		.await
	})
	.await
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

#[test]
fn chunks_cover_every_line_exactly_once_within_the_budget() {
	let text = body(1_800);
	let lines: Vec<&str> = text.lines().collect();
	let chunks = chunk_lines(&text);
	assert!(chunks.len() > 1);
	assert_eq!(chunks[0].first_line, 1);
	assert_eq!(chunks.last().unwrap().last_line, 1_800);
	for (i, chunk) in chunks.iter().enumerate() {
		assert_eq!(chunk.index, i);
		if i > 0 {
			assert_eq!(chunk.first_line, chunks[i - 1].last_line + 1);
		}
		assert_eq!(
			chunk.text,
			lines[chunk.first_line - 1..chunk.last_line].join("\n")
		);
		if chunk.last_line > chunk.first_line {
			assert!(estimate_tokens(&chunk.text) <= CONDENSE_CHUNK_TOKENS);
		}
	}
}

#[test]
fn an_oversized_line_is_a_chunk_of_its_own() {
	let long = "word ".repeat(2_000);
	let text = format!("short\n{long}\nshort again");
	let chunks = chunk_lines(&text);
	assert_eq!(chunks.len(), 3);
	assert_eq!((chunks[1].first_line, chunks[1].last_line), (2, 2));
	assert_eq!(chunks[1].text, long);
	assert_eq!((chunks[2].first_line, chunks[2].last_line), (3, 3));
	assert!(chunk_lines("").is_empty());
}

// ---------------------------------------------------------------------------
// Scoring state and switches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn seam_off_makes_no_call() {
	let fake = install_fake_evaluation(vec![chunk_answers(64, |_| 0.9)]).await;
	let results = vec![ok("t1", &body(200))];
	assert!(score(&config(false), &results, "condense-eval-off")
		.await
		.is_none());
	assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn scored_state_carries_only_the_documented_fields_per_candidate() {
	let fake =
		install_fake_evaluation(vec![chunk_answers(64, |_| 0.9), chunk_answers(64, |_| 0.9)]).await;
	let results = vec![ok("t1", &body(120)), ok("t2", &body(80))];
	let outcomes = score(&config(true), &results, "condense-eval-state")
		.await
		.expect("both windows answered");
	assert_eq!(outcomes.len(), 2);
	let requests = fake.requests();
	assert_eq!(requests.len(), 2, "one window per candidate");
	for (request, result) in requests.iter().zip(&results) {
		let state = request.state.as_object().expect("object state");
		let mut keys: Vec<&str> = state.keys().map(String::as_str).collect();
		keys.sort_unstable();
		assert_eq!(
			keys,
			["arguments", "chunks", "intent", "status", "task", "tool"]
		);
		assert_eq!(state["tool"], "shell");
		assert_eq!(state["status"], "ok");
		assert_eq!(state["task"], "inspect the payload");
		assert_eq!(state["intent"], "reading big.txt");
		let chunks = state["chunks"].as_array().unwrap();
		assert_eq!(chunks.len(), request.questions.len());
		let rendered: String = chunks
			.iter()
			.map(|chunk| chunk["text"].as_str().unwrap())
			.collect::<Vec<_>>()
			.join("\n");
		assert_eq!(rendered, result.extract_content(), "full original, chunked");
		assert_eq!(request.max_retries, 0);
		assert_eq!(request.timeout, std::time::Duration::from_secs(5));
	}
	// No other result's text leaks into a candidate's state.
	let first = requests[0].state.to_string();
	assert!(!first.contains("payload line number 130"));
}

// ---------------------------------------------------------------------------
// Verdicts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kept_chunks_keep_their_neighbours_and_diagnostics_verbatim() {
	let mut lines: Vec<String> = body(400).lines().map(str::to_string).collect();
	let last = lines.len();
	lines[last - 1] = "error: boom at the very end".to_string();
	let text = lines.join("\n");
	let chunks = chunk_lines(&text);
	assert!(chunks.len() >= 6, "fixture needs several chunks");
	let _fake = install_fake_evaluation(vec![chunk_answers(chunks.len(), |slot| {
		if slot == 1 {
			0.9
		} else {
			0.1
		}
	})])
	.await;
	let results = vec![ok("t1", &text)];
	let outcomes = score(&config(true), &results, "condense-eval-neighbours")
		.await
		.expect("scored");
	let out = outcomes[0]
		.content
		.as_deref()
		.expect("a partial selection applies");
	assert_eq!(outcomes[0].verdict, "extract");
	// Chunks 0..=2 survive verbatim: the hit and one neighbour on each side.
	for chunk in &chunks[..3] {
		for line in chunk.text.lines() {
			assert!(out.contains(line), "kept line missing: {line}");
		}
	}
	// Chunk 3 is beyond the neighbour rule and far from the diagnostic tail.
	assert!(!out.contains(chunks[3].text.lines().next().unwrap()));
	assert!(out.contains("error: boom at the very end"));
	assert!(out.contains(CONDENSE_NOTICE_TAG));
	let kept = chunks[..3]
		.iter()
		.map(|c| c.last_line - c.first_line + 1)
		.sum::<usize>()
		+ 3; // the diagnostic line and its two lines of context
	assert!(
		out.contains(&format!("kept {kept} of {last} original lines")),
		"{out}"
	);
	assert!(out.contains("Full original output:"));
}

#[tokio::test]
async fn all_chunks_relevant_leaves_the_result_untouched() {
	let _fake = install_fake_evaluation(vec![chunk_answers(64, |_| 0.5)]).await;
	let results = vec![ok("t1", &body(150))];
	let outcomes = score(&config(true), &results, "condense-eval-keep")
		.await
		.expect("scored");
	assert!(outcomes[0].content.is_none());
	assert_eq!(outcomes[0].verdict, "keep");
}

#[tokio::test]
async fn nothing_relevant_omits_an_ok_result_and_never_an_error_result() {
	let _fake =
		install_fake_evaluation(vec![chunk_answers(64, |_| 0.1), chunk_answers(64, |_| 0.1)]).await;
	let text = body(150);
	let results = vec![
		ok("t1", &text),
		McpToolResult::error("shell".to_string(), "t2".to_string(), text.clone()),
	];
	let outcomes = score(&config(true), &results, "condense-eval-omit")
		.await
		.expect("scored");
	let notice = outcomes[0].content.as_deref().expect("ok result omitted");
	assert!(notice.starts_with(CONDENSE_NOTICE_TAG));
	assert!(notice.contains("omitted the complete 150-line successful `shell` result"));
	assert_eq!(outcomes[0].verdict, "omit");
	assert!(
		outcomes[1].content.is_none(),
		"an error result is never omitted"
	);
	assert_eq!(outcomes[1].verdict, "omit");
}

#[tokio::test]
async fn spill_failure_and_no_gain_leave_the_result_untouched() {
	// No session context → no spill directory → the selection is refused.
	let _fake = install_fake_evaluation(vec![chunk_answers(
		64,
		|slot| {
			if slot == 0 {
				0.9
			} else {
				0.1
			}
		},
	)])
	.await;
	let results = vec![ok("t1", &body(150))];
	let calls = vec![call("t1")];
	let candidates = vec![candidate(0, &results[0].extract_content())];
	let outcomes = evaluate_candidates(&config(true), &results, &calls, &candidates, "inspect", "")
		.await
		.expect("scored");
	assert!(outcomes[0].content.is_none());

	// A replacement that is not smaller stays unapplied by the shared finish.
	let mut results = vec![ok("t1", "short")];
	let cfg = config(true).supervisor.condense;
	let changed = finish_round(
		&mut results,
		&cfg,
		512,
		1,
		vec![Outcome {
			result_index: 0,
			content: Some("a much longer replacement than the original".to_string()),
			verdict: "extract".to_string(),
		}],
	);
	assert_eq!(changed, 0);
	assert_eq!(results[0].extract_content(), "short");
}

// ---------------------------------------------------------------------------
// Round-level fallback and accounting
// ---------------------------------------------------------------------------

fn round_results() -> Vec<McpToolResult> {
	vec![ok("t1", &body(200)), ok("t2", &body(200))]
}

async fn run_round(config: &Config, results: &mut [McpToolResult], session: &str) {
	let calls: Vec<McpToolCall> = results.iter().map(|r| call(&r.tool_id)).collect();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	crate::session::context::with_session_id(
		session.to_string(),
		condense_round(
			results,
			&calls,
			config,
			"inspect the payload",
			"agent context",
			"reading big.txt",
			rx,
		),
	)
	.await;
}

/// Run `body` under a session that owns an isolated tool map carrying a spill
/// reader, registered the way runtime `mcp add` does; the core builtin exposes
/// no file-reading tool. The process map, which other tests initialize
/// concurrently, is never consulted.
async fn with_spill_reader<F: std::future::Future<Output = ()>>(
	config: &Config,
	session: &str,
	body: F,
) {
	crate::mcp::tool_map::isolate_session_tool_map(session);
	crate::session::context::with_session_id(session.to_string(), async {
		let mut empty = config.clone();
		empty.mcp.servers = Vec::new();
		crate::mcp::tool_map::initialize_tool_map(&empty)
			.await
			.expect("tool map initializes empty");
		crate::mcp::tool_map::register_dynamic_server_tools(
			"spill-reader",
			&McpServerConfig::builtin("spill-reader", 30, Vec::new()),
			&["view".to_string()],
		);
		body.await;
	})
	.await;
	crate::session::context::cleanup_session(&session.to_string());
}

#[tokio::test]
#[serial_test::serial]
async fn a_scored_round_condenses_and_counts_applied() {
	let fake = install_fake_evaluation(vec![
		chunk_answers(64, |slot| if slot == 0 { 0.9 } else { 0.1 }),
		chunk_answers(64, |_| 0.1),
	])
	.await;
	let calls_before = evaluate_counter(Seam::Condense, "calls");
	let applied_before = evaluate_counter(Seam::Condense, "applied");
	let config = config(true);
	let mut results = round_results();
	with_spill_reader(
		&config,
		"__condense_eval_round",
		run_round(&config, &mut results, "__condense_eval_round"),
	)
	.await;
	assert_eq!(fake.requests().len(), 2);
	assert_eq!(evaluate_counter(Seam::Condense, "calls") - calls_before, 2);
	assert_eq!(
		evaluate_counter(Seam::Condense, "applied") - applied_before,
		2
	);
	let first = results[0].extract_content();
	assert!(first.contains("payload line number 1 with"));
	assert!(!first.contains("payload line number 199 with"));
	assert!(first.contains("kept"));
	assert!(results[1]
		.extract_content()
		.starts_with(CONDENSE_NOTICE_TAG));
}

#[tokio::test]
#[serial_test::serial]
async fn a_failed_window_falls_back_to_one_model_condenser_call() {
	let fake = install_fake_evaluation(vec![
		chunk_answers(64, |_| 0.1),
		FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY"),
	])
	.await;
	let unavailable_before = evaluate_counter(Seam::Condense, "unavailable");
	let applied_before = evaluate_counter(Seam::Condense, "applied");
	let config = config(true);
	let mut results = round_results();
	let url = spawn_stub(vec![final_response(
		r#"{"results":[{"id":"t1","verdict":"extract","lines":["10-20"]},{"id":"t2","verdict":"keep"}]}"#,
	)])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	with_spill_reader(
		&config,
		"__condense_eval_fallback",
		run_round(&config, &mut results, "__condense_eval_fallback"),
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");

	assert_eq!(fake.requests().len(), 2, "both windows were issued");
	assert_eq!(
		evaluate_counter(Seam::Condense, "unavailable") - unavailable_before,
		1
	);
	assert_eq!(evaluate_counter(Seam::Condense, "applied"), applied_before);
	// The model verdict, not the first window's answers, shaped the round.
	let first = results[0].extract_content();
	assert!(first.contains("kept 11 of 200 original lines"), "{first}");
	assert!(!results[1].extract_content().contains(CONDENSE_NOTICE_TAG));
}

#[tokio::test]
async fn adaptive_controller_observes_the_same_numbers_as_the_model_path() {
	let mut cfg = config(true).supervisor.condense;
	cfg.adaptive = true;
	cfg.tokens_threshold = 5_000;
	let session = "condense-eval-adaptive".to_string();
	crate::session::context::with_session_id(session.clone(), async {
		let mut results = vec![ok("t1", &body(300))];
		let attempted = estimate_tokens(&results[0].extract_content()) as u64;
		let changed = finish_round(
			&mut results,
			&cfg,
			5_000,
			attempted,
			vec![Outcome {
				result_index: 0,
				content: Some("payload line number 1 with some filler text".to_string()),
				verdict: "extract".to_string(),
			}],
		);
		assert_eq!(changed, 1);
		let saved = attempted - estimate_tokens(&results[0].extract_content()) as u64;
		let mut expected = AdaptiveThresholdState::new(5_000);
		expected.observe(attempted, saved);
		assert_eq!(adaptive_threshold(&cfg), expected.threshold());
	})
	.await;
	clear_for_session(&session);
}
