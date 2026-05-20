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

//! Session restoration tests
//!
//! Tests for session resume scenarios to ensure sessions restore to exact state
//! regardless of interruptions (Ctrl+C, compression, /done, etc.)
//!
//! ## Test Coverage
//! - TRUNCATION_POINT: Ctrl+C cleanup during various processing states
//! - COMPRESSION_POINT: Message compression during conversation
//! - RESTORATION_POINT: /done optimization
//! - Combined scenarios: Multiple markers in sequence
//! - Edge cases: Empty sessions, single message, boundary conditions

#[cfg(test)]
mod session_restore_tests {
	use octomind::session::{Message, SessionInfo};
	use std::fs::{self, File};
	use std::io::Write;
	use std::path::PathBuf;
	use std::time::{SystemTime, UNIX_EPOCH};
	use tempfile::TempDir;

	/// Helper to create a test message
	fn create_message(role: &str, content: &str) -> Message {
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		Message {
			role: role.to_string(),
			content: content.to_string(),
			timestamp,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		}
	}

	/// Helper to create a test session file with messages and markers
	fn create_test_session_file(
		temp_dir: &TempDir,
		messages: Vec<Message>,
		markers: Vec<serde_json::Value>,
	) -> PathBuf {
		let session_file = temp_dir.path().join("test_session.jsonl");
		let mut file = File::create(&session_file).expect("Failed to create session file");

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// Write session info (SUMMARY)
		let session_info = SessionInfo {
			name: "test_session".to_string(),
			role: String::new(),
			created_at: timestamp,
			model: "test-model".to_string(),
			input_tokens: 0,
			output_tokens: 0,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			reasoning_tokens: 0,
			total_cost: 0.0,
			duration_seconds: 0,
			layer_stats: vec![],
			tool_calls: 0,
			total_api_time_ms: 0,
			total_tool_time_ms: 0,
			total_layer_time_ms: 0,
			compression_stats: octomind::session::CompressionStats::default(),
			anchor: Default::default(),
			total_api_calls: 0,
			current_non_cached_tokens: 0,
			current_total_tokens: 0,
			last_cache_checkpoint_time: timestamp,
			cache_next_user_message: false,
			spending_threshold_checkpoint: 0.0,
			compression_hint_count: 0,
			last_compression_hint_shown: 0,
			context_tokens_after_last_compression: 0,
			predicted_turns_at_last_compression: 0.0,
			api_calls_at_last_compression: 0,
			output_tokens_at_last_compression: 0,
			consecutive_compressions: 0,
		};

		let summary = serde_json::json!({
			"type": "SUMMARY",
			"timestamp": timestamp,
			"session_info": session_info,
		});

		writeln!(file, "{}", serde_json::to_string(&summary).unwrap())
			.expect("Failed to write summary");

		// Write messages (as plain JSON, not wrapped in MESSAGE type)
		for msg in &messages {
			writeln!(file, "{}", serde_json::to_string(&msg).unwrap())
				.expect("Failed to write message");
		}

		// Write markers
		for marker in markers {
			writeln!(file, "{}", serde_json::to_string(&marker).unwrap())
				.expect("Failed to write marker");
		}

		session_file
	}

	#[test]
	fn test_restore_normal_session_without_markers() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Hello"),
			create_message("assistant", "Hi there!"),
			create_message("user", "How are you?"),
			create_message("assistant", "I'm doing well!"),
		];

		let session_file = create_test_session_file(&temp_dir, messages.clone(), vec![]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(
			loaded_session.messages.len(),
			5,
			"Should restore all 5 messages"
		);
		assert_eq!(loaded_session.messages[0].role, "system");
		assert_eq!(loaded_session.messages[1].content, "Hello");
		assert_eq!(loaded_session.messages[4].content, "I'm doing well!");
	}

	#[test]
	fn test_restore_with_truncation_point_ctrl_c_cleanup() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
			create_message("user", "Message 2"),
			create_message("assistant", "Response 2"),
			// These messages were added but then Ctrl+C was pressed
			create_message("user", "Message 3 - interrupted"),
			create_message("assistant", "Response 3 - incomplete"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// TRUNCATION_POINT indicates cleanup removed last 2 messages
		let truncation_marker = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp,
			"message_count": 5, // Only keep first 5 messages
			"reason": "ctrl_c_cleanup"
		});

		let session_file = create_test_session_file(&temp_dir, messages, vec![truncation_marker]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(
			loaded_session.messages.len(),
			5,
			"Should truncate to 5 messages as specified by TRUNCATION_POINT"
		);
		assert_eq!(loaded_session.messages[4].content, "Response 2");
		// Messages 6 and 7 should be removed
	}

	#[test]
	fn test_restore_with_compression_point() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		// Simulate: 10 messages compressed into 1 summary
		let messages_before_compression = vec![
			create_message("system", "System prompt"),
			create_message("user", "Old message 1"),
			create_message("assistant", "Old response 1"),
			create_message("user", "Old message 2"),
			create_message("assistant", "Old response 2"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		let compression_marker = serde_json::json!({
			"type": "COMPRESSION_POINT",
			"timestamp": timestamp,
			"compression_type": "adaptive",
			"messages_removed": 4,
			"summary_added": true
		});

		// Create session file with messages before compression
		let session_file = create_test_session_file(&temp_dir, messages_before_compression, vec![]);

		// Append compression marker
		let mut file = fs::OpenOptions::new()
			.append(true)
			.open(&session_file)
			.expect("Failed to open session file");

		writeln!(
			file,
			"{}",
			serde_json::to_string(&compression_marker).unwrap()
		)
		.expect("Failed to write marker");

		// Messages after compression (written AFTER the marker)
		let messages_after_compression = vec![
			create_message("user", "Summary of previous conversation"),
			create_message("user", "New message after compression"),
			create_message("assistant", "New response after compression"),
		];

		for msg in &messages_after_compression {
			writeln!(file, "{}", serde_json::to_string(&msg).unwrap())
				.expect("Failed to write message");
		}

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// After COMPRESSION_POINT, old messages are cleared
		// Only system + post-compression messages remain
		assert!(
			loaded_session.messages.len() >= 3,
			"Should have system + summary + new messages after compression, got {}",
			loaded_session.messages.len()
		);
	}

	#[test]
	fn test_restore_with_restoration_point_done_command() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages_before_done = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
			create_message("user", "Message 2"),
			create_message("assistant", "Response 2"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		let restoration_marker = serde_json::json!({
			"type": "RESTORATION_POINT",
			"timestamp": timestamp,
			"reason": "done_command"
		});

		// Create session file with messages before /done
		let session_file = create_test_session_file(&temp_dir, messages_before_done, vec![]);

		// Append restoration marker
		let mut file = fs::OpenOptions::new()
			.append(true)
			.open(&session_file)
			.expect("Failed to open session file");

		writeln!(
			file,
			"{}",
			serde_json::to_string(&restoration_marker).unwrap()
		)
		.expect("Failed to write marker");

		// Messages after /done optimization (written AFTER the marker)
		let messages_after_done = vec![create_message("user", "Optimized summary")];

		for msg in &messages_after_done {
			writeln!(file, "{}", serde_json::to_string(&msg).unwrap())
				.expect("Failed to write message");
		}

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// After RESTORATION_POINT, old messages are cleared
		// Only system + optimized summary remain
		assert!(
			!loaded_session.messages.is_empty(),
			"Should have optimized summary after /done, got {}",
			loaded_session.messages.len()
		);
	}

	#[test]
	fn test_restore_with_multiple_truncation_points() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
			create_message("user", "Message 2"),
			create_message("assistant", "Response 2"),
			create_message("user", "Message 3"),
			create_message("assistant", "Response 3"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// First Ctrl+C: truncate to 5 messages
		let truncation1 = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp,
			"message_count": 5,
			"reason": "ctrl_c_cleanup"
		});

		// Second Ctrl+C: truncate to 3 messages
		let truncation2 = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp + 10,
			"message_count": 3,
			"reason": "ctrl_c_cleanup"
		});

		let markers = vec![truncation1, truncation2];

		let session_file = create_test_session_file(&temp_dir, messages, markers);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// Last TRUNCATION_POINT wins
		assert_eq!(
			loaded_session.messages.len(),
			3,
			"Should apply last TRUNCATION_POINT (3 messages)"
		);
	}

	#[test]
	fn test_restore_with_compression_then_truncation() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// Create session file with initial messages
		let session_file = create_test_session_file(&temp_dir, messages, vec![]);

		// Append compression marker
		let mut file = fs::OpenOptions::new()
			.append(true)
			.open(&session_file)
			.expect("Failed to open session file");

		let compression_marker = serde_json::json!({
			"type": "COMPRESSION_POINT",
			"timestamp": timestamp,
			"compression_type": "adaptive",
			"messages_removed": 2,
			"summary_added": true
		});

		writeln!(
			file,
			"{}",
			serde_json::to_string(&compression_marker).unwrap()
		)
		.expect("Failed to write marker");

		// Write messages after compression
		let post_compression_messages = vec![
			create_message("user", "Summary"),
			create_message("user", "New message"),
		];

		for msg in &post_compression_messages {
			writeln!(file, "{}", serde_json::to_string(&msg).unwrap())
				.expect("Failed to write message");
		}

		// Then: Ctrl+C truncates to 1 message
		let truncation_marker = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp + 10,
			"message_count": 1,
			"reason": "ctrl_c_cleanup"
		});

		writeln!(
			file,
			"{}",
			serde_json::to_string(&truncation_marker).unwrap()
		)
		.expect("Failed to write marker");

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// Compression clears, then truncation applies to remaining messages
		assert_eq!(
			loaded_session.messages.len(),
			1,
			"Should have 1 message after compression + truncation"
		);
	}

	#[test]
	fn test_restore_empty_session() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![];

		let session_file = create_test_session_file(&temp_dir, messages, vec![]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(
			loaded_session.messages.len(),
			0,
			"Empty session should restore with 0 messages"
		);
	}

	#[test]
	fn test_restore_single_system_message() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![create_message("system", "System prompt")];

		let session_file = create_test_session_file(&temp_dir, messages, vec![]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(
			loaded_session.messages.len(),
			1,
			"Should restore single system message"
		);
		assert_eq!(loaded_session.messages[0].role, "system");
	}

	#[test]
	fn test_truncation_point_with_zero_messages() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// Edge case: truncate to 0 messages
		let truncation_marker = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp,
			"message_count": 0,
			"reason": "ctrl_c_cleanup"
		});

		let markers = vec![truncation_marker];

		let session_file = create_test_session_file(&temp_dir, messages, markers);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(
			loaded_session.messages.len(),
			0,
			"Should truncate to 0 messages"
		);
	}

	#[test]
	fn test_truncation_point_beyond_message_count() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// Edge case: truncate to more messages than exist
		let truncation_marker = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp,
			"message_count": 100, // More than actual message count
			"reason": "ctrl_c_cleanup"
		});

		let markers = vec![truncation_marker];

		let session_file = create_test_session_file(&temp_dir, messages.clone(), markers);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// Should keep all messages (truncate is no-op if target > current)
		assert_eq!(
			loaded_session.messages.len(),
			messages.len(),
			"Should keep all messages when truncation target exceeds count"
		);
	}

	#[test]
	fn test_restore_with_restoration_point_then_truncation() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "Message 1"),
			create_message("assistant", "Response 1"),
		];

		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		// Create session file with initial messages
		let session_file = create_test_session_file(&temp_dir, messages, vec![]);

		// Append restoration marker
		let mut file = fs::OpenOptions::new()
			.append(true)
			.open(&session_file)
			.expect("Failed to open session file");

		let restoration_marker = serde_json::json!({
			"type": "RESTORATION_POINT",
			"timestamp": timestamp,
			"reason": "done_command"
		});

		writeln!(
			file,
			"{}",
			serde_json::to_string(&restoration_marker).unwrap()
		)
		.expect("Failed to write marker");

		// Write messages after restoration
		let post_restoration_messages = vec![
			create_message("user", "Summary"),
			create_message("user", "New message"),
		];

		for msg in &post_restoration_messages {
			writeln!(file, "{}", serde_json::to_string(&msg).unwrap())
				.expect("Failed to write message");
		}

		// Then: Ctrl+C truncates
		let truncation_marker = serde_json::json!({
			"type": "TRUNCATION_POINT",
			"timestamp": timestamp + 10,
			"message_count": 1,
			"reason": "ctrl_c_cleanup"
		});

		writeln!(
			file,
			"{}",
			serde_json::to_string(&truncation_marker).unwrap()
		)
		.expect("Failed to write marker");

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// RESTORATION_POINT clears messages, then TRUNCATION_POINT applies to restoration_messages
		assert_eq!(
			loaded_session.messages.len(),
			1,
			"Should have 1 message after restoration + truncation"
		);
	}

	#[test]
	fn test_restore_preserves_message_order() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "First user message"),
			create_message("assistant", "First assistant response"),
			create_message("user", "Second user message"),
			create_message("assistant", "Second assistant response"),
		];

		let session_file = create_test_session_file(&temp_dir, messages.clone(), vec![]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(loaded_session.messages.len(), messages.len());

		for (i, msg) in loaded_session.messages.iter().enumerate() {
			assert_eq!(
				msg.role, messages[i].role,
				"Message {} role should match",
				i
			);
			assert_eq!(
				msg.content, messages[i].content,
				"Message {} content should match",
				i
			);
		}
	}

	#[test]
	fn test_restore_with_tool_calls() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let _timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		let mut assistant_msg = create_message("assistant", "Let me help with that");
		assistant_msg.tool_calls = Some(serde_json::json!([
			{
				"id": "call_123",
				"type": "function",
				"function": {
					"name": "test_tool",
					"arguments": "{}"
				}
			}
		]));

		let mut tool_result = create_message("tool", "Tool result");
		tool_result.tool_call_id = Some("call_123".to_string());
		tool_result.name = Some("test_tool".to_string());

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "User message"),
			assistant_msg,
			tool_result,
			create_message("assistant", "Final response"),
		];

		let session_file = create_test_session_file(&temp_dir, messages.clone(), vec![]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		assert_eq!(loaded_session.messages.len(), 5);
		assert!(loaded_session.messages[2].tool_calls.is_some());
		assert_eq!(
			loaded_session.messages[3].tool_call_id,
			Some("call_123".to_string())
		);
	}

	/// Regression test: assistant's final response (no tool calls) must be persisted
	/// to the session file so it survives session close and --resume-recent.
	/// Bug: handle_final_response pushed to in-memory messages but never wrote to file.
	#[test]
	fn test_restore_final_assistant_response_persisted() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		// Simulate a conversation where the assistant's last message is a plain
		// text response (no tool calls) — the exact path handle_final_response takes.
		let messages = vec![
			create_message("system", "You are a helpful assistant"),
			create_message("user", "What is 2+2?"),
			create_message("assistant", "2+2 equals 4."),
			create_message("user", "And 3+3?"),
			create_message("assistant", "3+3 equals 6."),
		];

		let session_file = create_test_session_file(&temp_dir, messages.clone(), vec![]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// The final assistant message must be present after restore
		assert_eq!(
			loaded_session.messages.len(),
			5,
			"All 5 messages (including final assistant response) must be restored"
		);
		assert_eq!(loaded_session.messages[4].role, "assistant");
		assert_eq!(loaded_session.messages[4].content, "3+3 equals 6.");
	}

	/// Regression test: session ending with user message followed by assistant response
	/// must restore both — the exact scenario from the --resume-recent bug.
	#[test]
	fn test_restore_session_ending_with_assistant_response() {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

		let messages = vec![
			create_message("system", "System prompt"),
			create_message("user", "First question"),
			create_message("assistant", "First answer"),
			create_message("user", "Second question"),
			create_message("assistant", "Second answer — this was the last thing shown"),
		];

		// Also add a SUMMARY marker after messages (simulates session save on close)
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();
		let summary_marker = serde_json::json!({
			"type": "SUMMARY",
			"timestamp": timestamp,
			"session_info": {
				"name": "test",
				"created_at": timestamp,
				"model": "test-model",
				"role": "developer:general",
				"provider": "test",
				"input_tokens": 100,
				"output_tokens": 50,
				"cache_read_tokens": 0,
				"cache_write_tokens": 0,
				"reasoning_tokens": 0,
				"total_cost": 0.01,
				"duration_seconds": 60,
				"layer_stats": [],
				"tool_calls": 0,
				"total_api_time_ms": 0,
				"total_tool_time_ms": 0,
				"total_layer_time_ms": 0,
				"compression_stats": octomind::session::CompressionStats::default(),
				"total_api_calls": 2,
				"current_non_cached_tokens": 0,
				"current_total_tokens": 150,
				"last_cache_checkpoint_time": timestamp,
				"cache_next_user_message": false,
				"spending_threshold_checkpoint": 0.0,
				"compression_hint_count": 0,
				"last_compression_hint_shown": 0,
				"context_tokens_after_last_compression": 0,
				"predicted_turns_at_last_compression": 0.0,
				"api_calls_at_last_compression": 0,
				"output_tokens_at_last_compression": 0,
				"consecutive_compressions": 0
			}
		});

		let session_file =
			create_test_session_file(&temp_dir, messages.clone(), vec![summary_marker]);

		let loaded_session =
			octomind::session::load_session(&session_file).expect("Failed to load session");

		// The assistant's final response must survive even with a trailing SUMMARY marker
		assert_eq!(
			loaded_session.messages.len(),
			5,
			"All messages must be restored; assistant's final response must not be lost"
		);

		let last_msg = &loaded_session.messages[4];
		assert_eq!(last_msg.role, "assistant");
		assert_eq!(
			last_msg.content,
			"Second answer — this was the last thing shown"
		);
	}
}
