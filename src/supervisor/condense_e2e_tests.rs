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

//! Gate tests for round condensation. The full verdict round trip needs an
//! enabled local file-reading tool in the global tool map (spill-recovery
//! precondition), which a unit process never has — so what IS testable here
//! is exactly the gates: every early-return must leave the round untouched.
//! Verdict application itself is covered by the inline unit tests in
//! `condense.rs`.

use super::*;
use crate::mcp::{McpToolCall, McpToolResult};
use crate::session::chat::test_support::fake_provider_config;

fn tool_call(id: &str) -> McpToolCall {
	McpToolCall {
		tool_name: "shell".to_string(),
		parameters: serde_json::json!({"cmd": "cat big.txt"}),
		tool_id: id.to_string(),
	}
}

fn tool_result(id: &str, text: &str) -> McpToolResult {
	McpToolResult::success("shell".to_string(), id.to_string(), text.to_string())
}

fn condense_config() -> crate::config::Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.condense.enabled = true;
	config.supervisor.condense.tokens_threshold = 10;
	config.supervisor.condense.model = "ollama:fake-model".to_string();
	config
}

fn big_body() -> String {
	(1..=200)
		.map(|i| format!("payload line number {i} with some filler text"))
		.collect::<Vec<_>>()
		.join("\n")
}

async fn run_round(config: &crate::config::Config, results: &mut [McpToolResult]) {
	let calls: Vec<McpToolCall> = results.iter().map(|r| tool_call(&r.tool_id)).collect();
	let (_tx, rx) = tokio::sync::watch::channel(false);
	condense_round(
		results,
		&calls,
		config,
		"inspect the payload",
		"agent context",
		"reading big.txt",
		rx,
	)
	.await;
}

/// Without an enabled local file-reading tool, condensation must decline the
/// whole round untouched — narrowing away content that could never be
/// re-read would lose it. (This is the gate every unit-test process hits.)
#[tokio::test]
#[serial_test::serial]
async fn test_condense_declines_without_spill_reader() {
	let config = condense_config();
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();
	run_round(&config, &mut results).await;
	assert_eq!(results[0].extract_content(), before);
}

/// A round under the token threshold returns before any other gate.
#[tokio::test]
#[serial_test::serial]
async fn test_condense_below_threshold_is_a_noop() {
	let config = condense_config();
	let mut results = vec![tool_result("t1", "tiny")];
	let before = results[0].extract_content();
	run_round(&config, &mut results).await;
	assert_eq!(results[0].extract_content(), before);
}

/// The trigger is per result, not per round: many modest outputs that only add
/// up to something large are left exactly as returned.
#[tokio::test]
#[serial_test::serial]
async fn test_condense_ignores_small_results_in_a_large_round() {
	let config = condense_config();
	let body = (1..=20)
		.map(|i| format!("short line {i}"))
		.collect::<Vec<_>>()
		.join("\n");
	let mut results: Vec<McpToolResult> = (0..20)
		.map(|i| tool_result(&format!("t{i}"), &body))
		.collect();
	let before: Vec<String> = results.iter().map(|r| r.extract_content()).collect();
	run_round(&config, &mut results).await;
	for (result, original) in results.iter().zip(before) {
		assert_eq!(result.extract_content(), original);
	}
}

/// Supervisor disabled: the very first gate — even an oversized round stays
/// untouched.
#[tokio::test]
#[serial_test::serial]
async fn test_condense_disabled_supervisor_is_a_noop() {
	let mut config = condense_config();
	config.supervisor.enabled = false;
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();
	run_round(&config, &mut results).await;
	assert_eq!(results[0].extract_content(), before);
}

// ---------------------------------------------------------------------------
// Full rounds: the tool map carries a spill reader and the condenser model is
// the scripted fake provider, so the whole narrow→apply→notify path runs.
// ---------------------------------------------------------------------------

use crate::config::McpServerConfig;
use crate::mcp::tool_map::initialize_tool_map;
use crate::session::chat::test_support::{
	final_response, spawn_stub, spawn_stub_with_status, ENV_LOCK,
};

/// Register a spill reader the way runtime `mcp add` does: a dynamic server
/// contributing a `view` tool to the global tool map. The core builtin exposes
/// no file-reading tool, so `spill_reader_available()` keys on exactly this
/// registration path.
async fn enable_spill_reader() {
	let mut config = condense_config();
	config.mcp.servers = Vec::new();
	initialize_tool_map(&config)
		.await
		.expect("tool map initializes empty");
	crate::mcp::tool_map::register_dynamic_server_tools(
		"spill-reader",
		&McpServerConfig::builtin("spill-reader", 30, Vec::new()),
		&["view".to_string()],
	);
}

/// Drop the dynamic registration so tests asserting the no-spill-reader
/// decline keep their meaning after this file runs. Re-initializing alone
/// would short-circuit on an unchanged config hash and leave `view` mapped.
async fn reset_tool_map() {
	crate::mcp::tool_map::unregister_dynamic_server_tools("spill-reader", &["view".to_string()]);
}
fn verdict_json(body: &str) -> String {
	format!(r#"{{"results":{body}}}"#)
}

#[tokio::test]
#[serial_test::serial]
async fn a_full_round_extracts_selected_lines_and_skips_rich_results() {
	let _guard = ENV_LOCK.lock().await;
	enable_spill_reader().await;
	let mut config = condense_config();
	config.supervisor.condense.adaptive = true;

	let mut plain = tool_result("t1", &big_body());
	let mut rich = tool_result("t2", &big_body());
	rich.result.structured_content = Some(serde_json::json!({"rows": 7}));
	let rich_before = rich.extract_content();
	let mut results = vec![plain, rich];

	let url = spawn_stub(vec![final_response(&verdict_json(
		r#"[{"id":"t1","verdict":"extract","lines":["10-20"]}]"#,
	))])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	crate::session::context::with_session_id(
		"__condense_e2e_full".to_string(),
		run_round(&config, &mut results),
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");
	reset_tool_map().await;
	let _ = std::fs::remove_dir_all(
		std::env::temp_dir()
			.join("octomind-spill")
			.join("__condense_e2e_full"),
	);

	plain = results.remove(0);
	let condensed = plain.extract_content();
	assert!(
		condensed.contains(CONDENSE_NOTICE_TAG),
		"the notice names the spill"
	);
	assert!(
		condensed.contains("kept 11 of 200 original lines"),
		"the kept-line count is factual: {condensed}"
	);
	assert!(condensed.contains("payload line number 10"));
	assert!(!condensed.contains("payload line number 30"));
	assert_eq!(
		results[0].extract_content(),
		rich_before,
		"a structured result is never flattened into the text path"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn an_unparseable_condenser_answer_leaves_the_round_untouched() {
	let _guard = ENV_LOCK.lock().await;
	enable_spill_reader().await;
	let mut config = condense_config();
	config.supervisor.condense.adaptive = true;
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();

	let url = spawn_stub(vec![final_response("sorry, I cannot answer that")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	run_round(&config, &mut results).await;
	std::env::remove_var("OLLAMA_API_URL");
	reset_tool_map().await;

	assert_eq!(results[0].extract_content(), before);
}

#[tokio::test]
#[serial_test::serial]
async fn a_failed_condenser_call_leaves_results_as_is() {
	let _guard = ENV_LOCK.lock().await;
	enable_spill_reader().await;
	let config = condense_config();
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();

	let url = spawn_stub_with_status(vec![(
		500,
		serde_json::json!({"error": "condenser unavailable"}),
	)])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	run_round(&config, &mut results).await;
	std::env::remove_var("OLLAMA_API_URL");
	reset_tool_map().await;

	assert_eq!(results[0].extract_content(), before);
}

#[tokio::test]
#[serial_test::serial]
async fn a_missing_verdict_costs_only_the_result_it_omits() {
	let _guard = ENV_LOCK.lock().await;
	enable_spill_reader().await;
	let config = condense_config();
	let mut results = vec![
		tool_result("t1", &big_body()),
		tool_result("t2", &big_body()),
	];
	let t1_before = results[0].extract_content();

	let url = spawn_stub(vec![final_response(&verdict_json(
		r#"[{"id":"t2","verdict":"extract","lines":["5-8"]}]"#,
	))])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	crate::session::context::with_session_id(
		"__condense_e2e_missing".to_string(),
		run_round(&config, &mut results),
	)
	.await;
	std::env::remove_var("OLLAMA_API_URL");
	reset_tool_map().await;
	let _ = std::fs::remove_dir_all(
		std::env::temp_dir()
			.join("octomind-spill")
			.join("__condense_e2e_missing"),
	);

	assert_eq!(
		results[0].extract_content(),
		t1_before,
		"the result with no verdict stays inline in full"
	);
	assert!(results[1].extract_content().contains(CONDENSE_NOTICE_TAG));
}

#[tokio::test]
#[serial_test::serial]
async fn a_selection_that_cannot_shrink_the_result_is_left_inline() {
	let _guard = ENV_LOCK.lock().await;
	enable_spill_reader().await;
	let mut config = condense_config();
	config.supervisor.condense.adaptive = true;
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();

	// Keeping 199 of 200 lines: the spill notice costs more than one dropped
	// line, so the "condensed" form would be larger than the original.
	let url = spawn_stub(vec![final_response(&verdict_json(
		r#"[{"id":"t1","verdict":"extract","lines":["1-199"]}]"#,
	))])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);
	run_round(&config, &mut results).await;
	std::env::remove_var("OLLAMA_API_URL");
	reset_tool_map().await;

	assert_eq!(
		results[0].extract_content(),
		before,
		"a verdict with no token gain must not grow the result"
	);
}

/// Adaptive re-probe: when the raised threshold skips a result the baseline
/// would have condensed, the threshold relaxes so the next round sees it.
#[tokio::test]
#[serial_test::serial]
async fn skipped_baseline_candidates_relax_the_raised_threshold_for_a_reprobe() {
	let mut config = condense_config();
	config.supervisor.condense.adaptive = true;
	config.supervisor.condense.tokens_threshold = 600;

	let session = "condense-reprobe-session".to_string();
	crate::session::context::with_session_id(session.clone(), async {
		// Starve the saver: 0% realized savings drive the multiplier up.
		for _ in 0..5 {
			let _ = observe_adaptive_round(&config.supervisor.condense, 100_000, 0);
		}
		let raised = adaptive_threshold(&config.supervisor.condense);
		assert!(raised > 700, "the threshold must be raised first: {raised}");

		// A result the baseline (600) would condense but the raised one skips.
		let body: String = (1..=70)
			.map(|i| format!("reprobe filler line {i} with words"))
			.collect::<Vec<_>>()
			.join("\n");
		let tokens = estimate_tokens(&body);
		assert!(
			tokens > 600 && tokens <= raised,
			"the body must sit between baseline and raised threshold: {tokens} vs {raised}"
		);
		let mut results = vec![tool_result("t1", &body)];
		run_round(&config, &mut results).await;

		let after = adaptive_threshold(&config.supervisor.condense);
		assert!(
			after < raised,
			"the threshold must relax toward the baseline: {raised} → {after}"
		);
	})
	.await;
	clear_for_session(&session);
}
