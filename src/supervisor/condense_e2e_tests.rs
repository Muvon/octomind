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
async fn test_condense_declines_without_spill_reader() {
	let config = condense_config();
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();
	run_round(&config, &mut results).await;
	assert_eq!(results[0].extract_content(), before);
}

/// A round under the token threshold returns before any other gate.
#[tokio::test]
async fn test_condense_below_threshold_is_a_noop() {
	let config = condense_config();
	let mut results = vec![tool_result("t1", "tiny")];
	let before = results[0].extract_content();
	run_round(&config, &mut results).await;
	assert_eq!(results[0].extract_content(), before);
}

/// Supervisor disabled: the very first gate — even an oversized round stays
/// untouched.
#[tokio::test]
async fn test_condense_disabled_supervisor_is_a_noop() {
	let mut config = condense_config();
	config.supervisor.enabled = false;
	let mut results = vec![tool_result("t1", &big_body())];
	let before = results[0].extract_content();
	run_round(&config, &mut results).await;
	assert_eq!(results[0].extract_content(), before);
}
