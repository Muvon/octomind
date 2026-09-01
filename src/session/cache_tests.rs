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

#[test]
fn test_cache_manager_creation() {
	let manager = CacheManager::new();
	assert_eq!(manager.max_content_markers, 2);
}

#[test]
fn test_automatic_cache_markers() {
	let manager = CacheManager::new();
	let mut messages = vec![
		Message {
			role: "system".to_string(),
			content: "You are an AI assistant".to_string(),
			timestamp: 0,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		Message {
			role: "user".to_string(),
			content: "Hello".to_string(),
			timestamp: 0,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	manager.add_automatic_cache_markers(&mut messages, true, true);

	// System message should be cached
	assert!(messages[0].cached);
	// User message should not be automatically cached
	assert!(!messages[1].cached);
}

#[test]
fn rolling_content_markers_preserve_previous_and_advance_to_current() {
	let manager = CacheManager::new();
	let mut session = Session::new(
		"cache-roll".to_string(),
		"anthropic:claude-sonnet-4-6".to_string(),
	);
	session.messages = vec![
		Message::default(),
		Message {
			role: "user".to_string(),
			content: "old boundary".to_string(),
			cached: true,
			..Default::default()
		},
		Message {
			role: "user".to_string(),
			content: "previous boundary".to_string(),
			cached: true,
			..Default::default()
		},
		Message {
			role: "assistant".to_string(),
			content: "work".to_string(),
			..Default::default()
		},
		Message {
			role: "user".to_string(),
			content: "current boundary".to_string(),
			..Default::default()
		},
	];

	assert!(manager
		.apply_cache_to_message(&mut session, 4, true)
		.unwrap());
	let markers: Vec<usize> = session
		.messages
		.iter()
		.enumerate()
		.filter(|(_, message)| message.cached && message.role != "system")
		.map(|(index, _)| index)
		.collect();

	assert_eq!(markers, vec![2, 4]);
	assert!(!session.messages[1].cached, "oldest boundary must advance");
}

// ── CacheMarker / CacheMarkerType ───────────────────────────────────────────

#[test]
fn cache_manager_default_matches_new() {
	assert_eq!(
		CacheManager::default().max_content_markers,
		CacheManager::new().max_content_markers
	);
}

#[test]
fn cache_marker_type_equality_and_distinction() {
	assert_eq!(CacheMarkerType::System, CacheMarkerType::System);
	assert_ne!(CacheMarkerType::System, CacheMarkerType::Tools);
	assert_ne!(CacheMarkerType::Tools, CacheMarkerType::Content);
	assert_ne!(CacheMarkerType::System, CacheMarkerType::Content);
	assert!(format!("{:?}", CacheMarkerType::Content).contains("Content"));
}

#[test]
fn cache_marker_serde_roundtrip_preserves_all_fields() {
	let marker = CacheMarker {
		message_index: 7,
		marker_type: CacheMarkerType::Content,
		automatic: false,
		timestamp: 1_700_000_000,
	};
	let json = serde_json::to_string(&marker).expect("serialize marker");
	let back: CacheMarker = serde_json::from_str(&json).expect("deserialize marker");
	assert_eq!(back.message_index, 7);
	assert_eq!(back.marker_type, CacheMarkerType::Content);
	assert!(!back.automatic);
	assert_eq!(back.timestamp, 1_700_000_000);
}

#[test]
fn cache_marker_type_serde_uses_variant_names() {
	assert_eq!(
		serde_json::to_string(&CacheMarkerType::System).unwrap(),
		"\"System\""
	);
	assert_eq!(
		serde_json::to_string(&CacheMarkerType::Tools).unwrap(),
		"\"Tools\""
	);
	assert_eq!(
		serde_json::to_string(&CacheMarkerType::Content).unwrap(),
		"\"Content\""
	);
	let parsed: CacheMarkerType = serde_json::from_str("\"Tools\"").unwrap();
	assert_eq!(parsed, CacheMarkerType::Tools);
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn msg(role: &str, content: &str, cached: bool) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		cached,
		..Default::default()
	}
}

fn test_session(model: &str) -> Session {
	Session::new("cache-tests".to_string(), model.to_string())
}

fn test_config() -> Config {
	toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template")
}

// ── add_automatic_cache_markers ──────────────────────────────────────────────

#[test]
fn automatic_markers_require_caching_support() {
	let manager = CacheManager::new();
	let mut messages = vec![msg("system", "sys prompt", false), msg("user", "hi", false)];
	manager.add_automatic_cache_markers(&mut messages, true, false);
	assert!(
		!messages[0].cached,
		"no markers when the provider cannot cache"
	);
	assert!(!messages[1].cached);
}

#[test]
fn automatic_markers_on_empty_messages_is_noop() {
	let manager = CacheManager::new();
	let mut messages: Vec<Message> = Vec::new();
	manager.add_automatic_cache_markers(&mut messages, true, true);
	assert!(messages.is_empty());
}

#[test]
fn automatic_markers_cache_first_and_last_system_message_with_tools() {
	let manager = CacheManager::new();
	let mut messages = vec![
		msg("system", "base prompt", false),
		msg("user", "hi", false),
		msg("system", "prompt + tool definitions", false),
	];

	// Without tools only the first system message is cached.
	manager.add_automatic_cache_markers(&mut messages, false, true);
	assert!(messages[0].cached);
	assert!(
		!messages[2].cached,
		"last system only cached when tools exist"
	);

	// With tools the LAST system message (tool definitions) is cached too.
	messages[0].cached = false;
	manager.add_automatic_cache_markers(&mut messages, true, true);
	assert!(messages[0].cached);
	assert!(
		messages[2].cached,
		"last system message must be cached with tools"
	);
	assert!(!messages[1].cached, "user content is never auto-cached");
}

#[test]
fn automatic_markers_skip_non_system_first_message() {
	let manager = CacheManager::new();
	let mut messages = vec![
		msg("user", "hi", false),
		msg("system", "late system", false),
	];
	manager.add_automatic_cache_markers(&mut messages, true, true);
	assert!(
		!messages[0].cached,
		"first-message rule only applies to the system role"
	);
	assert!(
		messages[1].cached,
		"last system message still cached with tools"
	);
}

#[test]
fn automatic_markers_are_idempotent_on_cached_system() {
	let manager = CacheManager::new();
	let mut messages = vec![msg("system", "sys", true), msg("user", "hi", false)];
	manager.add_automatic_cache_markers(&mut messages, true, true);
	assert!(messages[0].cached);
	assert!(!messages[1].cached);
}

// ── check_and_apply_auto_cache_threshold ─────────────────────────────────────

#[test]
fn auto_threshold_noop_when_disabled_or_no_messages() {
	let manager = CacheManager::new();
	let config = test_config();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	assert!(!manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, false, "developer")
		.unwrap());
	session.messages = vec![msg("user", "hello", false)];
	assert!(!manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, false, "developer")
		.unwrap());
	assert!(!session.messages[0].cached);
}

#[test]
fn auto_threshold_marks_latest_uncached_boundary_then_settles() {
	let manager = CacheManager::new();
	let config = test_config();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("system", "sys", true),
		msg("user", "old", true),
		msg("assistant", "work", false),
		msg("user", "current", false),
	];

	assert!(manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
		.unwrap());
	assert!(
		session.messages[3].cached,
		"latest uncached user message becomes the boundary"
	);

	// Every user/tool message is now cached: no target left, no-op.
	assert!(!manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
		.unwrap());
}

#[test]
fn auto_threshold_prefers_the_latest_tool_message() {
	let manager = CacheManager::new();
	let config = test_config();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("user", "request", false),
		msg("assistant", "calling", false),
		msg("tool", "tool output", false),
	];
	assert!(manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
		.unwrap());
	assert!(
		session.messages[2].cached,
		"walk-back must stop at the latest tool message"
	);
	assert!(!session.messages[0].cached);
}

#[test]
fn auto_threshold_never_advances_behind_the_cached_frontier() {
	// Post-compression layout produced by align_compression_cache_markers:
	// anchor watermark (1h) + final marker, with the preserved skill and the
	// summary deliberately uncached between them. The advance must be a no-op —
	// marking a message behind the frontier would evict the anchor before its
	// 1h cache entry is ever written to the provider.
	let manager = CacheManager::new();
	let config = test_config();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("system", "sys", true),
		Message {
			cache_ttl: Some("1h".into()),
			..msg("assistant", "unchanged welcome anchor", true)
		},
		msg("user", "<skill name=\"rust\">rules</skill>", false),
		msg("assistant", "compressed summary", false),
		msg("user", "<continuation>resume</continuation>", true),
	];

	assert!(!manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
		.unwrap());
	assert!(session.messages[1].cached, "anchor watermark must survive");
	assert_eq!(session.messages[1].cache_ttl.as_deref(), Some("1h"));
	assert!(
		!session.messages[2].cached,
		"skill behind the frontier must not become a boundary"
	);
}

#[test]
fn auto_threshold_eviction_clears_stale_ttl_when_marker_rolls_forward() {
	let manager = CacheManager::new();
	let config = test_config();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		Message {
			cache_ttl: Some("1h".into()),
			..msg("assistant", "anchor", true)
		},
		msg("user", "final compacted state", true),
		msg("assistant", "tool call", false),
		msg("tool", "fresh result", false),
	];

	assert!(manager
		.check_and_apply_auto_cache_threshold(&mut session, &config, true, "developer")
		.unwrap());
	assert!(
		session.messages[3].cached,
		"marker advances to the fresh tool result past the frontier"
	);
	assert!(!session.messages[0].cached, "oldest marker is evicted");
	assert_eq!(
		session.messages[0].cache_ttl, None,
		"eviction must clear the stale TTL with the marker"
	);
	assert!(
		session.messages[1].cached,
		"previous frontier survives as marker #1"
	);
}

// ── update_token_tracking / estimate_current_session_tokens ──────────────────

#[test]
fn update_token_tracking_accumulates_lifetime_and_current_counters() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");

	manager.update_token_tracking(&mut session, 100, 40, 500, 20, 5);
	manager.update_token_tracking(&mut session, 50, 10, 250, 0, 3);

	assert_eq!(session.info.input_tokens, 150);
	assert_eq!(session.info.output_tokens, 50);
	assert_eq!(session.info.cache_read_tokens, 750);
	assert_eq!(session.info.cache_write_tokens, 20);
	assert_eq!(session.info.reasoning_tokens, 8);
	// current_total counts cached + non-cached input; non-cached only raw input.
	assert_eq!(session.info.current_total_tokens, 900);
	assert_eq!(session.info.current_non_cached_tokens, 150);
}

#[test]
fn estimate_current_session_tokens_splits_cached_and_uncached() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("system", "cached system prompt", true),
		msg("user", "uncached user turn", false),
		msg("assistant", "uncached reply", false),
	];

	let (total, non_cached) = manager.estimate_current_session_tokens(&session);
	let expected_total: u64 = session
		.messages
		.iter()
		.map(|m| crate::session::estimate_message_tokens(m) as u64)
		.sum();
	let expected_uncached: u64 = session
		.messages
		.iter()
		.filter(|m| !m.cached)
		.map(|m| crate::session::estimate_message_tokens(m) as u64)
		.sum();
	assert_eq!(total, expected_total);
	assert_eq!(non_cached, expected_uncached);
	assert!(
		non_cached < total,
		"cached message must be excluded from the non-cached estimate"
	);
}

// ── apply_cache_to_message ───────────────────────────────────────────────────

#[test]
fn apply_cache_disabled_is_noop() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("user", "hi", false)];
	session.info.current_total_tokens = 42;
	assert!(!manager
		.apply_cache_to_message(&mut session, 0, false)
		.unwrap());
	assert!(!session.messages[0].cached);
	assert_eq!(
		session.info.current_total_tokens, 42,
		"disabled path must not touch counters"
	);
}

#[test]
fn apply_cache_out_of_bounds_is_error() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("user", "hi", false)];
	let err = manager
		.apply_cache_to_message(&mut session, 1, true)
		.unwrap_err()
		.to_string();
	assert!(err.contains("out of bounds"), "unexpected error: {err}");
}

#[test]
fn apply_cache_already_cached_returns_false_without_counter_reset() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("user", "hi", true)];
	session.info.current_total_tokens = 42;
	session.info.current_non_cached_tokens = 7;
	assert!(!manager
		.apply_cache_to_message(&mut session, 0, true)
		.unwrap());
	assert_eq!(session.info.current_total_tokens, 42);
	assert_eq!(session.info.current_non_cached_tokens, 7);
}

#[test]
fn apply_cache_marks_message_and_resets_current_counters() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("user", "hi", false)];
	session.info.current_total_tokens = 500;
	session.info.current_non_cached_tokens = 300;
	let before = session.info.last_cache_checkpoint_time;

	assert!(manager
		.apply_cache_to_message(&mut session, 0, true)
		.unwrap());
	assert!(session.messages[0].cached);
	assert_eq!(
		session.info.current_total_tokens, 0,
		"checkpoint must reset the rolling total"
	);
	assert_eq!(
		session.info.current_non_cached_tokens, 0,
		"checkpoint must reset the rolling non-cached total"
	);
	assert!(session.info.last_cache_checkpoint_time >= before);
}

#[test]
fn apply_cache_keeps_existing_markers_below_the_two_marker_limit() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("user", "boundary one", true),
		msg("assistant", "work", false),
		msg("user", "boundary two target", false),
	];
	assert!(manager
		.apply_cache_to_message(&mut session, 2, true)
		.unwrap());
	assert!(
		session.messages[0].cached,
		"one existing marker is below the limit and must survive"
	);
	assert!(session.messages[2].cached);
}

// ── apply_cache_to_current_user_message ──────────────────────────────────────

#[test]
fn apply_cache_to_current_user_targets_the_last_user_message() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("user", "first turn", false),
		msg("assistant", "reply", false),
		msg("user", "latest turn", false),
	];
	assert!(manager
		.apply_cache_to_current_user_message(&mut session, true)
		.unwrap());
	assert!(!session.messages[0].cached);
	assert!(
		session.messages[2].cached,
		"the LAST user message is the cacheable boundary"
	);
}

#[test]
fn apply_cache_to_current_user_without_user_message_is_error() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("system", "sys", false), msg("assistant", "hi", false)];
	let err = manager
		.apply_cache_to_current_user_message(&mut session, true)
		.unwrap_err()
		.to_string();
	assert!(err.contains("No user message"), "unexpected error: {err}");
}

#[test]
fn apply_cache_to_current_user_respects_disabled_caching() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("user", "hi", false)];
	assert!(!manager
		.apply_cache_to_current_user_message(&mut session, false)
		.unwrap());
	assert!(!session.messages[0].cached);
}

// ── clear_content_cache_markers ──────────────────────────────────────────────

#[test]
fn clear_content_markers_clears_content_roles_but_keeps_system() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("system", "sys", true),
		Message {
			cache_ttl: Some("1h".into()),
			..msg("user", "u", true)
		},
		msg("assistant", "a", true),
		Message {
			tool_call_id: Some("call-1".into()),
			..msg("tool", "t", true)
		},
		msg("user", "already plain", false),
	];

	let cleared = manager.clear_content_cache_markers(&mut session);
	assert_eq!(
		cleared, 3,
		"user + assistant + tool markers are content markers"
	);
	assert!(session.messages[0].cached, "system marker must survive");
	assert!(!session.messages[1].cached);
	assert_eq!(
		session.messages[1].cache_ttl, None,
		"clearing a marker must also drop its TTL"
	);
	assert!(!session.messages[2].cached);
	assert!(!session.messages[3].cached);
}

// ── get_cache_statistics / get_cache_statistics_with_config ──────────────────

#[test]
fn statistics_count_markers_by_role() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![
		msg("system", "sys", true),
		msg("user", "u", true),
		msg("assistant", "a", true),
		Message {
			tool_call_id: Some("call-1".into()),
			..msg("tool", "tool result", true)
		},
		// Tool message WITHOUT tool_call_id is a tool definition, not a result.
		// Its presence also keeps the virtual-marker branch inert (tool_markers != 0).
		Message {
			tool_call_id: None,
			..msg("tool", "tool definition", true)
		},
		msg("user", "uncached", false),
	];

	let stats = manager.get_cache_statistics(&session);
	assert_eq!(stats.system_markers, 1);
	assert_eq!(
		stats.content_markers, 3,
		"user + assistant + tool RESULT are content markers"
	);
	assert_eq!(
		stats.tool_markers, 1,
		"tool message without tool_call_id is a tool marker"
	);
}

#[test]
fn statistics_virtual_tool_marker_requires_cached_system_and_tools() {
	let manager = CacheManager::new();
	let config = test_config();
	assert!(
		!config.mcp.servers.is_empty(),
		"default template ships builtin servers"
	);

	// Cached system + caching model + configured servers → virtual tool marker.
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.messages = vec![msg("system", "sys", true), msg("user", "u", false)];
	let stats = manager.get_cache_statistics_with_config(&session, Some(&config));
	assert_eq!(
		stats.tool_markers, 1,
		"tool definitions are cached alongside the system message"
	);

	// Without config a fresh cacheable session (no tokens yet) still infers tools.
	let stats = manager.get_cache_statistics(&session);
	assert_eq!(stats.tool_markers, 1);

	// No cached system message → nothing to piggyback tool caching on.
	session.messages[0].cached = false;
	let stats = manager.get_cache_statistics_with_config(&session, Some(&config));
	assert_eq!(stats.tool_markers, 0);
	assert_eq!(stats.system_markers, 0);

	// Config with zero servers → no tool definitions to cache.
	session.messages[0].cached = true;
	let mut empty = test_config();
	empty.mcp.servers.clear();
	let stats = manager.get_cache_statistics_with_config(&session, Some(&empty));
	assert_eq!(
		stats.tool_markers, 0,
		"no servers configured means no tool definitions to cache"
	);
}

#[test]
fn statistics_report_token_totals_and_cache_efficiency() {
	let manager = CacheManager::new();
	let mut session = test_session("anthropic/claude-sonnet-4-6");
	session.info.input_tokens = 100;
	session.info.output_tokens = 60;
	session.info.cache_read_tokens = 300;
	session.info.cache_write_tokens = 25;
	session.info.current_non_cached_tokens = 100;
	session.info.current_total_tokens = 400;

	let stats = manager.get_cache_statistics(&session);
	assert_eq!(stats.total_input_tokens, 400, "input + cache read");
	assert_eq!(stats.total_output_tokens, 60);
	assert_eq!(stats.total_cache_read_tokens, 300);
	assert_eq!(stats.total_cache_write_tokens, 25);
	assert_eq!(stats.current_non_cached_tokens, 100);
	assert_eq!(stats.current_total_tokens, 400);
	assert!(
		(stats.cache_efficiency - 75.0).abs() < 1e-9,
		"300 of 400 input tokens cached = 75%"
	);

	// Zero tokens must not divide by zero.
	let empty = test_session("anthropic/claude-sonnet-4-6");
	let stats = manager.get_cache_statistics(&empty);
	assert_eq!(stats.cache_efficiency, 0.0);
	assert_eq!(stats.total_input_tokens, 0);
}

// ── CacheStatistics::format_for_display ──────────────────────────────────────

#[test]
fn format_for_display_renders_empty_and_populated_states() {
	let empty = CacheStatistics {
		content_markers: 0,
		system_markers: 0,
		tool_markers: 0,
		total_cache_read_tokens: 0,
		total_cache_write_tokens: 0,
		total_input_tokens: 0,
		total_output_tokens: 0,
		current_non_cached_tokens: 0,
		current_total_tokens: 0,
		cache_efficiency: 0.0,
	};
	let text = empty.format_for_display();
	assert!(text.contains("No active cache markers"));
	assert!(text.contains("No cached tokens recorded yet"));

	let populated = CacheStatistics {
		content_markers: 2,
		system_markers: 1,
		tool_markers: 1,
		total_cache_read_tokens: 300,
		total_cache_write_tokens: 25,
		total_input_tokens: 400,
		total_output_tokens: 60,
		current_non_cached_tokens: 100,
		current_total_tokens: 400,
		cache_efficiency: 75.0,
	};
	let text = populated.format_for_display();
	assert!(text.contains("Active markers:"));
	assert!(text.contains("Overall cache efficiency"));
	assert!(text.contains("Session totals:"));
}
