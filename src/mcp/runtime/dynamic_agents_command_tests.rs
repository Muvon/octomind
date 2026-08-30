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

//! Tests for the `agent` management tool (`execute_agent_tool_command`):
//! validation arms and the add/enable/list/disable/remove lifecycle, plus a
//! full in-process dynamic-agent execution against the scripted fake
//! provider — the one path where a registered agent actually runs a turn.

use super::*;
use crate::session::chat::test_support::{
	fake_provider_config, final_response, spawn_stub, ENV_LOCK,
};
use serial_test::serial;

fn agent_call(params: serde_json::Value) -> crate::mcp::McpToolCall {
	crate::mcp::McpToolCall {
		tool_name: "agent".to_string(),
		parameters: params,
		tool_id: "t-dynagent".to_string(),
	}
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
async fn test_agent_tool_validation_arms() {
	clear_all();

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({})))
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("action"));

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({"action": "explode"})))
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Unknown action"));

	// add without name / without system
	let result = execute_agent_tool_command(&agent_call(serde_json::json!({"action": "add"})))
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("name"));

	let result = execute_agent_tool_command(&agent_call(
		serde_json::json!({"action": "add", "name": "x"}),
	))
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("system"));

	// lifecycle actions on an unknown agent
	for action in ["enable", "disable", "remove"] {
		let result = execute_agent_tool_command(&agent_call(
			serde_json::json!({"action": action, "name": "__dynagent_nope"}),
		))
		.await
		.unwrap_or_else(|e| panic!("{action} dispatch errored: {e}"));
		assert!(
			is_err(&result),
			"{action} on unknown agent must error, got: {}",
			text_of(&result)
		);
	}
}

#[tokio::test]
#[serial]
async fn test_agent_lifecycle_and_in_process_execution() {
	let _guard = ENV_LOCK.lock().await;
	clear_all();
	let name = "__dynagent_e2e";

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({
		"action": "add",
		"name": name,
		"description": "test agent",
		"system": "You are a test agent. Answer briefly.",
		"model": "ollama:fake-model"
	})))
	.await
	.expect("add dispatches");
	assert!(!is_err(&result), "add failed: {}", text_of(&result));
	assert!(is_dynamic(name));
	assert!(!is_enabled(name));

	let result = execute_agent_tool_command(&agent_call(
		serde_json::json!({"action": "enable", "name": name}),
	))
	.await
	.expect("enable dispatches");
	assert!(!is_err(&result), "enable failed: {}", text_of(&result));
	assert!(is_enabled(name));
	assert!(get_enabled_agent(name).is_some());
	assert!(is_dynamic_by_tool(&format!("agent_{name}")));

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({"action": "list"})))
		.await
		.expect("list dispatches");
	assert!(text_of(&result).contains(name));

	// The enabled agent runs a real in-process turn against the stub
	let url = spawn_stub(vec![final_response("DYNAGENT-ANSWER: done")]).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let config = fake_provider_config();
	let exec_call = crate::mcp::McpToolCall {
		tool_name: format!("agent_{name}"),
		parameters: serde_json::json!({"task": "say the answer"}),
		tool_id: "t-exec".to_string(),
	};
	let result = crate::mcp::agent::execute_agent_command(&exec_call, &config, None)
		.await
		.expect("agent executes");
	assert!(
		!is_err(&result),
		"dynamic agent run failed: {}",
		text_of(&result)
	);
	assert!(
		text_of(&result).contains("DYNAGENT-ANSWER"),
		"agent answer missing: {}",
		text_of(&result)
	);
	std::env::remove_var("OLLAMA_API_URL");

	// disable + remove tear it down
	let result = execute_agent_tool_command(&agent_call(
		serde_json::json!({"action": "disable", "name": name}),
	))
	.await
	.expect("disable dispatches");
	assert!(!is_err(&result), "disable failed: {}", text_of(&result));
	assert!(!is_enabled(name));

	let result = execute_agent_tool_command(&agent_call(
		serde_json::json!({"action": "remove", "name": name}),
	))
	.await
	.expect("remove dispatches");
	assert!(!is_err(&result), "remove failed: {}", text_of(&result));
	assert!(!is_dynamic(name));

	clear_all();
}
#[tokio::test]
#[serial]
async fn test_agent_list_empty_message() {
	clear_all();

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({"action": "list"})))
		.await
		.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
	assert!(
		text_of(&result).contains("No dynamic agents"),
		"got: {}",
		text_of(&result)
	);
}

#[tokio::test]
#[serial]
async fn test_agent_add_rejects_unknown_server_ref() {
	clear_all();

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({
		"action": "add",
		"name": "__dynagent_badref",
		"system": "You are a test.",
		"server_refs": ["__dynagent_no_such_srv"]
	})))
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(
		text_of(&result).contains("'__dynagent_no_such_srv' not found"),
		"got: {}",
		text_of(&result)
	);
	assert!(text_of(&result).contains("Available servers"));
	assert!(!is_dynamic("__dynagent_badref"));
}

#[tokio::test]
#[serial]
async fn test_agent_add_infers_server_refs_from_allowed_tools() {
	clear_all();

	// The template config's `runtime` builtin server is made visible via the
	// global tool map — the inference path resolves allowed_tools entries
	// through get_tool_server_name.
	let config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	crate::mcp::tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	let result = execute_agent_tool_command(&agent_call(serde_json::json!({
		"action": "add",
		"name": "__dynagent_infer",
		"system": "You are a test.",
		"allowed_tools": ["skill"]
	})))
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "add failed: {}", text_of(&result));

	let agents = list_agents();
	let added = agents
		.iter()
		.find(|(a, _)| a.name == "__dynagent_infer")
		.expect("agent registered");
	assert_eq!(added.0.server_refs, vec!["runtime".to_string()]);

	clear_all();
}
