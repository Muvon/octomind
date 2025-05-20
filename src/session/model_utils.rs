// Utilities for model-specific features

// Function to check if a model supports caching
pub fn model_supports_caching(model: &str) -> bool {
    // Models known to support caching
    let supported_models = [
        "anthropic/",       // All Anthropic (Claude) models
        "google/",          // Google models
        "anthropic.claude",  // Alternative format for Anthropic models
        "gemini",           // Google Gemini models
    ];
    
    // Check if the model name contains any of the supported prefixes
    supported_models.iter().any(|prefix| model.to_lowercase().contains(prefix))
}