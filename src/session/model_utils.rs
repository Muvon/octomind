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
		"anthropic.claude",  // Alternative format for Anthropic models
		"gemini",           // Google Gemini models
	];

	// Check if the model name contains any of the supported prefixes
	supported_models.iter().any(|prefix| model.to_lowercase().contains(prefix))
}
