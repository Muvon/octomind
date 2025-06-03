// Copyright 2025 Muvon Un Limited
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

use crate::session::ProviderFactory;

// Function to check if a model supports caching
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
