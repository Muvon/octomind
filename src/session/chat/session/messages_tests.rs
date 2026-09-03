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
fn active_memory_pack_materializes_once_and_can_be_restored_or_cleared() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.set_active_memory_pack(Some("memory pack".into()));
	session.ensure_active_memory_pack_message();
	session.ensure_active_memory_pack_message();
	assert_eq!(
		session
			.session
			.messages
			.iter()
			.filter(|message| message.name.as_deref() == Some("__active_memory_pack"))
			.count(),
		1
	);

	session.remove_active_memory_pack_message();
	assert!(session.session.messages.is_empty());
	session.ensure_active_memory_pack_message();
	assert_eq!(session.session.messages.len(), 1);
	session.recalled_refs.push((
		"M1".into(),
		"memory".into(),
		"assistant".into(),
		"project".into(),
	));
	session.used_memory_ids.insert("M1".into());
	session.clear_active_memory_pack();
	assert!(session.session.messages.is_empty());
	assert!(session.recalled_refs.is_empty());
	assert!(session.used_memory_ids.is_empty());
}

#[test]
fn message_lifecycle_persists_each_role_and_runtime_summary() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("messages.jsonl.zst");
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(path.clone());
	session.session.info.name = "message-lifecycle".into();
	let config = crate::session::chat::test_support::fake_provider_config();

	session.add_system_message("system prompt").unwrap();
	session.add_user_message("user request").unwrap();
	session
		.add_system_managed_turn_message("<system-note>event</system-note>")
		.unwrap();
	session
		.add_tool_message("tool result", "call-1", "view", &config)
		.unwrap();
	session
		.add_assistant_message("assistant answer", None, &config, "assistant")
		.unwrap();
	session.save().unwrap();

	assert!(path.exists());
	assert_eq!(session.session.messages[0].role, "system");
	assert_eq!(session.session.messages[1].role, "user");
	assert_eq!(session.session.messages[2].role, "user");
	assert_eq!(session.session.messages[3].role, "tool");
	assert_eq!(session.session.messages[4].role, "assistant");
	assert_eq!(session.last_response, "assistant answer");
	assert_eq!(session.turn_answers, vec!["assistant answer"]);
}

#[test]
fn disabled_spending_limits_continue_and_request_checkpoint_tracks_cost() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = crate::session::chat::test_support::fake_provider_config();
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = -1.0;
	session.session.info.total_cost = 2.5;
	assert!(session.check_spending_threshold(&config).unwrap());
	assert!(session.check_request_spending_threshold(&config).unwrap());
	session.start_request_spending_tracking();
	assert_eq!(session.request_spending_checkpoint, 2.5);
}

#[test]
fn spending_thresholds_stop_execution_only_when_exceeded() {
	let mut session = ChatSession::for_tests(Vec::new());
	let mut config = crate::session::chat::test_support::fake_provider_config();

	// Disabled thresholds never gate
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.0;
	assert!(session.check_spending_threshold(&config).unwrap());
	assert!(session.check_request_spending_threshold(&config).unwrap());

	// Under threshold → continue
	config.max_session_spending_threshold = 1.0;
	session.session.info.total_cost = 0.5;
	assert!(session.check_spending_threshold(&config).unwrap());

	// Over threshold with non-interactive stdin → auto-decline
	session.session.info.total_cost = 2.0;
	assert!(!session.check_spending_threshold(&config).unwrap());

	// Request-level threshold stops the request
	config.max_request_spending_threshold = 0.5;
	session.start_request_spending_tracking();
	session.session.info.total_cost += 1.0;
	assert!(!session.check_request_spending_threshold(&config).unwrap());
}

#[test]
fn user_message_resets_turn_state_and_cache_flag() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.cache_next_user_message = true;
	session.add_user_message("hello").unwrap();
	assert_eq!(session.session.messages.len(), 1);
	assert_eq!(session.session.messages[0].role, "user");
	// ollama:fake-model does not support caching → flag reset, no marker applied
	assert!(!session.cache_next_user_message);
	assert!(session.completion_gate_eligible);
}

#[test]
fn system_managed_turn_message_is_wrapped_and_not_turn_owned() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = true;
	session
		.add_system_managed_turn_message("background event")
		.unwrap();
	let message = session.session.messages.last().unwrap();
	assert_eq!(message.role, "user");
	assert!(message.content.contains("background event"));
	assert!(!session.completion_gate_eligible);
}

#[test]
fn deferred_turn_hands_eligibility_to_the_resuming_delivery_once() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.completion_gate_eligible = true;
	session.gate_deferred = true;
	session
		.add_system_managed_turn_message("job result")
		.unwrap();
	assert!(session.completion_gate_eligible);
	assert!(!session.gate_deferred);
	session
		.add_system_managed_turn_message("schedule fired")
		.unwrap();
	assert!(!session.completion_gate_eligible);
	session.gate_deferred = true;
	session.add_user_message("new request").unwrap();
	assert!(!session.gate_deferred);
}

#[test]
fn assistant_message_tracks_usage_and_cost_from_exchange() {
	let mut session = ChatSession::for_tests(Vec::new());
	let config = crate::session::chat::test_support::fake_provider_config();
	let usage = crate::session::TokenUsage {
		input_tokens: 100,
		cache_read_tokens: 20,
		cache_write_tokens: 10,
		output_tokens: 50,
		reasoning_tokens: 5,
		total_tokens: 185,
		cost: Some(0.5),
		request_time_ms: Some(250),
	};
	let exchange = crate::session::ProviderExchange::new(
		serde_json::json!({}),
		serde_json::json!({"usage": {"cost": 0.25}}),
		Some(usage),
		"test",
	);
	session
		.add_assistant_message("answer", Some(exchange), &config, "assistant")
		.unwrap();
	assert_eq!(session.session.messages.last().unwrap().role, "assistant");
	assert_eq!(session.last_response, "answer");
	assert_eq!(session.session.info.total_api_calls, 1);
	// Normalized usage.cost wins over the raw response cost
	assert!((session.session.info.total_cost - 0.5).abs() < 1e-9);
	assert_eq!(session.session.info.total_api_time_ms, 250);
	assert_eq!(session.turn_answers, vec!["answer".to_string()]);
}
