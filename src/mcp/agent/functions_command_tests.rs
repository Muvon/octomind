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

//! Validation-arm tests for the `agent_*` builtin tool dispatcher. Nothing
//! here spawns an agent — only the parameter/lookup failures that must come
//! back as structured tool errors, never as process work.

use super::*;

fn agent_call(tool_name: &str, params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: tool_name.to_string(),
		parameters: params,
		tool_id: "t-agent".to_string(),
	}
}

fn test_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
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
async fn test_agent_tool_validation_arms() {
	let config = test_config();

	// Tool name without the agent_ prefix
	let result = execute_agent_command(
		&agent_call("not_an_agent_tool", serde_json::json!({"task": "x"})),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Invalid agent tool name"));

	// Missing / empty task
	for params in [serde_json::json!({}), serde_json::json!({"task": "   "})] {
		let result = execute_agent_command(&agent_call("agent_developer", params), &config, None)
			.await
			.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("task"));
	}

	// Agent that exists neither in config nor as a dynamic agent
	let result = execute_agent_command(
		&agent_call(
			"agent___functest_nonexistent",
			serde_json::json!({"task": "do it"}),
		),
		&config,
		None,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("not configured"));
}
