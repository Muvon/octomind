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

//! Tests for the `capability` tool command surface: parameter validation,
//! unknown-name handling, and the idempotent disable path. Only the
//! deterministic arms — nothing here depends on which taps happen to be
//! installed on the machine (list is asserted as "answers", not contents).

use super::*;
use serial_test::serial;

fn cap_call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "capability".to_string(),
		parameters: params,
		tool_id: "t-cap".to_string(),
	}
}

fn test_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

fn text_of(result: &McpToolResult) -> String {
	result
		.result
		.content
		.iter()
		.filter_map(|block| match block {
			rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect()
}

fn is_err(result: &McpToolResult) -> bool {
	result.result.is_error.unwrap_or(false)
}

#[tokio::test]
#[serial]
async fn test_capability_action_validation() {
	let config = test_config();

	let result = execute_capability_command(&cap_call(serde_json::json!({})), &config)
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("action"));

	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "explode"})), &config)
			.await
			.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Unknown action"));
}

#[tokio::test]
#[serial]
async fn test_capability_enable_unknown_and_disable_idempotent() {
	let config = test_config();

	// enable without a name
	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "enable"})), &config)
			.await
			.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("name"));

	// enable a capability no tap provides
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "__captest_nonexistent"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("not found"));

	// disable of an inactive capability is an idempotent success
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "__captest_nonexistent"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
	assert!(text_of(&result).contains("not active"));
}

#[tokio::test]
#[serial]
async fn test_capability_list_answers() {
	let config = test_config();
	// Contents depend on installed taps — only the contract matters: the
	// command answers rather than erroring or hanging.
	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "list"})), &config)
			.await
			.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
	assert!(!text_of(&result).is_empty());
}

#[tokio::test]
#[serial]
async fn test_capability_discover_arms() {
	let config = test_config();

	// discover without intent → validation error
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "discover"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("intent"));

	// With an intent it must answer (match set depends on installed taps)
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "discover", "intent": "review some rust code"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
}
