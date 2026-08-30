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

//! Additional unit tests for `/info` rendering — complements `display_tests.rs`
//! by covering the compression, time, tool-call, and layer-stat paths that the
//! base suite (which mostly exercises an empty session) leaves unhit.

use super::*;

fn default_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

#[allow(clippy::too_many_arguments)]
fn layer(
	layer_type: &str,
	model: &str,
	input: u64,
	output: u64,
	cost: f64,
	api: u64,
	tool: u64,
	total: u64,
) -> crate::session::LayerStats {
	crate::session::LayerStats {
		layer_type: layer_type.to_string(),
		model: model.to_string(),
		input_tokens: input,
		output_tokens: output,
		cost,
		timestamp: 1_700_000_000,
		api_time_ms: api,
		tool_time_ms: tool,
		total_time_ms: total,
	}
}

// 2 task + 1 phase + 1 project + 1 conversation compressions,
// 42 messages removed, 10_000 tokens saved, 800/200 tokens, $0.05, 4s.
fn compression_fixture() -> crate::session::CompressionStats {
	let mut cs = crate::session::CompressionStats::default();
	cs.add_compression(crate::session::CompressionKind::Task, 10, 2_000);
	cs.add_compression(crate::session::CompressionKind::Task, 5, 1_000);
	cs.add_compression(crate::session::CompressionKind::Phase, 8, 1_500);
	cs.add_compression(crate::session::CompressionKind::Project, 12, 2_500);
	cs.add_compression(crate::session::CompressionKind::Conversation, 7, 3_000);
	cs.input_tokens = 800;
	cs.output_tokens = 200;
	cs.cost = 0.05;
	cs.api_time_ms = 4_000;
	cs
}

// ---------------------------------------------------------------------------
// get_session_info_json
// ---------------------------------------------------------------------------

#[test]
fn session_info_json_reports_compression_runs_by_kind() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.compression_stats = compression_fixture();
	let info = session.get_session_info_json();
	let runs = &info["compression"]["runs"];
	assert_eq!(runs["total"].as_u64(), Some(5));
	assert_eq!(runs["task"].as_u64(), Some(2));
	assert_eq!(runs["phase"].as_u64(), Some(1));
	assert_eq!(runs["project"].as_u64(), Some(1));
	assert_eq!(runs["conversation"].as_u64(), Some(1));
	assert_eq!(info["compression"]["messages_removed"].as_u64(), Some(42));
	assert_eq!(info["compression"]["tokens_saved"].as_u64(), Some(10_000));
	assert_eq!(info["compression"]["tokens"]["input"].as_u64(), Some(800));
	assert_eq!(info["compression"]["tokens"]["output"].as_u64(), Some(200));
	assert_eq!(info["compression"]["cost"].as_f64(), Some(0.05));
	assert_eq!(info["compression"]["api_time_ms"].as_u64(), Some(4_000));
}

#[test]
fn session_info_json_sums_time_tokens_and_reports_tool_calls() {
	let mut session = ChatSession::for_tests(Vec::new());
	{
		let info = &mut session.session.info;
		info.input_tokens = 1_000;
		info.output_tokens = 550;
		info.cache_read_tokens = 300;
		info.cache_write_tokens = 100;
		info.reasoning_tokens = 50;
		info.total_api_time_ms = 1_500;
		info.total_tool_time_ms = 2_500;
		info.total_layer_time_ms = 500;
		info.turn_timing.completed = 2;
		info.turn_timing.total_time_ms = 8_000;
		info.turn_timing.last_time_ms = 3_000;
		info.tool_calls = 9;
	}
	let json = session.get_session_info_json();
	assert_eq!(json["tokens"]["total"].as_u64(), Some(2_000));
	assert_eq!(json["tokens"]["reasoning"].as_u64(), Some(50));
	assert_eq!(json["time"]["total_ms"].as_u64(), Some(4_500));
	assert_eq!(json["time"]["api_ms"].as_u64(), Some(1_500));
	assert_eq!(json["time"]["tool_ms"].as_u64(), Some(2_500));
	assert_eq!(json["time"]["processing_ms"].as_u64(), Some(500));
	assert_eq!(json["timing"]["completed_turns"].as_u64(), Some(2));
	assert_eq!(json["timing"]["avg_turn_time_ms"].as_u64(), Some(4_000));
	assert_eq!(json["timing"]["last_turn_time_ms"].as_u64(), Some(3_000));
	assert_eq!(json["tool_calls"].as_u64(), Some(9));
	assert_eq!(json["messages"].as_u64(), Some(0));
}

#[test]
fn session_info_json_separates_command_layers_and_aggregates() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.layer_stats = vec![
		layer("reduce", "ollama:fake", 500, 100, 0.01, 1_200, 300, 1_500),
		layer("reduce", "ollama:fake", 200, 50, 0.005, 400, 0, 400),
		layer("command:review", "gpt:fake", 300, 80, 0.02, 900, 100, 1_000),
	];
	let info = session.get_session_info_json();
	let regular = info["layers"]["regular"].as_array().expect("regular array");
	let commands = info["layers"]["commands"]
		.as_array()
		.expect("commands array");
	assert_eq!(regular.len(), 1);
	assert_eq!(commands.len(), 1);

	let reduce = &regular[0];
	assert_eq!(reduce["layer_type"], "reduce");
	assert_eq!(reduce["model"], "ollama:fake");
	assert_eq!(reduce["executions"].as_u64(), Some(2));
	assert_eq!(reduce["tokens"]["input"].as_u64(), Some(700));
	assert_eq!(reduce["tokens"]["output"].as_u64(), Some(150));
	assert_eq!(reduce["time"]["api_ms"].as_u64(), Some(1_600));
	assert_eq!(reduce["time"]["tool_ms"].as_u64(), Some(300));
	assert_eq!(reduce["time"]["total_ms"].as_u64(), Some(1_900));
	let cost = reduce["cost"].as_f64().expect("cost");
	assert!((cost - 0.015).abs() < 1e-9, "aggregated cost: {cost}");

	let review = &commands[0];
	assert_eq!(review["layer_type"], "command:review");
	assert_eq!(review["executions"].as_u64(), Some(1));
	assert_eq!(review["model"], "gpt:fake");
}

// ---------------------------------------------------------------------------
// get_session_info_string
// ---------------------------------------------------------------------------

#[test]
fn session_info_string_includes_compression_summary() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.compression_stats = compression_fixture();
	let text = session.get_session_info_string();
	assert!(text.contains("Compression: 5 runs"), "text: {text}");
	assert!(text.contains("tokens saved"), "text: {text}");
	assert!(text.contains("800 in / 200 out"), "text: {text}");
	assert!(text.contains("cost $0.05000"), "text: {text}");
}

#[test]
fn session_info_string_includes_time_and_tool_calls() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_api_time_ms = 1_500;
	session.session.info.total_tool_time_ms = 2_500;
	session.session.info.total_layer_time_ms = 500;
	session.session.info.turn_timing.completed = 2;
	session.session.info.turn_timing.total_time_ms = 8_000;
	session.session.info.turn_timing.last_time_ms = 3_000;
	session.session.info.tool_calls = 9;
	let text = session.get_session_info_string();
	assert!(text.contains("Total time: "), "text: {text}");
	assert!(text.contains("(API: "), "text: {text}");
	assert!(text.contains("Turns: 2 completed"), "text: {text}");
	assert!(text.contains("Tool calls: 9"), "text: {text}");
}

#[test]
fn session_info_string_empty_session_omits_optional_sections() {
	let session = ChatSession::for_tests(Vec::new());
	let text = session.get_session_info_string();
	assert!(!text.contains("Compression:"), "text: {text}");
	assert!(!text.contains("Total time: "), "text: {text}");
	assert!(!text.contains("Tool calls:"), "text: {text}");
	assert!(!text.contains("Layer Statistics"), "text: {text}");
}

#[test]
fn session_info_string_renders_regular_and_command_layers() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.layer_stats = vec![
		layer("reduce", "ollama:fake", 500, 100, 0.01, 0, 0, 0),
		layer("reduce", "ollama:fake", 200, 50, 0.005, 0, 0, 0),
		layer("command:review", "gpt:fake", 300, 80, 0.02, 0, 0, 0),
	];
	let text = session.get_session_info_string();
	assert!(text.contains("─── Layer Statistics ───"), "text: {text}");
	assert!(text.contains("Layer: reduce"), "text: {text}");
	assert!(text.contains("Model: ollama:fake"), "text: {text}");
	assert!(text.contains("Executions: 2"), "text: {text}");
	assert!(text.contains("700 input, 150 output"), "text: {text}");
	assert!(text.contains("─── Command Layers ───"), "text: {text}");
	assert!(text.contains("Command: review"), "text: {text}");
	assert!(
		!text.contains("Command: command:review"),
		"prefix must be stripped: {text}"
	);
}

// ---------------------------------------------------------------------------
// render_layer_stats
// ---------------------------------------------------------------------------

#[test]
fn render_layer_stats_empty_slice_is_noop() {
	render_layer_stats(&[]);
}

#[test]
fn render_layer_stats_single_stat_without_time_skips_time_row() {
	let stat = layer("reduce", "ollama:fake", 10, 5, 0.0, 0, 0, 0);
	render_layer_stats(&[&stat]);
}

#[test]
fn render_layer_stats_multiple_stats_renders_aggregate() {
	let a = layer("reduce", "ollama:fake", 500, 100, 0.01, 1_200, 300, 1_500);
	let b = layer("reduce", "ollama:fake", 200, 50, 0.005, 400, 0, 400);
	render_layer_stats(&[&a, &b]);
}

// ---------------------------------------------------------------------------
// display_session_info
// ---------------------------------------------------------------------------

#[test]
fn display_session_info_with_compression_stats_renders_section() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.compression_stats = compression_fixture();
	session.display_session_info();
}

#[test]
fn display_session_info_compression_without_optional_metrics() {
	// Compressions happened but every derived metric is zero — the optional
	// ratio/tokens/throughput/cost rows must be skipped without panicking.
	let mut session = ChatSession::for_tests(Vec::new());
	let mut cs = crate::session::CompressionStats::default();
	cs.add_compression(crate::session::CompressionKind::Task, 3, 0);
	session.session.info.compression_stats = cs;
	session.display_session_info();
}

#[test]
fn display_session_info_renders_time_and_tool_call_rows() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_api_time_ms = 1_000;
	session.session.info.total_tool_time_ms = 500;
	session.session.info.total_layer_time_ms = 250;
	session.session.info.tool_calls = 3;
	session.display_session_info();
}

#[test]
fn display_session_info_renders_layer_notes() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.layer_stats = vec![
		layer("context_optimization", "m:fake", 1, 1, 0.0, 0, 0, 0),
		layer("command:review", "m:fake", 1, 1, 0.0, 0, 0, 0),
	];
	session.display_session_info();
}

// ---------------------------------------------------------------------------
// display_session_context (populated-session path)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn display_session_context_with_populated_session() {
	let mut session = ChatSession::for_tests(Vec::new());
	let config = default_config();
	session
		.add_user_message("inspect the logs")
		.expect("add user message");
	session
		.add_assistant_message("On it.", None, &config, "assistant")
		.expect("add assistant message");
	session.display_session_context(&config).await;
}

#[tokio::test]
async fn test_context_rendering_rich_messages_and_edge_filters() {
	let config = default_config();

	// Long content exercises the >200-char truncation branch; the assistant
	// message carries cache + tool_calls rows, the tool message carries
	// tool_call_id/name/images rows.
	let long_content = "word ".repeat(60);
	let assistant = crate::session::Message {
		role: "assistant".to_string(),
		content: long_content,
		timestamp: 1_700_000_000,
		cached: true,
		tool_calls: Some(serde_json::json!([
			{"id": "call_1", "function": {"name": "shell", "arguments": "{}"}}
		])),
		..Default::default()
	};

	let tool = crate::session::Message {
		role: "tool".to_string(),
		content: "ok".to_string(),
		timestamp: 1_700_000_001,
		tool_call_id: Some("call_1".to_string()),
		name: Some("shell".to_string()),
		images: Some(vec![crate::session::image::ImageAttachment {
			data: crate::session::image::ImageData::Base64("unused".to_string()),
			media_type: "image/png".to_string(),
			source_type: crate::session::image::SourceType::Clipboard,
			dimensions: Some((1, 1)),
			size_bytes: None,
		}]),
		..Default::default()
	};

	let mut session = ChatSession::for_tests(vec![assistant, tool]);

	// "all" renders every optional row: cached, tool call id, name, images,
	// tool calls, and the truncation notice.
	session
		.display_session_context_filtered(&config, "all")
		.await;

	// Unknown filter → error block; "user" with no user messages → empty match.
	session
		.display_session_context_filtered(&config, "bogus")
		.await;
	session
		.display_session_context_filtered(&config, "user")
		.await;

	// Debug mode lifts the content limit and switches the mode footer.
	let mut debug_config = default_config();
	debug_config.log_level = crate::config::LogLevel::Debug;
	session
		.display_session_context_filtered(&debug_config, "all")
		.await;
}
