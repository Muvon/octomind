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
use crate::session::persistence::has_incomplete_tool_calls;
use serde_json::json;

#[test]
fn turn_timing_records_completed_turns_and_average() {
	let mut timing = TurnTimingStats::default();
	timing.record(std::time::Duration::from_millis(1_500));
	timing.record(std::time::Duration::from_millis(500));

	assert_eq!(timing.completed, 2);
	assert_eq!(timing.total_time_ms, 2_000);
	assert_eq!(timing.average_time_ms(), 1_000);
	assert_eq!(timing.last_time_ms, 500);
}

#[test]
fn learning_stats_persist_and_default_for_old_sessions() {
	let old: SessionInfo = serde_json::from_value(json!({
		"name": "old",
		"created_at": 1,
		"model": "test",
		"role": "assistant",
		"input_tokens": 0,
		"output_tokens": 0,
		"cache_read_tokens": 0,
		"cache_write_tokens": 0,
		"total_cost": 0.0,
		"duration_seconds": 0,
		"layer_stats": []
	}))
	.unwrap();
	assert_eq!(old.learning_stats.packs, 0);
	assert_eq!(old.turn_timing.completed, 0);

	let mut current = SessionInfo::default();
	current.learning_stats.record_pack(4, 700);
	current.learning_stats.record_use(0.05);
	let restored: SessionInfo =
		serde_json::from_value(serde_json::to_value(current).unwrap()).unwrap();
	assert_eq!(restored.learning_stats.items, 4);
	assert_eq!(restored.learning_stats.tokens, 700);
	assert_eq!(restored.learning_stats.used, 1);
	assert_eq!(restored.learning_stats.credit_positive, 1);
}

#[test]
fn latest_task_timestamp_is_the_live_request_message_timestamp() {
	let user = |content: &str, timestamp: u64| Message {
		role: "user".into(),
		content: content.into(),
		timestamp,
		..Default::default()
	};
	let mut assistant = user("working on it", 15);
	assistant.role = "assistant".into();
	let messages = vec![
		user("first task", 10),
		assistant,
		user("<system-note>not a task</system-note>", 20),
		user("newest real task", 30),
	];
	assert_eq!(latest_task_timestamp(&messages), Some(30));
	assert_eq!(latest_task_timestamp(&[]), None);
}

fn create_test_message(
	role: &str,
	content: &str,
	tool_calls: Option<serde_json::Value>,
	tool_call_id: Option<String>,
) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp: 1234567890,
		cached: false,
		cache_ttl: None,
		tool_call_id,
		name: None,
		tool_calls,
		images: None,
		videos: None,
		thinking: None,
		id: None,
	}
}
#[test]
fn latest_task_falls_back_to_continuation_after_compaction() {
	let wrapper = "<continuation>\nResume from where the previous turn left off.\n<task>\nprepare the benchmark script\n</task>\n</continuation>";

	// While a genuine user turn survives, it wins outright.
	let live = vec![
		create_test_message("system", "sys", None, None),
		create_test_message("user", wrapper, None, None),
		create_test_message("user", "prepare the benchmark script", None, None),
	];
	assert_eq!(
		latest_real_user_task_content(&live),
		Some("prepare the benchmark script")
	);

	// After compaction drains every raw user turn, the wrapper carries the
	// live request — without this the whole supervisor stack goes blind.
	let compacted = vec![
		create_test_message("system", "sys", None, None),
		create_test_message("assistant", "<conversation_summary/>", None, None),
		create_test_message("user", wrapper, None, None),
	];
	assert_eq!(
		latest_real_user_task_content(&compacted),
		Some("prepare the benchmark script")
	);

	// The synthetic placeholder is not intent — it must not resolve.
	let barren = vec![create_test_message(
		"user",
		&format!(
			"<continuation>\n<task>\n{}\n</task>\n</continuation>",
			CONTINUATION_FALLBACK_INTENT
		),
		None,
		None,
	)];
	assert_eq!(latest_real_user_task_content(&barren), None);

	// Non-wrappers and malformed wrappers never yield a task.
	assert_eq!(continuation_task("just a user message"), None);
	assert_eq!(continuation_task("<continuation>\nno task tag"), None);
	assert_eq!(continuation_task("<continuation>\n<task>\n</task>"), None);
}

#[test]
fn continuation_keeps_user_request_separate_from_resumption_action() {
	let wrapper = "<continuation>\n<request>\nShould work now\n</request>\n<task>\nContinue monitoring the active benchmark; the monitor is already running.\n</task>\n</continuation>";

	assert_eq!(continuation_task(wrapper), Some("Should work now"));
}

#[test]
fn ensure_system_managed_wraps_unmarked_content_only() {
	// Marked content passes through untouched.
	for marked in [
		"<pay-attention>\nnote\n</pay-attention>",
		"<recall>\nlessons\n</recall>",
		"<instructions>\nrole\n</instructions>",
		"<system-note>\nalready wrapped\n</system-note>",
	] {
		assert_eq!(ensure_system_managed(marked), marked);
		assert!(is_system_managed_user_content(marked));
	}
	// Unmarked injection content (inbox reports, tool-usage hints) is wrapped
	// so it can never classify as a genuine user turn.
	let report = "[Tap-run 'x' (dev) completed]\n\nall done";
	let wrapped = ensure_system_managed(report);
	assert!(wrapped.starts_with("<system-note>"));
	assert!(wrapped.contains(report));
	assert!(is_system_managed_user_content(&wrapped));
	assert!(!is_real_user_task_message(&create_test_message(
		"user", &wrapped, None, None
	)));
}

#[test]
fn test_has_incomplete_tool_calls_complete_sequence() {
	// Test complete tool call sequence: assistant -> tool_calls -> tool_response
	let messages = vec![
		create_test_message("user", "List files", None, None),
		create_test_message(
			"assistant",
			"I'll list the files for you.",
			Some(
				json!([{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}}]),
			),
			None,
		),
		create_test_message(
			"tool",
			"file1.txt\nfile2.txt",
			None,
			Some("call_123".to_string()),
		),
		create_test_message(
			"assistant",
			"Here are the files in the directory.",
			None,
			None,
		),
	];

	// This should NOT be considered incomplete
	assert!(!has_incomplete_tool_calls(&messages));
}

#[test]
fn test_has_incomplete_tool_calls_incomplete_sequence() {
	// Test incomplete tool call sequence: assistant -> tool_calls -> [missing tool response]
	let messages = vec![
		create_test_message("user", "List files", None, None),
		create_test_message(
			"assistant",
			"I'll list the files for you.",
			Some(
				json!([{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}}]),
			),
			None,
		),
		// Missing tool response - this should be detected as incomplete
	];

	// This SHOULD be considered incomplete
	assert!(has_incomplete_tool_calls(&messages));
}

#[test]
fn test_has_incomplete_tool_calls_multiple_calls_partial() {
	// Test multiple tool calls where some have responses and some don't
	let messages = vec![
		create_test_message("user", "Do multiple things", None, None),
		create_test_message(
			"assistant",
			"I'll do multiple things.",
			Some(json!([
				{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}},
				{"id": "call_456", "name": "shell", "arguments": {"command": "pwd"}}
			])),
			None,
		),
		create_test_message(
			"tool",
			"file1.txt\nfile2.txt",
			None,
			Some("call_123".to_string()),
		),
		// Missing response for call_456 - this should be detected as incomplete
	];

	// This SHOULD be considered incomplete (call_456 has no response)
	assert!(has_incomplete_tool_calls(&messages));
}

#[test]
fn test_has_incomplete_tool_calls_no_tool_calls() {
	// Test messages with no tool calls
	let messages = vec![
		create_test_message("user", "Hello", None, None),
		create_test_message("assistant", "Hello! How can I help you?", None, None),
	];

	// This should NOT be considered incomplete
	assert!(!has_incomplete_tool_calls(&messages));
}

#[test]
fn test_clean_interrupted_tool_calls_preserves_complete() {
	// Test that complete sequences are preserved
	let mut messages = vec![
		create_test_message("user", "List files", None, None),
		create_test_message(
			"assistant",
			"I'll list the files for you.",
			Some(
				json!([{"id": "call_123", "name": "list_files", "arguments": {"directory": "."}}]),
			),
			None,
		),
		create_test_message(
			"tool",
			"file1.txt\nfile2.txt",
			None,
			Some("call_123".to_string()),
		),
		create_test_message("assistant", "Here are the files.", None, None),
	];

	let original_count = messages.len();
	let cleaned = clean_interrupted_tool_calls(&mut messages, "Test");

	// Should not clean anything (complete sequence)
	assert!(!cleaned);
	assert_eq!(messages.len(), original_count);
}

#[test]
fn test_clean_interrupted_tool_calls_inserts_synthetic_result() {
	// Test that incomplete sequences get a synthetic tool result instead of truncation
	let mut messages = vec![
		create_test_message("user", "List files", None, None),
		create_test_message(
			"assistant",
			"I'll list the files for you.",
			Some(
				json!([{"id": "call_123", "function": {"name": "list_files", "arguments": "{\"directory\": \".\"}"}}]),
			),
			None,
		),
		// Missing tool response - a synthetic result should be inserted
	];

	let cleaned = clean_interrupted_tool_calls(&mut messages, "Test");

	// Should insert a synthetic tool result, preserving all messages
	assert!(cleaned);
	assert_eq!(messages.len(), 3); // user + assistant + synthetic tool result
	assert_eq!(messages[0].role, "user");
	assert_eq!(messages[1].role, "assistant");
	assert_eq!(messages[2].role, "tool");
	assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_123"));
	assert!(messages[2].content.contains("interrupted"));
}

#[test]
fn test_session_loading_preserves_stats_from_summary() {
	// Test that SUMMARY is the source of truth and old STATS don't overwrite it
	use crate::session::persistence::append_to_session_file;
	use tempfile::NamedTempFile;

	// Create a temporary session file
	let temp_file = NamedTempFile::new().expect("Failed to create temp file");
	let path = temp_file.path().to_path_buf();

	// Write initial SUMMARY with some stats
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "SUMMARY",
			"timestamp": 1000,
			"session_info": {
				"name": "test-session",
				"created_at": 1000,
				"model": "openrouter:anthropic/claude-sonnet-4",
				"role": "developer",
				"provider": "openrouter",
				"input_tokens": 100,
				"output_tokens": 50,
				"cache_read_tokens": 20,
				"cache_write_tokens": 5,
				"total_cost": 0.001,
				"duration_seconds": 10,
				"layer_stats": [
					{
						"layer_type": "main",
						"model": "openrouter:anthropic/claude-sonnet-4",
						"input_tokens": 100,
						"output_tokens": 50,
						"cost": 0.001,
						"timestamp": 1000,
						"api_time_ms": 500,
						"tool_time_ms": 100,
						"total_time_ms": 600
					}
				],
				"tool_calls": 5,
				"total_api_time_ms": 500,
				"total_tool_time_ms": 100,
				"total_layer_time_ms": 600,
				"compression_stats": {
					"task_compressions": 0,
					"phase_compressions": 0,
					"project_compressions": 0,
					"conversation_compressions": 0,
					"total_messages_removed": 0,
					"total_tokens_saved": 0
				},

				"total_api_calls": 1,
				"current_non_cached_tokens": 0,
				"current_total_tokens": 0,
				"last_cache_checkpoint_time": 1000,
				"cache_next_user_message": false,
				"spending_threshold_checkpoint": 0.0,

			}
		}))
		.unwrap(),
	)
	.expect("Failed to write SUMMARY");

	// Write some STATS entries with OLDER timestamps (should be ignored)
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "STATS",
			"timestamp": 900, // OLDER than SUMMARY
			"total_cost": 0.0,
			"input_tokens": 0,
			"output_tokens": 0,
			"cache_read_tokens": 0,
			"cache_write_tokens": 0,
			"tool_calls": 0,
			"total_api_time_ms": 0,
			"total_tool_time_ms": 0,
			"total_layer_time_ms": 0,
			"model": "openrouter:anthropic/claude-sonnet-4",
			"provider": "openrouter"
		}))
		.unwrap(),
	)
	.expect("Failed to write old STATS");

	// Write a user message
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"role": "user",
			"content": "Hello",
			"timestamp": 1100,
			"cached": false
		}))
		.unwrap(),
	)
	.expect("Failed to write message");

	// Write final SUMMARY with updated stats (should be used)
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "SUMMARY",
			"timestamp": 2000, // NEWER timestamp
			"session_info": {
				"name": "test-session",
				"created_at": 1000,
				"model": "openrouter:anthropic/claude-sonnet-4",
				"role": "developer",
				"provider": "openrouter",
				"input_tokens": 200, // Updated values
				"output_tokens": 100,
				"cache_read_tokens": 40,
				"cache_write_tokens": 10,
				"total_cost": 0.002,
				"duration_seconds": 20,
				"layer_stats": [
					{
						"layer_type": "main",
						"model": "openrouter:anthropic/claude-sonnet-4",
						"input_tokens": 200,
						"output_tokens": 100,
						"cost": 0.002,
						"timestamp": 2000,
						"api_time_ms": 1000,
						"tool_time_ms": 200,
						"total_time_ms": 1200
					}
				],
				"tool_calls": 10,
				"total_api_time_ms": 1000,
				"total_tool_time_ms": 200,
				"total_layer_time_ms": 1200,
				"compression_stats": {
					"task_compressions": 0,
					"phase_compressions": 0,
					"project_compressions": 0,
					"conversation_compressions": 0,
					"total_messages_removed": 0,
					"total_tokens_saved": 0
				},

				"total_api_calls": 2,
				"current_non_cached_tokens": 0,
				"current_total_tokens": 0,
				"last_cache_checkpoint_time": 2000,
				"cache_next_user_message": false,
				"spending_threshold_checkpoint": 0.0,

			}
		}))
		.unwrap(),
	)
	.expect("Failed to write final SUMMARY");

	// Load the session
	let session = load_session(&path).expect("Failed to load session");

	// Verify that the FINAL SUMMARY values are used, not the old STATS
	assert_eq!(
		session.info.input_tokens, 200,
		"Input tokens should be from final SUMMARY"
	);
	assert_eq!(
		session.info.output_tokens, 100,
		"Output tokens should be from final SUMMARY"
	);
	assert_eq!(
		session.info.cache_read_tokens, 40,
		"Cache read tokens should be from final SUMMARY"
	);
	assert_eq!(
		session.info.total_cost, 0.002,
		"Total cost should be from final SUMMARY"
	);
	assert_eq!(
		session.info.tool_calls, 10,
		"Tool calls should be from final SUMMARY"
	);
	assert_eq!(
		session.info.total_api_time_ms, 1000,
		"API time should be from final SUMMARY"
	);
	assert_eq!(
		session.info.total_tool_time_ms, 200,
		"Tool time should be from final SUMMARY"
	);
	assert_eq!(
		session.info.total_layer_time_ms, 1200,
		"Layer time should be from final SUMMARY"
	);

	// CRITICAL: Verify layer_stats are preserved
	assert_eq!(
		session.info.layer_stats.len(),
		1,
		"Layer stats should be preserved"
	);
	assert_eq!(
		session.info.layer_stats[0].input_tokens, 200,
		"Layer stats should match final SUMMARY"
	);
	assert_eq!(
		session.info.layer_stats[0].output_tokens, 100,
		"Layer stats should match final SUMMARY"
	);
	assert_eq!(
		session.info.layer_stats[0].cost, 0.002,
		"Layer stats cost should match final SUMMARY"
	);

	// Verify messages are loaded
	assert_eq!(session.messages.len(), 1, "Should have 1 message");
	assert_eq!(
		session.messages[0].role, "user",
		"Message should be user message"
	);

	// Verify model is preserved from SUMMARY
	assert_eq!(
		session.info.model, "openrouter:anthropic/claude-sonnet-4",
		"Model should be from SUMMARY"
	);
}

#[test]
fn test_session_loading_restores_model_from_command() {
	// Test that model changes via /model command are properly restored
	use crate::session::persistence::append_to_session_file;
	use tempfile::NamedTempFile;

	let temp_file = NamedTempFile::new().expect("Failed to create temp file");
	let path = temp_file.path().to_path_buf();

	// Write initial SUMMARY with original model
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "SUMMARY",
			"timestamp": 1000,
			"session_info": {
				"name": "test-session",
				"created_at": 1000,
				"model": "openrouter:anthropic/claude-sonnet-4",
				"role": "developer",
				"provider": "openrouter",
				"input_tokens": 100,
				"output_tokens": 50,
				"cache_read_tokens": 20,
				"cache_write_tokens": 5,
				"total_cost": 0.001,
				"duration_seconds": 10,
				"layer_stats": [],
				"tool_calls": 5,
				"total_api_time_ms": 500,
				"total_tool_time_ms": 100,
				"total_layer_time_ms": 600,
				"compression_stats": {
					"task_compressions": 0,
					"phase_compressions": 0,
					"project_compressions": 0,
					"conversation_compressions": 0,
					"total_messages_removed": 0,
					"total_tokens_saved": 0
				},
				"total_api_calls": 1,
				"current_non_cached_tokens": 0,
				"current_total_tokens": 0,
				"last_cache_checkpoint_time": 1000,
				"cache_next_user_message": false,
				"spending_threshold_checkpoint": 0.0,

			}
		}))
		.unwrap(),
	)
	.expect("Failed to write SUMMARY");

	// Write a /model command that changes the model
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "COMMAND",
			"timestamp": 1500,
			"command": "/model openrouter:openai/gpt-4o"
		}))
		.unwrap(),
	)
	.expect("Failed to write COMMAND");

	// Write a user message
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"role": "user",
			"content": "Hello with new model",
			"timestamp": 1600,
			"cached": false
		}))
		.unwrap(),
	)
	.expect("Failed to write message");

	// Write final SUMMARY with the changed model
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "SUMMARY",
			"timestamp": 2000,
			"session_info": {
				"name": "test-session",
				"created_at": 1000,
				"model": "openrouter:openai/gpt-4o",
				"role": "developer",
				"provider": "openrouter",
				"input_tokens": 200,
				"output_tokens": 100,
				"cache_read_tokens": 40,
				"cache_write_tokens": 10,
				"total_cost": 0.002,
				"duration_seconds": 20,
				"layer_stats": [],
				"tool_calls": 10,
				"total_api_time_ms": 1000,
				"total_tool_time_ms": 200,
				"total_layer_time_ms": 1200,
				"compression_stats": {
					"task_compressions": 0,
					"phase_compressions": 0,
					"project_compressions": 0,
					"conversation_compressions": 0,
					"total_messages_removed": 0,
					"total_tokens_saved": 0
				},
				"total_api_calls": 2,
				"current_non_cached_tokens": 0,
				"current_total_tokens": 0,
				"last_cache_checkpoint_time": 2000,
				"cache_next_user_message": false,
				"spending_threshold_checkpoint": 0.0,

			}
		}))
		.unwrap(),
	)
	.expect("Failed to write final SUMMARY");

	// Load the session
	let session = load_session(&path).expect("Failed to load session");

	// Verify that the changed model is restored
	// The /model command should be detected and applied
	assert_eq!(
		session.info.model, "openrouter:openai/gpt-4o",
		"Model should be restored from /model command and final SUMMARY"
	);

	// Verify stats are also correct
	assert_eq!(session.info.input_tokens, 200);
	assert_eq!(session.info.total_cost, 0.002);
}

#[test]
fn test_session_loading_model_without_command() {
	// Test that model is restored from SUMMARY when no /model command was used
	use crate::session::persistence::append_to_session_file;
	use tempfile::NamedTempFile;

	let temp_file = NamedTempFile::new().expect("Failed to create temp file");
	let path = temp_file.path().to_path_buf();

	// Write SUMMARY with a specific model (no /model command in session)
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"type": "SUMMARY",
			"timestamp": 1000,
			"session_info": {
				"name": "test-session",
				"created_at": 1000,
				"model": "openrouter:google/gemini-2.0-flash-exp:free",
				"role": "developer",
				"provider": "openrouter",
				"input_tokens": 100,
				"output_tokens": 50,
				"cache_read_tokens": 20,
				"cache_write_tokens": 5,
				"total_cost": 0.001,
				"duration_seconds": 10,
				"layer_stats": [],
				"tool_calls": 5,
				"total_api_time_ms": 500,
				"total_tool_time_ms": 100,
				"total_layer_time_ms": 600,
				"compression_stats": {
					"task_compressions": 0,
					"phase_compressions": 0,
					"project_compressions": 0,
					"conversation_compressions": 0,
					"total_messages_removed": 0,
					"total_tokens_saved": 0
				},
				"total_api_calls": 1,
				"current_non_cached_tokens": 0,
				"current_total_tokens": 0,
				"last_cache_checkpoint_time": 1000,
				"cache_next_user_message": false,
				"spending_threshold_checkpoint": 0.0,

			}
		}))
		.unwrap(),
	)
	.expect("Failed to write SUMMARY");

	// Write a user message (no /model command)
	append_to_session_file(
		&path,
		&serde_json::to_string(&json!({
			"role": "user",
			"content": "Hello",
			"timestamp": 1100,
			"cached": false
		}))
		.unwrap(),
	)
	.expect("Failed to write message");

	// Load the session
	let session = load_session(&path).expect("Failed to load session");

	// Verify that the model from SUMMARY is preserved
	assert_eq!(
		session.info.model, "openrouter:google/gemini-2.0-flash-exp:free",
		"Model should be restored from SUMMARY when no /model command exists"
	);
}
