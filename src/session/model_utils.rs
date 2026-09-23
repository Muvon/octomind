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

// Utilities for model-specific features

use crate::providers::ProviderFactory;

/// Whether a model may receive image attachments.
///
/// Unknown models stay permissive because proxy providers can expose models
/// that are newer than octolib's reference table.
pub fn model_supports_vision(model: &str) -> anyhow::Result<bool> {
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(model)?;
	if octolib::llm::reference_capabilities::get_reference_capabilities(&actual_model).is_none() {
		return Ok(true);
	}
	Ok(provider.supports_vision(&actual_model))
}

/// Whether a model may receive video attachments.
///
/// Unknown models stay permissive because proxy providers can expose models
/// that are newer than octolib's reference table.
pub fn model_supports_video(model: &str) -> anyhow::Result<bool> {
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(model)?;
	if octolib::llm::reference_capabilities::get_reference_capabilities(&actual_model).is_none() {
		return Ok(true);
	}
	Ok(provider.supports_video(&actual_model))
}

/// Provider-reported context window (max input tokens) for a model.
/// None when the model string doesn't resolve to a configured provider.
pub fn model_max_input_tokens(model: &str) -> Option<usize> {
	ProviderFactory::get_provider_for_model(model)
		.ok()
		.map(|(provider, actual_model)| provider.get_max_input_tokens(&actual_model))
}

// Function to check if a model supports caching
/// Whether the provider sends stored assistant `thinking` back with later
/// requests, so it occupies context the model actually reads. Mirrors the
/// request builders in octolib 0.39: Z.AI replays every historical
/// `reasoning_content` (Preserved Thinking), DeepSeek replays it on tool-using
/// requests, Moonshot and Ollama only for Kimi K2.6+/K3; every other provider
/// (the OpenAI-compatible family incl. Alibaba, Anthropic, OpenRouter, MiniMax,
/// OctoHub, OpenAI) drops it on the floor. Counting dropped thinking towards the
/// context estimate made a 60k-token prompt read as 100k (measured on
/// deepseek-v4-flash via Alibaba), so folds fired at half the fire line and the
/// "tokens saved" they reported were mostly text the provider never saw.
pub fn model_replays_thinking(model: &str) -> bool {
	let (provider, name) = model.split_once(':').unwrap_or(("", model));
	let provider = provider.to_ascii_lowercase();
	let name = name.to_ascii_lowercase();
	let kimi_preserved = ["kimi-k2.6", "kimi-k2.7", "kimi-k3"]
		.iter()
		.any(|needle| name.contains(needle));
	match provider.as_str() {
		"zai" | "deepseek" => true,
		"moonshot" | "ollama" => kimi_preserved,
		_ => false,
	}
}

pub fn model_supports_caching(model: &str) -> bool {
	// Try to use the new provider system first
	if let Ok((provider, actual_model)) = ProviderFactory::get_provider_for_model(model) {
		return provider.supports_caching(&actual_model);
	}

	// Fallback to legacy logic for backward compatibility
	let supported_models = [
		"anthropic/",       // All Anthropic (Claude) models
		"google/",          // Google models
		"anthropic.claude", // Alternative format for Anthropic models
		"gemini",           // Google Gemini models
	];

	// Check if the model name contains any of the supported prefixes
	supported_models
		.iter()
		.any(|prefix| model.to_lowercase().contains(prefix))
}

#[cfg(test)]
#[path = "model_utils_tests.rs"]
mod tests;
