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
use serde_json::json;

fn exchange_with_response(response: serde_json::Value) -> ProviderExchange {
	ProviderExchange::new(json!({}), response, None, "test")
}

#[test]
fn test_extract_original_tool_calls_unified_format() {
	let calls = json!([{"tool_name": "shell", "parameters": {"cmd": "ls"}, "tool_id": "id1"}]);
	let exchange = exchange_with_response(json!({ "tool_calls": calls }));
	assert_eq!(
		MessageHandler::extract_original_tool_calls(&exchange),
		Some(calls)
	);
}

#[test]
fn test_extract_original_tool_calls_absent() {
	let exchange = exchange_with_response(json!({"content": "plain answer"}));
	assert!(MessageHandler::extract_original_tool_calls(&exchange).is_none());
}

fn usage(
	input_tokens: u64,
	output_tokens: u64,
	cost: f64,
	request_time_ms: u64,
) -> crate::providers::TokenUsage {
	crate::providers::TokenUsage {
		input_tokens,
		cache_read_tokens: 3,
		cache_write_tokens: 4,
		output_tokens,
		reasoning_tokens: 0,
		total_tokens: input_tokens + output_tokens + 7,
		cost: Some(cost),
		request_time_ms: Some(request_time_ms),
	}
}

#[test]
fn test_extract_original_tool_calls_openai_provider_without_calls_is_none() {
	// A provider-specific exchange whose response carries no tool_calls at
	// all: octolib's openai extractor finds nothing and the handler must
	// return None (no fallback — unified format is mandatory).
	let exchange = ProviderExchange::new(
		json!({}),
		json!({"choices": [{"message": {"role": "assistant", "content": "hi"}}]}),
		None,
		"openai",
	);
	assert!(MessageHandler::extract_original_tool_calls(&exchange).is_none());
}

#[test]
fn test_add_assistant_message_with_tool_calls_persists_and_tracks_usage() {
	let dir = tempfile::tempdir().unwrap();
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(dir.path().join("session.jsonl"));

	let calls = json!([{"tool_name": "shell", "parameters": {"cmd": "ls"}, "tool_id": "id1"}]);
	let exchange = ProviderExchange::new(
		json!({}),
		json!({"tool_calls": calls}),
		Some(usage(10, 5, 0.02, 120)),
		"test",
	);

	MessageHandler::add_assistant_message_with_tool_calls(
		&mut session,
		"working on it",
		&exchange,
		Some("resp-1".to_string()),
	)
	.expect("add assistant message");

	// The assistant message is in memory with the tool calls and id preserved
	let last = session.session.messages.last().expect("message pushed");
	assert_eq!(last.role, "assistant");
	assert_eq!(last.content, "working on it");
	assert_eq!(last.tool_calls, Some(calls));
	assert_eq!(last.id.as_deref(), Some("resp-1"));
	assert_eq!(session.last_response, "working on it");

	// Usage flowed into the session bookkeeping
	assert_eq!(session.session.info.total_api_calls, 1);
	assert!((session.session.info.total_cost - 0.02).abs() < f64::EPSILON);
	assert_eq!(session.session.info.total_api_time_ms, 120);
	assert_eq!(session.session.info.input_tokens, 10);
	assert_eq!(session.session.info.output_tokens, 5);

	// The message was persisted BEFORE being pushed: the log line carries the
	// tool calls, so a resumed session can rebuild the pairing.
	let bytes = std::fs::read(dir.path().join("session.jsonl")).expect("session log");
	let decoded = zstd::decode_all(std::io::Cursor::new(&bytes)).expect("log is zstd");
	let log = String::from_utf8(decoded).expect("log is UTF-8");
	assert!(log.contains("working on it"), "log: {log}");
	assert!(log.contains("tool_calls"), "log: {log}");
}

#[test]
fn test_add_assistant_message_without_file_or_usage_skips_persist_and_tracking() {
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	assert!(session.session.session_file.is_none());

	let exchange = ProviderExchange::new(json!({}), json!({"content": "plain"}), None, "test");

	MessageHandler::add_assistant_message_with_tool_calls(
		&mut session,
		"no tools here",
		&exchange,
		None,
	)
	.expect("add assistant message");

	assert_eq!(session.session.messages.len(), 1);
	assert!(session.session.messages[0].tool_calls.is_none());
	assert_eq!(session.session.messages[0].id, None);
	// No usage on the exchange: counters stay untouched
	assert_eq!(session.session.info.total_api_calls, 0);
	assert_eq!(session.session.info.total_cost, 0.0);
	assert_eq!(session.session.info.total_api_time_ms, 0);
}
