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
fn test_model_purpose_contract_strings() {
	// These exact strings are a cross-repo CONTRACT: octohub routes by them
	// and the panel renders a picker per purpose. Renaming one silently
	// breaks purpose routing for every deployed CLI — this test is the tripwire.
	assert_eq!(MODEL_PURPOSE_HEADER, "X-Model-Purpose");
	assert_eq!(ModelPurpose::Main.as_str(), "main");
	assert_eq!(ModelPurpose::Compression.as_str(), "compression");
	assert_eq!(ModelPurpose::Supervisor.as_str(), "supervisor");
	// Untagged calls are MAIN traffic — session turns must never silently
	// become something a cheaper purpose route would catch.
	assert_eq!(ModelPurpose::default(), ModelPurpose::Main);
}

#[test]
fn test_thinking_block_conversion() {
	// Test that ThinkingBlock can be serialized to JSON and back
	let thinking_block = ThinkingBlock {
		content: "Test thinking content".to_string(),
		tokens: 42,
	};

	// Serialize to JSON (simulating storage in session)
	let json_value = serde_json::to_value(&thinking_block).expect("Failed to serialize");
	println!("Serialized: {}", json_value);

	// Deserialize back (simulating loading from session)
	let deserialized: ThinkingBlock =
		serde_json::from_value(json_value).expect("Failed to deserialize");
	println!("Deserialized: {:?}", deserialized);

	assert_eq!(deserialized.content, "Test thinking content");
	assert_eq!(deserialized.tokens, 42);
}

// ── convert_to_generic_tool_calls ──────────────────────────────

#[test]
fn test_generic_tool_calls_passthrough_unified_format() {
	let value = serde_json::json!([
		{ "id": "call_1", "name": "read", "arguments": {"path": "/x"} },
		{
			"id": "call_2",
			"name": "write",
			"arguments": {},
			"meta": {"origin": "test"}
		}
	]);
	let calls = convert_to_generic_tool_calls(&value).expect("unified format must pass through");
	assert_eq!(calls.len(), 2);
	assert_eq!(calls[0].id, "call_1");
	assert_eq!(calls[0].name, "read");
	assert_eq!(calls[0].arguments, serde_json::json!({"path": "/x"}));
	assert!(calls[0].meta.is_none());
	assert_eq!(
		calls[1].meta.as_ref().and_then(|m| m.get("origin")),
		Some(&serde_json::json!("test")),
		"meta must survive the passthrough"
	);
}

#[test]
fn test_openai_format_converts_with_parsed_arguments() {
	let value = serde_json::json!([
		{
			"id": "call_1",
			"type": "function",
			"function": {
				"name": "read",
				"arguments": "{\"path\": \"/x\"}"
			}
		}
	]);
	let calls = convert_to_generic_tool_calls(&value).expect("OpenAI format must convert");
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].id, "call_1");
	assert_eq!(calls[0].name, "read");
	assert_eq!(calls[0].arguments, serde_json::json!({"path": "/x"}));
}

#[test]
fn test_openai_format_preserves_root_meta_when_present() {
	// WITH meta at the tool-call root → preserved verbatim, like the
	// unified branch
	let with_meta = serde_json::json!([
		{
			"id": "call_1",
			"type": "function",
			"function": {"name": "read", "arguments": "{}"},
			"meta": {"origin": "session"}
		}
	]);
	let calls =
		convert_to_generic_tool_calls(&with_meta).expect("OpenAI format with meta must convert");
	assert_eq!(calls.len(), 1);
	let expected_meta: serde_json::Map<String, serde_json::Value> =
		serde_json::from_value(with_meta[0]["meta"].clone()).unwrap();
	assert_eq!(calls[0].meta, Some(expected_meta));

	// WITHOUT meta → None
	let without_meta = serde_json::json!([
		{
			"id": "call_2",
			"type": "function",
			"function": {"name": "read", "arguments": "{}"}
		}
	]);
	let calls = convert_to_generic_tool_calls(&without_meta)
		.expect("OpenAI format without meta must convert");
	assert!(calls[0].meta.is_none());
}

#[test]
fn test_openai_format_blank_arguments_become_empty_object() {
	for args in ["", "   "] {
		let value = serde_json::json!([
			{
				"id": "call_1",
				"type": "function",
				"function": {"name": "ping", "arguments": args}
			}
		]);
		let calls = convert_to_generic_tool_calls(&value)
			.expect("blank arguments must convert to an empty object");
		assert_eq!(calls[0].arguments, serde_json::json!({}), "args = {args:?}");
	}
}

#[test]
fn test_openai_format_missing_function_errors() {
	let value = serde_json::json!([{ "id": "call_1", "type": "function" }]);
	let err = convert_to_generic_tool_calls(&value).expect_err("missing function must fail");
	match err {
		octolib::MessageError::MissingToolField { field } => {
			assert_eq!(field, "function", "unexpected field name: {field}")
		}
		other => panic!("expected MissingToolField, got {other:?}"),
	}
}

#[test]
fn test_openai_format_missing_id_name_or_arguments_errors() {
	let value = serde_json::json!([
		{
			"type": "function",
			"function": {"name": "read", "arguments": "{}"}
		}
	]);
	let err = convert_to_generic_tool_calls(&value).expect_err("missing id must fail");
	match err {
		octolib::MessageError::MissingToolField { field } => assert_eq!(
			field, "function.{id|name|arguments}",
			"unexpected field name: {field}"
		),
		other => panic!("expected MissingToolField, got {other:?}"),
	}
}

#[test]
fn test_openai_format_invalid_json_arguments_error() {
	let value = serde_json::json!([
		{
			"id": "call_1",
			"type": "function",
			"function": {"name": "read", "arguments": "not json at all"}
		}
	]);
	let err = convert_to_generic_tool_calls(&value).expect_err("invalid JSON arguments must fail");
	assert!(
		matches!(err, octolib::MessageError::ToolCallsError(_)),
		"expected ToolCallsError, got {err:?}"
	);
}

#[test]
fn test_non_array_tool_calls_root_errors() {
	for value in [serde_json::json!({"foo": 1}), serde_json::json!("string")] {
		let err = convert_to_generic_tool_calls(&value).expect_err("non-array root must fail");
		match err {
			octolib::MessageError::MissingToolField { field } => assert_eq!(
				field, "tool_calls (root must be Vec<GenericToolCall> or OpenAI array)",
				"unexpected field name: {field}"
			),
			other => panic!("expected MissingToolField, got {other:?}"),
		}
	}
}

#[test]
fn test_empty_tool_calls_array_returns_empty_vec() {
	let calls = convert_to_generic_tool_calls(&serde_json::json!([]))
		.expect("empty array is valid unified format");
	assert!(calls.is_empty());
}

// ── convert_message_to_octolib ─────────────────────────────────

fn msg(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp: 1_700_000_000,
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

#[test]
fn test_convert_preserves_role_content_and_timestamp() {
	for role in ["user", "assistant", "system"] {
		let converted = convert_message_to_octolib(&msg(role, "hello"))
			.unwrap_or_else(|e| panic!("{role} must convert: {e:?}"));
		assert_eq!(converted.role, role);
		assert_eq!(converted.content, "hello");
		assert_eq!(converted.timestamp, 1_700_000_000);
	}
}

#[test]
fn test_convert_tool_message_requires_call_id_and_name() {
	let missing_id = Message {
		name: Some("tool_a".to_string()),
		..msg("tool", "result")
	};
	let err = convert_message_to_octolib(&missing_id)
		.expect_err("tool message without tool_call_id must fail");
	match err {
		octolib::MessageError::MissingToolField { field } => {
			assert_eq!(field, "tool_call_id")
		}
		other => panic!("expected MissingToolField, got {other:?}"),
	}

	let missing_name = Message {
		tool_call_id: Some("call_1".to_string()),
		..msg("tool", "result")
	};
	let err =
		convert_message_to_octolib(&missing_name).expect_err("tool message without name must fail");
	match err {
		octolib::MessageError::MissingToolField { field } => {
			assert_eq!(field, "name")
		}
		other => panic!("expected MissingToolField, got {other:?}"),
	}
}

#[test]
fn test_convert_complete_tool_message() {
	let message = Message {
		tool_call_id: Some("call_1".to_string()),
		name: Some("tool_a".to_string()),
		..msg("tool", "result")
	};
	let converted =
		convert_message_to_octolib(&message).expect("complete tool message must convert");
	assert_eq!(converted.role, "tool");
	assert_eq!(converted.tool_call_id.as_deref(), Some("call_1"));
	assert_eq!(converted.name.as_deref(), Some("tool_a"));
}

#[test]
fn test_convert_rejects_invalid_role() {
	let err = convert_message_to_octolib(&msg("bogus", "x")).expect_err("unknown role must fail");
	match err {
		octolib::MessageError::InvalidRole { role } => assert_eq!(role, "bogus"),
		other => panic!("expected InvalidRole, got {other:?}"),
	}
}

#[test]
fn test_convert_assistant_tool_calls_become_generic() {
	let message = Message {
		tool_calls: Some(serde_json::json!([
			{
				"id": "call_9",
				"type": "function",
				"function": {
					"name": "shell",
					"arguments": "{\"cmd\": \"ls\"}"
				}
			}
		])),
		..msg("assistant", "")
	};
	let converted =
		convert_message_to_octolib(&message).expect("assistant with tool calls must convert");
	let raw = converted.tool_calls.expect("tool_calls must be set");
	let calls: Vec<octolib::llm::GenericToolCall> = serde_json::from_value(raw)
		.expect("stored tool_calls must be unified GenericToolCall JSON");
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].name, "shell");
	assert_eq!(calls[0].arguments, serde_json::json!({"cmd": "ls"}));
}

#[test]
fn test_convert_assistant_malformed_tool_calls_fail() {
	let message = Message {
		tool_calls: Some(serde_json::json!({"bad": 1})),
		..msg("assistant", "")
	};
	assert!(
		convert_message_to_octolib(&message).is_err(),
		"malformed tool_calls must fail the request, not panic"
	);
}

#[test]
fn test_convert_cache_marker_and_ttl() {
	let cached = Message {
		cached: true,
		cache_ttl: Some("5m".to_string()),
		..msg("system", "sysprompt")
	};
	let converted = convert_message_to_octolib(&cached).expect("cached message must convert");
	assert!(converted.cached, "cache marker must survive");
	assert_eq!(converted.cache_ttl.as_deref(), Some("5m"));

	// TTL without the cache marker is ignored — only cached messages carry it.
	let uncached = Message {
		cache_ttl: Some("5m".to_string()),
		..msg("system", "sysprompt")
	};
	let converted = convert_message_to_octolib(&uncached).expect("uncached message must convert");
	assert!(!converted.cached);
	assert_eq!(
		converted.cache_ttl, None,
		"TTL must not apply without the marker"
	);
}

#[test]
fn test_convert_message_id_propagates() {
	let message = Message {
		id: Some("resp_123".to_string()),
		..msg("assistant", "hi")
	};
	let converted = convert_message_to_octolib(&message).expect("must convert");
	assert_eq!(converted.id.as_deref(), Some("resp_123"));
}

#[test]
fn test_convert_valid_thinking_json_becomes_block() {
	let message = Message {
		thinking: Some(serde_json::json!({"content": "let me think", "tokens": 7})),
		..msg("assistant", "answer")
	};
	let converted = convert_message_to_octolib(&message).expect("must convert");
	let thinking = converted.thinking.expect("thinking must be set");
	assert_eq!(thinking.content, "let me think");
	assert_eq!(thinking.tokens, 7);
}

#[test]
fn test_convert_invalid_thinking_json_is_dropped_not_fatal() {
	let message = Message {
		thinking: Some(serde_json::json!("not a thinking block")),
		..msg("assistant", "answer")
	};
	let converted = convert_message_to_octolib(&message)
		.expect("invalid thinking must not fail the conversion");
	assert!(
		converted.thinking.is_none(),
		"invalid thinking must be dropped"
	);
}

#[test]
fn test_convert_images_and_videos() {
	let message = Message {
		images: Some(vec![crate::session::image::ImageAttachment {
			data: crate::session::image::ImageData::Base64("aGVsbG8=".to_string()),
			media_type: "image/png".to_string(),
			source_type: crate::session::image::SourceType::File(std::path::PathBuf::from(
				"/tmp/x.png",
			)),
			dimensions: Some((800, 600)),
			size_bytes: Some(1024),
		}]),
		videos: Some(vec![crate::session::video::VideoAttachment {
			data: crate::session::video::VideoData::Url("https://example.test/v.mp4".to_string()),
			media_type: "video/mp4".to_string(),
			source_type: crate::session::video::SourceType::Url,
			dimensions: None,
			size_bytes: None,
			duration_secs: Some(1.5),
		}]),
		..msg("user", "look")
	};
	let converted = convert_message_to_octolib(&message).expect("must convert");

	let images = converted.images.expect("images must convert");
	assert_eq!(images.len(), 1);
	assert_eq!(images[0].media_type, "image/png");
	assert_eq!(images[0].dimensions, Some((800, 600)));
	assert_eq!(images[0].size_bytes, Some(1024));
	assert!(matches!(
		&images[0].data,
		octolib::llm::ImageData::Base64(b) if b == "aGVsbG8="
	));
	assert!(matches!(
		&images[0].source_type,
		octolib::llm::SourceType::File(p) if p == &std::path::PathBuf::from("/tmp/x.png")
	));

	let videos = converted.videos.expect("videos must convert");
	assert_eq!(videos.len(), 1);
	assert_eq!(videos[0].media_type, "video/mp4");
	assert_eq!(videos[0].duration_secs, Some(1.5));
	assert!(matches!(
		&videos[0].data,
		octolib::llm::VideoData::Url(u) if u == "https://example.test/v.mp4"
	));
	assert!(matches!(
		videos[0].source_type,
		octolib::llm::SourceType::Url
	));
}

// ── convert_response_from_octolib ──────────────────────────────

fn octolib_response(
	tool_calls: Option<Vec<octolib::llm::ToolCall>>,
) -> octolib::llm::ProviderResponse {
	octolib::llm::ProviderResponse {
		content: "done".to_string(),
		thinking: Some(ThinkingBlock::with_tokens("reasoning", 12)),
		exchange: ProviderExchange::new(
			serde_json::json!({"q": 1}),
			serde_json::json!({"a": 2}),
			None,
			"test",
		),
		tool_calls,
		finish_reason: Some("stop".to_string()),
		structured_output: Some(serde_json::json!({"ok": true})),
		id: Some("resp_1".to_string()),
	}
}

#[test]
fn test_convert_response_maps_tool_calls_to_mcp_format() {
	let response = octolib_response(Some(vec![octolib::llm::ToolCall {
		id: "call_1".to_string(),
		name: "read".to_string(),
		arguments: serde_json::json!({"path": "/x"}),
	}]));
	let converted = convert_response_from_octolib(response);
	let calls = converted.tool_calls.expect("tool calls must map");
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0].tool_name, "read");
	assert_eq!(calls[0].tool_id, "call_1");
	assert_eq!(calls[0].parameters, serde_json::json!({"path": "/x"}));
}

#[test]
fn test_convert_response_passes_fields_through() {
	let converted = convert_response_from_octolib(octolib_response(None));
	assert_eq!(converted.content, "done");
	assert!(converted.tool_calls.is_none());
	assert_eq!(converted.finish_reason.as_deref(), Some("stop"));
	assert_eq!(converted.response_id.as_deref(), Some("resp_1"));
	assert_eq!(
		converted.structured_output,
		Some(serde_json::json!({"ok": true}))
	);
	assert_eq!(converted.thinking.as_ref().expect("thinking").tokens, 12);
	assert_eq!(converted.exchange.provider, "test");
}

// ── ChatCompletionParams::to_octolib_params ────────────────────

fn test_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../config-templates/default.toml"))
		.expect("parse default config template");
	// Keep the conversion offline: no MCP servers → no tool fetching.
	config.mcp.servers.clear();
	config
}

#[tokio::test]
async fn test_octolib_params_defaults_and_passthrough() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];
	let params = ChatCompletionParams::new(&messages, "test-model", 0.5, 0.9, 7, 1234, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");

	assert_eq!(octo.model, "test-model");
	assert_eq!(octo.temperature, 0.5);
	assert_eq!(octo.top_p, 0.9);
	assert_eq!(octo.top_k, 7);
	assert_eq!(octo.max_tokens, 1234);
	// Defaults come from the config template: max_retries=1, retry_timeout=30s.
	assert_eq!(octo.max_retries, 1);
	assert_eq!(octo.retry_timeout, std::time::Duration::from_secs(30));
	assert_eq!(
		octo.request_timeout,
		Some(std::time::Duration::from_secs(300))
	);
	assert!(octo.use_long_cache, "long cache is always enabled");
	assert!(octo.tools.is_none(), "no MCP servers → no tools attached");
	assert_eq!(octo.messages.len(), 1);
	assert_eq!(octo.messages[0].timestamp, 1_700_000_000);
}

#[tokio::test]
async fn test_octolib_params_builder_overrides() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];
	let params =
		ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config).with_max_retries(7);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(octo.max_retries, 7);
}

#[tokio::test]
async fn test_octolib_params_cached_system_message_gets_one_hour_ttl() {
	let config = test_config();
	let messages = vec![
		Message {
			cached: true,
			..msg("system", "sysprompt")
		},
		msg("user", "hi"),
	];
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(octo.messages[0].role, "system");
	assert!(octo.messages[0].cached);
	assert_eq!(octo.messages[0].cache_ttl.as_deref(), Some("1h"));
}

#[tokio::test]
async fn test_octolib_params_appends_synthetic_user_after_assistant() {
	let config = test_config();
	let messages = vec![msg("user", "hi"), msg("assistant", "hello")];
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(
		octo.messages.len(),
		3,
		"synthetic continuation must be appended"
	);
	let last = octo.messages.last().expect("non-empty");
	assert_eq!(last.role, "user");
	assert_eq!(last.content, "Please continue.");
}

#[tokio::test]
async fn test_octolib_params_no_synthetic_user_after_user_message() {
	let config = test_config();
	let messages = vec![
		Message {
			cached: true,
			..msg("system", "sysprompt")
		},
		msg("user", "hi"),
	];
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(octo.messages.len(), 2, "no synthetic message expected");
	assert_eq!(octo.messages.last().expect("non-empty").role, "user");
}

#[tokio::test]
async fn test_octolib_params_system_only_messages_get_no_synthetic_user() {
	let config = test_config();
	let messages = vec![msg("system", "sysprompt")];
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(
		octo.messages.len(),
		1,
		"no non-system message → no synthetic append"
	);
}

#[tokio::test]
async fn test_octolib_params_purpose_header_sent_on_every_request() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];

	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(
		octo.extra_headers
			.as_ref()
			.and_then(|h| h.get(MODEL_PURPOSE_HEADER)),
		Some(&"main".to_string()),
		"untagged calls are MAIN traffic"
	);

	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
		.with_purpose(ModelPurpose::Compression);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(
		octo.extra_headers
			.as_ref()
			.and_then(|h| h.get(MODEL_PURPOSE_HEADER)),
		Some(&"compression".to_string())
	);
}

#[tokio::test]
async fn test_octolib_params_reasoning_effort_override_beats_config() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];

	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	// Config template default is "medium".
	assert_eq!(
		octo.reasoning_effort,
		Some(octolib::llm::ReasoningEffort::Medium)
	);

	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
		.with_reasoning_effort(crate::config::ReasoningEffortConfig::High);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(
		octo.reasoning_effort,
		Some(octolib::llm::ReasoningEffort::High)
	);
}

#[tokio::test]
async fn test_octolib_params_schema_becomes_strict_structured_output() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];
	let schema = serde_json::json!({
		"type": "object",
		"properties": {"answer": {"type": "string"}},
		"required": ["answer"]
	});
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
		.with_schema(schema.clone());
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	let rf = octo
		.response_format
		.expect("schema must set response_format");
	assert!(matches!(rf.format, octolib::llm::OutputFormat::JsonSchema));
	assert!(matches!(rf.mode, octolib::llm::ResponseMode::Strict));
	assert_eq!(rf.schema, Some(schema));
}

#[tokio::test]
async fn test_octolib_params_zero_request_timeout_disables_deadline() {
	let mut config = test_config();
	config.request_timeout_seconds = 0;
	let messages = vec![msg("user", "hi")];
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert_eq!(
		octo.request_timeout, None,
		"0 must mean no per-request timeout"
	);
}

#[tokio::test]
async fn test_octolib_params_cancellation_token_attached() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];
	let (_tx, rx) = watch::channel(false);
	let params = ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config)
		.with_cancellation_token(rx);
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert!(octo.cancellation_token.is_some(), "token must be forwarded");
}

#[tokio::test]
async fn test_octolib_params_without_tools_keeps_tools_empty() {
	let config = test_config();
	let messages = vec![msg("user", "hi")];
	let params =
		ChatCompletionParams::new(&messages, "m", 0.1, 1.0, 0, 10, &config).without_tools();
	let octo = params
		.to_octolib_params()
		.await
		.expect("conversion succeeds");
	assert!(
		octo.tools.is_none(),
		"text-only calls must not attach tools"
	);
}
