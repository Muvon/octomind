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
fn caching_fallback_matches_known_prefixes() {
	// No "provider:" prefix → parse fails → legacy prefix fallback runs.
	assert!(model_supports_caching("anthropic/claude-3-opus"));
	assert!(model_supports_caching("google/gemini-2.0-flash"));
	assert!(model_supports_caching("anthropic.claude-3-5-sonnet"));
	assert!(model_supports_caching("gemini-pro"));
}

#[test]
fn caching_fallback_is_case_insensitive() {
	assert!(model_supports_caching("Anthropic/Claude-3-Opus"));
	assert!(model_supports_caching("GEMINI-pro"));
}

#[test]
fn caching_fallback_rejects_unsupported_models() {
	assert!(!model_supports_caching("openai/gpt-4"));
	assert!(!model_supports_caching("meta-llama/llama-3"));
}

#[test]
fn caching_invalid_model_strings_do_not_panic() {
	assert!(!model_supports_caching(""));
	assert!(!model_supports_caching("   "));
	assert!(!model_supports_caching("::"));
	assert!(!model_supports_caching("no-such-provider:some-model"));
}

#[test]
fn caching_resolves_through_provider_when_model_is_known() {
	// Resolves to the Anthropic provider, which supports caching on every model.
	assert!(model_supports_caching("anthropic:claude-sonnet-4-5"));
}

#[test]
fn caching_unknown_provider_model_falls_back_to_prefix_check() {
	// Valid provider, but a model outside its pricing table → provider lookup
	// fails and the legacy prefix check runs on the raw string, which contains
	// "anthropic" but none of the four supported prefixes.
	assert!(!model_supports_caching("anthropic:not-a-real-model"));
}

#[test]
fn vision_invalid_model_returns_err_gracefully() {
	assert!(model_supports_vision("no-colon-model").is_err());
	assert!(model_supports_vision("").is_err());
	assert!(model_supports_vision("bad-provider:model").is_err());
}

#[test]
fn vision_known_model_returns_ok_without_panicking() {
	assert!(model_supports_vision("anthropic:claude-sonnet-4-5").is_ok());
}

#[test]
fn video_invalid_model_returns_err_gracefully() {
	assert!(model_supports_video("no-colon-model").is_err());
	assert!(model_supports_video("").is_err());
}

#[test]
fn max_input_tokens_unresolvable_model_is_none() {
	assert!(model_max_input_tokens("garbage").is_none());
	assert!(model_max_input_tokens("").is_none());
	assert!(model_max_input_tokens("no-such-provider:m").is_none());
}

#[test]
fn max_input_tokens_known_model_is_some() {
	assert!(model_max_input_tokens("anthropic:claude-sonnet-4-5").is_some());
}
