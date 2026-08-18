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

fn tap_call(action: &str) -> crate::mcp::McpToolCall {
	crate::mcp::McpToolCall {
		tool_name: "tap".to_string(),
		parameters: serde_json::json!({ "action": action }),
		tool_id: "id".to_string(),
	}
}

#[test]
fn test_is_tap_capability_call() {
	assert!(is_tap_capability_call(&tap_call("capability")));
	assert!(!is_tap_capability_call(&tap_call("run")));

	let other = crate::mcp::McpToolCall {
		tool_name: "shell".to_string(),
		parameters: serde_json::json!({ "action": "capability" }),
		tool_id: "id".to_string(),
	};
	assert!(!is_tap_capability_call(&other));
}

#[test]
fn test_error_messages() {
	let loop_msg = loop_error_message("shell", 3, "exit code 1");
	assert!(loop_msg.contains("LOOP DETECTED"));
	assert!(loop_msg.contains("'shell'"));
	assert!(loop_msg.contains("3 consecutive"));
	assert!(loop_msg.contains("exit code 1"));

	let attempt_msg = attempt_error_message(2, 3, "no such file");
	assert!(attempt_msg.contains("attempt 2/3"));
	assert!(attempt_msg.contains("no such file"));
}

/// The invariant handle_large_tool_results must hold: truncation may replace
/// the body but must NEVER flip is_error() — a truncated error entering the
/// dedup cache as "success" would get elided exactly when the model needs
/// the error text most.
#[tokio::test]
async fn test_truncation_preserves_error_flag() {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.mcp_response_tokens_threshold = 10;

	let big = "line of output\n".repeat(500);
	let results = vec![
		crate::mcp::McpToolResult::success("shell".to_string(), "id1".to_string(), big.clone()),
		crate::mcp::McpToolResult::error("shell".to_string(), "id2".to_string(), big.clone()),
	];

	let processed = handle_large_tool_results(results, &config, OutputMode::NonInteractive)
		.await
		.expect("truncation never fails");

	assert_eq!(processed.len(), 2);
	// Both bodies were truncated below the original size
	assert!(processed[0].extract_content().len() < big.len());
	assert!(processed[1].extract_content().len() < big.len());
	// The error flag survives truncation
	assert!(!processed[0].is_error());
	assert!(processed[1].is_error());
}

/// Rich (non-plain-text) results pass through untouched — flattening them
/// would discard resource/image/structured-content semantics.
#[tokio::test]
async fn test_rich_results_bypass_truncation() {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.mcp_response_tokens_threshold = 10;

	let big = "line of output\n".repeat(500);
	let rich = crate::mcp::McpToolResult::success_with_metadata(
		"tool".to_string(),
		"id".to_string(),
		big.clone(),
		serde_json::json!({"k": "v"}),
	);

	let processed = handle_large_tool_results(vec![rich], &config, OutputMode::NonInteractive)
		.await
		.expect("passthrough never fails");
	assert!(processed[0].extract_content().contains(&big[..100]));
}
