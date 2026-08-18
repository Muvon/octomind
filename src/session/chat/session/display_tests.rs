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

//! Rendering tests for the /info and /context surfaces. The structured
//! accessors are asserted on content; the print paths are exercised over a
//! populated session so layout code runs against realistic data.

use super::*;

fn test_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn populated_session() -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	let config = test_config();
	session
		.add_user_message("please fix the parser")
		.expect("user");
	session
		.add_assistant_message("Looking into it now.", None, &config, "assistant")
		.expect("assistant");
	session
		.add_tool_message("file contents here", "call_1", "view", &config)
		.expect("tool");
	session.session.info.total_cost = 0.42;
	session.session.info.input_tokens = 1000;
	session.session.info.output_tokens = 500;
	session.session.info.total_api_calls = 3;
	session
}

#[test]
fn test_session_info_json_structure() {
	let session = populated_session();
	let info = session.get_session_info_json();

	assert_eq!(info["session_name"], "test");
	assert_eq!(info["model"], "anthropic/claude-3-5-sonnet");
	assert!(info["tokens"].is_object(), "info: {info}");
	assert_eq!(info["tokens"]["input"], 1000);
	assert_eq!(info["tokens"]["output"], 500);
}

#[test]
fn test_session_info_string_contains_core_facts() {
	let session = populated_session();
	let text = session.get_session_info_string();
	assert!(text.contains("test"), "info string: {text}");
	assert!(
		text.contains("anthropic/claude-3-5-sonnet"),
		"info string: {text}"
	);
}

fn layered_session() -> ChatSession {
	let mut session = populated_session();
	session
		.session
		.info
		.layer_stats
		.push(crate::session::LayerStats {
			layer_type: "reduce".to_string(),
			model: "ollama:fake".to_string(),
			input_tokens: 500,
			output_tokens: 100,
			cost: 0.01,
			timestamp: 1_700_000_000,
			api_time_ms: 1200,
			tool_time_ms: 300,
			total_time_ms: 1500,
		});
	session
		.session
		.info
		.layer_stats
		.push(crate::session::LayerStats {
			layer_type: "reduce".to_string(),
			model: "ollama:fake".to_string(),
			input_tokens: 200,
			output_tokens: 50,
			cost: 0.002,
			timestamp: 1_700_000_100,
			api_time_ms: 400,
			tool_time_ms: 0,
			total_time_ms: 400,
		});
	session
}

#[test]
fn test_layer_stats_render_in_info() {
	let session = layered_session();
	// JSON groups executions per layer type
	let info = session.get_session_info_json();
	let text = info.to_string();
	assert!(text.contains("reduce"), "info json: {text}");

	// The print path renders the layer table without panicking
	session.display_session_info();
	// The string form carries the layer section too
	assert!(session.get_session_info_string().contains("reduce"));
}

#[test]
fn test_display_session_info_smoke() {
	// Print path over both a populated and an empty session — layout code
	// must handle zero-message sessions without panicking.
	populated_session().display_session_info();
	ChatSession::for_tests(Vec::new()).display_session_info();
}

#[tokio::test]
async fn test_display_session_context_all_filters() {
	let config = test_config();
	let mut session = populated_session();
	for filter in ["all", "user", "assistant", "tool", "large"] {
		session
			.display_session_context_filtered(&config, filter)
			.await;
	}
	// Empty session renders too
	let mut empty = ChatSession::for_tests(Vec::new());
	empty.display_session_context(&config).await;
}
