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
use crate::session::image::{ImageAttachment, ImageData, SourceType};
use crate::session::Message;

fn msg(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
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
	}
}

#[test]
fn empty_text_costs_nothing() {
	assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn token_count_grows_with_text() {
	let short = estimate_tokens("hello world");
	let long = estimate_tokens(&"hello world ".repeat(50));
	assert!(short > 0);
	assert!(long > short * 10, "short={short} long={long}");
}

#[test]
fn truncate_returns_input_untouched_when_under_cap() {
	let text = "a short sentence";
	assert_eq!(truncate_to_tokens(text, 1_000), text);
}

#[test]
fn truncate_never_exceeds_the_cap() {
	let text = "The quick brown fox jumps over the lazy dog. ".repeat(50);
	for cap in [1, 5, 32, 100] {
		let out = truncate_to_tokens(&text, cap);
		assert!(
			estimate_tokens(&out) <= cap,
			"cap={cap} produced {} tokens",
			estimate_tokens(&out)
		);
		assert!(text.starts_with(&out), "cap={cap} produced a non-prefix");
	}
}

#[test]
fn truncate_to_zero_is_empty() {
	assert_eq!(truncate_to_tokens("anything at all", 0), "");
}

#[test]
fn truncate_is_safe_on_multibyte_text() {
	// Emoji and CJK sit across several BPE tokens; decoding a cut token
	// sequence must not panic or produce invalid UTF-8.
	let text = "日本語のテキスト 🎉🎉🎉 más texto ".repeat(20);
	for cap in [1, 2, 3, 7, 40] {
		let out = truncate_to_tokens(&text, cap);
		assert!(text.starts_with(&out), "cap={cap} produced a non-prefix");
		assert!(
			estimate_tokens(&out) <= cap,
			"cap={cap} produced {} tokens",
			estimate_tokens(&out)
		);
	}
}

#[test]
fn message_tokens_include_the_per_message_overhead() {
	let empty = estimate_message_tokens(&msg("user", ""));
	// 3 formatting tokens + the role itself.
	assert_eq!(empty, 3 + estimate_tokens("user"));

	let with_content = estimate_message_tokens(&msg("user", "hello there"));
	assert_eq!(with_content, empty + estimate_tokens("hello there"));
}

#[test]
fn message_tokens_count_name_tool_calls_and_images() {
	let base = estimate_message_tokens(&msg("assistant", "text"));

	let mut named = msg("assistant", "text");
	named.name = Some("read_file".to_string());
	// name + 1 overhead token, per the OpenAI formula.
	assert_eq!(
		estimate_message_tokens(&named),
		base + estimate_tokens("read_file") + 1
	);

	let mut with_tools = msg("assistant", "text");
	with_tools.tool_calls = Some(serde_json::json!([{"name": "shell", "args": {"cmd": "ls"}}]));
	assert!(estimate_message_tokens(&with_tools) > base);

	let mut with_images = msg("user", "text");
	with_images.images = Some(vec![
		ImageAttachment {
			data: ImageData::Base64("x".to_string()),
			media_type: "image/png".to_string(),
			source_type: SourceType::Clipboard,
			dimensions: None,
			size_bytes: None,
		};
		2
	]);
	// Flat 85 tokens per image.
	assert_eq!(
		estimate_message_tokens(&with_images),
		estimate_message_tokens(&msg("user", "text")) + 170
	);
}

#[test]
fn session_tokens_add_priming_overhead_only_when_non_empty() {
	assert_eq!(estimate_session_tokens(&[]), 0);

	let messages = vec![msg("user", "hi"), msg("assistant", "hello")];
	let sum: usize = messages.iter().map(estimate_message_tokens).sum();
	assert_eq!(estimate_session_tokens(&messages), sum + 3);
}

#[test]
fn full_context_without_tools_equals_session_tokens() {
	let messages = vec![msg("user", "hi")];
	assert_eq!(
		estimate_full_context_tokens(&messages, None),
		estimate_session_tokens(&messages)
	);
}

#[test]
fn fallback_estimator_never_returns_zero_for_non_empty_text() {
	// Guards the degraded path: a zero estimate would make budget maths
	// treat arbitrarily large text as free.
	assert_eq!(fallback_token_count("abc"), 1);
	assert_eq!(fallback_token_count("12345678"), 2);
}

#[test]
fn estimate_tokens_counts_unicode_and_whitespace_text() {
	// CJK chars are individual BPE tokens; the count must be non-trivial.
	assert!(estimate_tokens("日本語のテキスト") >= 3);
	assert!(estimate_tokens("🎉🎉🎉") > 0);
	assert!(estimate_tokens("héllo wörld") > 0);
	// Whitespace-only text still encodes to tokens.
	assert!(estimate_tokens("   \n\t  ") > 0);
}

#[test]
fn truncate_of_empty_input_stays_empty() {
	assert_eq!(truncate_to_tokens("", 0), "");
	assert_eq!(truncate_to_tokens("", 100), "");
}

#[test]
fn message_tokens_count_thinking() {
	let base = estimate_message_tokens(&msg("assistant", "text"));
	let mut with_thinking = msg("assistant", "text");
	with_thinking.thinking =
		Some(serde_json::json!({"reasoning": "deep thoughts about the problem"}));
	assert!(estimate_message_tokens(&with_thinking) > base);
}

fn tool(name: &str) -> crate::mcp::McpFunction {
	crate::mcp::McpFunction {
		name: name.to_string(),
		description: format!("Description for {name}"),
		parameters: serde_json::json!({"type": "object", "properties": {}}),
	}
}

#[test]
fn full_context_with_empty_tool_list_adds_only_array_overhead() {
	let messages = vec![msg("user", "hi")];
	assert_eq!(
		estimate_full_context_tokens(&messages, Some(&[])),
		estimate_session_tokens(&messages) + 10
	);
}

#[test]
fn full_context_tool_overhead_is_linear_per_tool() {
	let messages = vec![msg("user", "hi")];
	let one = [tool("shell")];
	let two = [tool("shell"), tool("shell")];
	let with_one = estimate_full_context_tokens(&messages, Some(&one));
	let with_two = estimate_full_context_tokens(&messages, Some(&two));
	assert!(with_one > estimate_session_tokens(&messages));
	// A second identical tool adds exactly its JSON tokens + 5 formatting
	// tokens; the +10 tools-array overhead is constant.
	let tool_json = serde_json::to_string(&serde_json::json!({
		"name": "shell",
		"description": "Description for shell",
		"input_schema": one[0].parameters,
	}))
	.expect("serialize tool json");
	assert_eq!(with_two - with_one, estimate_tokens(&tool_json) + 5);
}

fn template_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
async fn threshold_validation_is_disabled_when_threshold_is_zero() {
	let mut config = template_config();
	config.max_session_tokens_threshold = 0;
	validate_session_token_threshold(&config, "assistant", std::path::Path::new("."))
		.await
		.expect("zero threshold must skip validation entirely");
}

#[tokio::test]
async fn threshold_validation_rejects_a_tiny_threshold() {
	let mut config = template_config();
	config.max_session_tokens_threshold = 1;
	let dir = tempfile::tempdir().expect("tempdir");
	let err = validate_session_token_threshold(&config, "assistant", dir.path())
		.await
		.expect_err("threshold of 1 must be rejected");
	let message = format!("{err:#}");
	assert!(
		message.contains("max_session_tokens_threshold (1)"),
		"{message}"
	);
	assert!(message.contains("role 'assistant'"), "{message}");
}

#[tokio::test]
async fn default_template_threshold_passes_its_own_validation() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	validate_session_token_threshold(&config, "assistant", dir.path())
		.await
		.expect("shipped default threshold must satisfy the 2x safety check");
}

#[tokio::test]
async fn minimum_tokens_cover_system_prompt_and_request_overhead() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	let minimum = calculate_minimum_session_tokens(&config, "assistant", dir.path())
		.await
		.expect("minimum calculation");
	let (_, _, _, _, system_prompt) = config.get_role_config("assistant");
	let system_tokens = estimate_tokens(system_prompt);
	// request_overhead (50) + at least the welcome message's 20-token
	// structure cost — the welcome message is always present.
	assert!(minimum >= system_tokens + 70);
}
