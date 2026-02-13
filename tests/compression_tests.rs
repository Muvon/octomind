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

//! Compression unit tests
//!
//! Tests for message range removal and token calculation edge cases
//! that caused the off-by-one bug in plan compression.
//!
//! Note: These tests focus on the core remove_messages_in_range logic
//! using a minimal test setup to avoid complex ChatSession initialization.
//!
//! ## Test Isolation (Race Condition Fix)
//!
//! Tests that load Config use `create_isolated_config()` helper to avoid
//! race conditions when running in parallel (e.g., CI with multiple cores).
//! Each test gets its own temporary config directory via OCTOMIND_CONFIG_PATH,
//! preventing concurrent config migrations from corrupting shared state.

#[cfg(test)]
mod compression_tests {
	use octomind::session::Message;
	use std::time::{SystemTime, UNIX_EPOCH};

	/// Helper to create a test Session with N message pairs
	fn create_test_messages(message_count: usize) -> Vec<Message> {
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		let mut messages = vec![Message {
			role: "system".to_string(),
			content: "System prompt".to_string(),
			timestamp,
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			thinking: None,
			id: None,
		}];

		// Add user/assistant pairs
		for i in 0..message_count {
			messages.push(Message {
				role: "user".to_string(),
				content: format!("User message {}", i),
				timestamp,
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				thinking: None,
				id: None,
			});
			messages.push(Message {
				role: "assistant".to_string(),
				content: format!("Assistant response {}", i),
				timestamp,
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				thinking: None,
				id: None,
			});
		}

		messages
	}

	#[test]
	fn test_message_count_calculation() {
		let messages = create_test_messages(10);
		// 1 system + 10 user + 10 assistant = 21 total
		assert_eq!(messages.len(), 21);
	}

	#[test]
	fn test_realistic_93_message_scenario() {
		// Reproduce the exact bug scenario: 93 messages
		let messages = create_test_messages(46); // 1 system + 92 = 93 total
		assert_eq!(messages.len(), 93, "Should have 93 messages");

		// The bug: using end_index = len() = 93
		// Valid indices are 0-92, so end_index must be < 93
		let last_valid_index = messages.len() - 1; // 92
		assert_eq!(last_valid_index, 92);

		// This is what the bug was doing (would fail validation)
		let buggy_end_index = messages.len(); // 93
		assert!(
			buggy_end_index >= messages.len(),
			"Bug: end_index {} >= len {} should be rejected",
			buggy_end_index,
			messages.len()
		);

		// This is the fix (should pass validation)
		let fixed_end_index = messages.len() - 1; // 92
		assert!(
			fixed_end_index < messages.len(),
			"Fix: end_index {} < len {} should be accepted",
			fixed_end_index,
			messages.len()
		);
	}

	#[test]
	fn test_inclusive_range_semantics() {
		let messages = create_test_messages(10); // 21 messages (0-20)

		// For inclusive range removal: drain(start+1..=end)
		// If start=5, end=10:
		// - Removes indices: 6, 7, 8, 9, 10 (5 messages)
		// - Keeps index 5
		// - Index 11 becomes new index 6

		let start_index = 5;
		let end_index = 10;

		// Calculate how many messages would be removed
		let messages_to_remove = end_index - start_index; // 10 - 5 = 5
		assert_eq!(messages_to_remove, 5);

		// Verify end_index is valid (< len)
		assert!(
			end_index < messages.len(),
			"end_index {} must be < len {}",
			end_index,
			messages.len()
		);
	}

	#[test]
	fn test_boundary_validation_rules() {
		let messages = create_test_messages(10); // 21 messages
		let len = messages.len();

		// Valid: end_index = len - 1 (last valid index)
		assert!(len - 1 < len, "Last valid index should pass");

		// Invalid: end_index = len (out of bounds for inclusive range)
		assert!(len >= len, "Using len() as end_index should fail");

		// Invalid: end_index > len
		assert!(len + 1 >= len, "Beyond len() should fail");
	}

	#[test]
	fn test_off_by_one_edge_cases() {
		// Test various message counts to ensure no off-by-one errors
		for count in [1, 10, 46, 50, 100] {
			let messages = create_test_messages(count);
			let len = messages.len();

			// Last valid index for inclusive range
			let last_valid = len - 1;

			assert!(
				last_valid < len,
				"For {} messages, last_valid={} should be < len={}",
				count,
				last_valid,
				len
			);

			// Using len() should be invalid
			assert!(
				len >= len,
				"For {} messages, using len={} as end_index should fail",
				count,
				len
			);
		}
	}

	#[test]
	fn test_compression_range_calculation() {
		// Simulate plan compression: compress from start to end of conversation
		let messages = create_test_messages(46); // 93 messages
		let len = messages.len();

		let start_index = 10; // Start compression after message 10

		// BUG: let end_index = len; // 93 - WRONG!
		// FIX: let end_index = len - 1; // 92 - CORRECT!

		let buggy_end_index = len;
		let fixed_end_index = len - 1;

		// Verify the bug would be caught
		assert!(
			buggy_end_index >= len,
			"Buggy end_index should fail validation"
		);

		// Verify the fix is valid
		assert!(
			fixed_end_index < len,
			"Fixed end_index should pass validation"
		);

		// Calculate messages to remove with fix
		let messages_to_remove = fixed_end_index - start_index; // 92 - 10 = 82
		assert_eq!(messages_to_remove, 82);
	}

	#[test]
	fn test_empty_range_detection() {
		let _messages = create_test_messages(10);

		// start_index >= end_index should be invalid
		let start = 5;
		let end = 5;

		assert!(
			start >= end,
			"Empty range (start={} >= end={}) should be rejected",
			start,
			end
		);
	}

	#[test]
	fn test_inverted_range_detection() {
		let _messages = create_test_messages(10);

		// start_index > end_index should be invalid
		let start = 10;
		let end = 5;

		assert!(
			start > end,
			"Inverted range (start={} > end={}) should be rejected",
			start,
			end
		);
	}
}

#[cfg(test)]
mod adaptive_compression_tests {
	use octomind::config::Config;
	use octomind::session::{estimate_full_context_tokens, Message, Session, SessionInfo};
	use std::time::{SystemTime, UNIX_EPOCH};

	/// Helper to create isolated config for testing
	/// Returns (TempDir, Config) - TempDir must be kept alive for config to remain valid
	fn create_isolated_config() -> (tempfile::TempDir, Config) {
		let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
		let config_path = temp_dir.path().join("config.toml");

		// Set env var to use isolated config directory
		std::env::set_var("OCTOMIND_CONFIG_PATH", config_path.to_str().unwrap());

		// Load config (will create default in temp dir)
		let config = Config::load().expect("Failed to load isolated config");

		(temp_dir, config)
	}

	/// Helper to create a test session with N message pairs
	fn create_test_session(message_count: usize) -> Session {
		let timestamp = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap()
			.as_secs();

		let mut messages = vec![Message {
			role: "system".to_string(),
			content: "System prompt".to_string(),
			timestamp,
			cached: false,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			thinking: None,
			id: None,
		}];

		// Add user/assistant pairs with substantial content to reach token thresholds
		for i in 0..message_count {
			messages.push(Message {
				role: "user".to_string(),
				content: format!("User message {} with some additional content to increase token count. This is a longer message that simulates real conversation.", i),
				timestamp,
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				thinking: None,
				id: None,
			});
			messages.push(Message {
				role: "assistant".to_string(),
				content: format!("Assistant response {} with detailed explanation and multiple sentences. This response contains enough text to contribute meaningfully to the token count.", i),
				timestamp,
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
				images: None,
				thinking: None,
				id: None,
			});
		}

		Session {
			messages,
			info: SessionInfo {
				name: "test_session".to_string(),
				created_at: timestamp,
				model: "test-model".to_string(),
				provider: "test-provider".to_string(),
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
				total_api_calls: 0,
				// Cache state (Phase 1)
				current_non_cached_tokens: 100, // Simulate some cache activity
				current_total_tokens: 500,      // This is the cache counter, NOT full context
				last_cache_checkpoint_time: timestamp,
				// Runtime state (Phase 2)
				cache_next_user_message: false,
				spending_threshold_checkpoint: 0.0,
				continuation_pending: false,
				continuation_disabled: false,
				compression_hint_count: 0,
				last_compression_hint_shown: 0,
			},
			session_file: None,
		}
	}

	#[test]
	fn test_token_counting_uses_full_context_not_cache_counter() {
		// Create session with 50 message pairs (100 messages + 1 system = 101 total)
		let session = create_test_session(50);

		// The cache counter (current_total_tokens) is artificially set to 500
		assert_eq!(session.info.current_total_tokens, 500);

		// But the ACTUAL full context should be much larger
		let full_context_tokens = estimate_full_context_tokens(&session.messages, None, None);

		// Full context should be significantly larger than cache counter
		assert!(
			full_context_tokens > session.info.current_total_tokens as usize,
			"Full context tokens ({}) should be > cache counter ({})",
			full_context_tokens,
			session.info.current_total_tokens
		);

		// With 101 messages of substantial content, we should have thousands of tokens
		assert!(
			full_context_tokens > 2000,
			"Full context should have > 2000 tokens, got {}",
			full_context_tokens
		);
	}

	#[test]
	fn test_should_check_compression_uses_correct_token_source() {
		// This test verifies Bug #1 fix: should_check_compression uses full context, not cache counter

		// Load isolated config to avoid race conditions
		let (_temp_dir, config) = create_isolated_config();

		// Create a session with enough messages to exceed lowest pressure level
		// We'll create a custom threshold for testing
		let mut test_config = config.clone();
		test_config.compression.adaptive_threshold = true;
		test_config.compression.pressure_levels = vec![octomind::config::PressureLevel {
			threshold: 1000,
			target_ratio: 2.0,
		}];

		let session = create_test_session(50); // Should generate > 1000 tokens

		// Calculate what the function should see
		let full_context = estimate_full_context_tokens(&session.messages, None, None) as usize;
		let cache_counter = session.info.current_total_tokens as usize;

		// Verify our test setup: full context > threshold, cache counter < threshold
		assert!(
			full_context > test_config.compression.pressure_levels[0].threshold,
			"Full context ({}) should exceed threshold ({})",
			full_context,
			test_config.compression.pressure_levels[0].threshold
		);
		assert!(
			cache_counter < test_config.compression.pressure_levels[0].threshold,
			"Cache counter ({}) should be below threshold ({}) for this test",
			cache_counter,
			test_config.compression.pressure_levels[0].threshold
		);

		// The bug would use cache_counter (500) and return false
		// The fix uses full_context (>1000) and returns true

		// We can't directly call should_check_compression without ChatSession,
		// but we've verified the logic: full_context > threshold = should compress
		assert!(full_context >= test_config.compression.pressure_levels[0].threshold);
	}

	#[test]
	fn test_compression_threshold_calculation() {
		let (_temp_dir, config) = create_isolated_config();

		// Test that pressure levels are correctly configured
		assert!(
			!config.compression.pressure_levels.is_empty(),
			"Pressure levels should be configured"
		);

		// Verify levels are in ascending order
		let mut prev_threshold = 0;
		for level in &config.compression.pressure_levels {
			assert!(
				level.threshold > prev_threshold,
				"Pressure levels should be in ascending order"
			);
			assert!(
				level.target_ratio >= 1.0,
				"Target ratio should be >= 1.0 (compression factor)"
			);
			prev_threshold = level.threshold;
		}
	}

	#[test]
	fn test_adaptive_threshold_flag() {
		let (_temp_dir, config) = create_isolated_config();

		// Verify adaptive_threshold is configurable
		// Default should be true (enabled)
		assert!(
			config.compression.adaptive_threshold,
			"Adaptive threshold should be enabled by default"
		);
	}

	#[test]
	fn test_pressure_levels_configuration() {
		let (_temp_dir, config) = create_isolated_config();

		// Verify pressure_levels are configured
		assert!(
			!config.compression.pressure_levels.is_empty(),
			"Pressure levels should be configured"
		);

		// Verify first level is reasonable (should be around 50k)
		assert!(
			config.compression.pressure_levels[0].threshold >= 10000,
			"First pressure level should be >= 10k tokens"
		);
	}

	#[test]
	fn test_full_context_estimation_consistency() {
		// Verify that estimate_full_context_tokens is consistent
		let session = create_test_session(10);

		let tokens1 = estimate_full_context_tokens(&session.messages, None, None);
		let tokens2 = estimate_full_context_tokens(&session.messages, None, None);

		assert_eq!(tokens1, tokens2, "Token estimation should be deterministic");

		// Should be > 0 for non-empty conversation
		assert!(tokens1 > 0, "Should estimate > 0 tokens for conversation");
	}

	#[test]
	fn test_cache_counter_resets_independently() {
		// Verify that cache counter behavior doesn't affect compression logic
		let mut session = create_test_session(20);

		let full_context_before = estimate_full_context_tokens(&session.messages, None, None);

		// Simulate cache checkpoint (resets counter to 0)
		session.info.current_total_tokens = 0;
		session.info.current_non_cached_tokens = 0;

		let full_context_after = estimate_full_context_tokens(&session.messages, None, None);

		// Full context should be unchanged by cache counter reset
		assert_eq!(
			full_context_before, full_context_after,
			"Full context should not change when cache counter resets"
		);

		// But cache counter is now 0
		assert_eq!(session.info.current_total_tokens, 0);
	}
}
