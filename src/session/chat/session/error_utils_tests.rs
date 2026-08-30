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

fn err(msg: &str) -> anyhow::Error {
	anyhow::anyhow!(msg.to_string())
}

#[test]
fn unknown_status_codes_get_actionable_context() {
	for (code, marker) in [
		("520", "overloaded"),
		("429", "Rate limit"),
		("503", "temporarily unavailable"),
		("502", "Gateway"),
		("504", "Gateway"),
		("500", "Internal server error"),
	] {
		let raw = format!("API error {code} <unknown status code>");
		let out = format_provider_error("openai", &err(&raw));
		assert!(out.starts_with(&format!("HTTP {code} - ")), "got: {out}");
		assert!(out.contains(marker), "got: {out}");
	}
}

#[test]
fn unmapped_status_code_falls_back_to_a_generic_note() {
	let out = format_provider_error("openai", &err("API error 418 <unknown status code>"));
	assert_eq!(
		out,
		"HTTP 418 - Server error - temporary issue with the provider."
	);
}

#[test]
fn octohub_errors_are_passed_through_verbatim() {
	// The server's own message is more specific than any rewrite here — in
	// particular it must not be swallowed by the "API key" branch below.
	let raw = "OctoHub API error 403: model 'x' is not permitted for this API key";
	assert_eq!(format_provider_error("octohub", &err(raw)), raw);
}

#[test]
fn common_failure_shapes_are_rewritten() {
	assert!(format_provider_error("openai", &err("rate limit reached")).contains("Rate limit"));
	assert!(format_provider_error("openai", &err("Rate limit hit")).contains("Rate limit"));
	assert!(format_provider_error("openai", &err("connection timeout")).contains("timed out"));
	assert!(format_provider_error("openai", &err("Timeout after 60s")).contains("timed out"));
	assert!(format_provider_error("openai", &err("model overloaded")).contains("overloaded"));
	assert!(format_provider_error("openai", &err("at capacity")).contains("overloaded"));
}

#[test]
fn auth_failures_name_the_provider() {
	for raw in ["invalid API key", "authentication failed", "unauthorized"] {
		let out = format_provider_error("anthropic", &err(raw));
		assert_eq!(
			out,
			"Authentication failed - check your anthropic API key configuration."
		);
	}
}

#[test]
fn unrecognised_errors_are_returned_unchanged() {
	let raw = "socket hang up while streaming";
	assert_eq!(format_provider_error("openai", &err(raw)), raw);
}

#[test]
fn multibyte_error_text_does_not_panic() {
	// The status-code branch slices by byte offset — non-ASCII text around
	// the marker must not split a char.
	let raw = "провайдер: API error 520 <unknown status code> — недоступен";
	assert!(format_provider_error("openai", &err(raw)).starts_with("HTTP 520"));
	assert_eq!(
		format_provider_error("openai", &err("сетевая ошибка 日本語")),
		"сетевая ошибка 日本語"
	);
}

#[test]
fn rate_limit_info_renders_every_provider_shape() {
	let exchange_with_headers =
		|provider: &str, headers: Option<std::collections::HashMap<String, String>>| {
			let mut exchange = crate::session::ProviderExchange::new(
				serde_json::json!({}),
				serde_json::json!({}),
				None,
				provider,
			);
			exchange.rate_limit_headers = headers;
			exchange
		};

	let mut anthropic = std::collections::HashMap::new();
	anthropic.insert("tokens_remaining".to_string(), "1000".to_string());
	anthropic.insert("tokens_limit".to_string(), "2000".to_string());
	anthropic.insert("input_tokens_remaining".to_string(), "900".to_string());
	anthropic.insert("input_tokens_limit".to_string(), "1000".to_string());
	anthropic.insert("output_tokens_remaining".to_string(), "500".to_string());
	anthropic.insert("output_tokens_limit".to_string(), "600".to_string());
	display_rate_limit_info(&exchange_with_headers("anthropic", Some(anthropic)));

	// Partial anthropic headers: only the tokens pair is present
	let mut partial = std::collections::HashMap::new();
	partial.insert("tokens_remaining".to_string(), "1".to_string());
	partial.insert("tokens_limit".to_string(), "2".to_string());
	display_rate_limit_info(&exchange_with_headers("anthropic", Some(partial)));

	let mut openai = std::collections::HashMap::new();
	openai.insert("requests_remaining".to_string(), "58".to_string());
	openai.insert("requests_limit".to_string(), "60".to_string());
	openai.insert("tokens_remaining".to_string(), "1000".to_string());
	openai.insert("tokens_limit".to_string(), "2000".to_string());
	openai.insert("request_reset".to_string(), "1h".to_string());
	display_rate_limit_info(&exchange_with_headers("openai", Some(openai)));

	let mut generic = std::collections::HashMap::new();
	generic.insert("x-rpm".to_string(), "30".to_string());
	display_rate_limit_info(&exchange_with_headers("groq", Some(generic)));

	// No headers at all: early return
	display_rate_limit_info(&exchange_with_headers("test", None));
}

#[test]
fn api_error_removes_failed_user_message_and_prints_provider_help() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.add_user_message("doomed request").unwrap();
	assert_eq!(session.session.messages.len(), 1);

	handle_api_error(
		&mut session,
		0,
		"anthropic/claude-sonnet-4",
		&err("boom"),
		OutputMode::NonInteractive,
	);
	assert!(session.session.messages.is_empty());

	// OctoHub plan-restriction hint, unknown-provider fallback, and the
	// suppressed-output early return.
	let mut session = ChatSession::for_tests(Vec::new());
	handle_api_error(
		&mut session,
		0,
		"octohub:big-model",
		&err("OctoHub API error 403: model 'big-model' is not permitted for this API key"),
		OutputMode::NonInteractive,
	);
	handle_api_error(
		&mut session,
		0,
		"weird:model",
		&err("socket hang up"),
		OutputMode::NonInteractive,
	);
	handle_api_error(
		&mut session,
		0,
		"openai:gpt-x",
		&err("timeout"),
		OutputMode::Jsonl,
	);
}

#[test]
fn followup_error_prints_without_touching_history() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.add_user_message("q").unwrap();
	handle_followup_api_error(
		"openai:gpt-x",
		&err("connection timeout"),
		OutputMode::NonInteractive,
	);
	handle_followup_api_error(
		"openai:gpt-x",
		&err("connection timeout"),
		OutputMode::Jsonl,
	);
	assert_eq!(session.session.messages.len(), 1);
}
