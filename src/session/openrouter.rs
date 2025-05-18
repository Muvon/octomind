// OpenRouter API client for OctoDev

use anyhow::Result;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

// Store raw request/response for logging
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OpenRouterExchange {
    pub request: serde_json::Value,
    pub response: serde_json::Value,
    pub timestamp: u64,
}

// Default OpenRouter API key environment variable name
const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

// Get OpenRouter API key from environment
pub fn get_api_key() -> Result<String, anyhow::Error> {
    match env::var(OPENROUTER_API_KEY_ENV) {
        Ok(key) => Ok(key),
        Err(_) => Err(anyhow::anyhow!("OPENROUTER_API_KEY environment variable not set"))
    }
}

/// Message format for the OpenRouter API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

// Convert our session messages to OpenRouter format
pub fn convert_messages(messages: &[super::Message]) -> Vec<Message> {
    messages.iter().map(|msg| {
        Message {
            role: msg.role.clone(),
            content: msg.content.clone(),
        }
    }).collect()
}

// Send a chat completion request to OpenRouter
pub async fn chat_completion(
	messages: Vec<Message>,
	model: &str,
	temperature: f32,
) -> Result<(String, OpenRouterExchange)> {
	// Get API key
	let api_key = get_api_key()?;

	// Create the request body
	let request_body = serde_json::json!({
		"model": model,
		"messages": messages,
		"temperature": temperature,
		"max_tokens": 2048,
	});

	// Create HTTP client
	let client = Client::new();

	// Make the actual API request
	let response = client.post(OPENROUTER_API_URL)
		.header("Authorization", format!("Bearer {}", api_key))
		.header("Content-Type", "application/json")
		.header("HTTP-Referer", "https://github.com/muvon/octodev")
		.header("X-Title", "OctoDev")
		.json(&request_body)
		.send()
	.await?;

	// Get response status
	let status = response.status();

	// Get response body as text first for debugging
	let response_text = response.text().await?;

	// Parse the text to JSON
	let response_json: serde_json::Value = match serde_json::from_str(&response_text) {
		Ok(json) => json,
		Err(e) => {
			return Err(anyhow::anyhow!("Failed to parse response JSON: {}. Response: {}", e, response_text));
		}
	};

	// Handle error responses
	if !status.is_success() {
		let error_msg = if let Some(msg) = response_json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
			msg
		} else if let Some(msg) = response_json.get("error").and_then(|e| e.as_str()) {
			msg
		} else {
			"Unknown API error"
		};

		return Err(anyhow::anyhow!("OpenRouter API error: {}", error_msg));
	}

	// Extract content from response
	let content = response_json
		.get("choices")
		.and_then(|choices| choices.get(0))
		.and_then(|choice| choice.get("message"))
		.and_then(|message| message.get("content"))
		.and_then(|content| content.as_str())
		.ok_or_else(|| anyhow::anyhow!("Invalid response format from OpenRouter: {}", response_text))?
		.to_string();

	// Create exchange record for logging
	let exchange = OpenRouterExchange {
		request: request_body,
		response: response_json.clone(),
		timestamp: SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
	};

	Ok((content, exchange))
}
