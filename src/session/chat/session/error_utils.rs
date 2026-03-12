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

// Error utilities for session handling

use crate::session::chat::session::core::ChatSession;
use crate::session::output::OutputMode;
use crate::{log_debug, log_info};
use colored::*;

// Helper function to display rate limit information from provider response
pub fn display_rate_limit_info(exchange: &crate::session::ProviderExchange) {
	if let Some(ref rate_limit_headers) = exchange.rate_limit_headers {
		let mut rate_limit_info = Vec::new();

		match exchange.provider.as_str() {
			"anthropic" => {
				// Anthropic rate limit format
				if let (Some(tokens_remaining), Some(tokens_limit)) = (
					rate_limit_headers.get("tokens_remaining"),
					rate_limit_headers.get("tokens_limit"),
				) {
					rate_limit_info.push(format!("Tokens: {}/{}", tokens_remaining, tokens_limit));
				}

				if let (Some(input_remaining), Some(input_limit)) = (
					rate_limit_headers.get("input_tokens_remaining"),
					rate_limit_headers.get("input_tokens_limit"),
				) {
					rate_limit_info
						.push(format!("Input tokens: {}/{}", input_remaining, input_limit));
				}

				if let (Some(output_remaining), Some(output_limit)) = (
					rate_limit_headers.get("output_tokens_remaining"),
					rate_limit_headers.get("output_tokens_limit"),
				) {
					rate_limit_info.push(format!(
						"Output tokens: {}/{}",
						output_remaining, output_limit
					));
				}

				if !rate_limit_info.is_empty() {
					log_info!("📊 Anthropic rate limits: {}", rate_limit_info.join(" | "));
				}
			}
			"openai" => {
				// OpenAI rate limit format
				if let (Some(requests_remaining), Some(requests_limit)) = (
					rate_limit_headers.get("requests_remaining"),
					rate_limit_headers.get("requests_limit"),
				) {
					rate_limit_info.push(format!(
						"Requests: {}/{}",
						requests_remaining, requests_limit
					));
				}

				if let (Some(tokens_remaining), Some(tokens_limit)) = (
					rate_limit_headers.get("tokens_remaining"),
					rate_limit_headers.get("tokens_limit"),
				) {
					rate_limit_info.push(format!("Tokens: {}/{}", tokens_remaining, tokens_limit));
				}

				if let Some(request_reset) = rate_limit_headers.get("request_reset") {
					rate_limit_info.push(format!("Request reset: {}", request_reset));
				}

				if !rate_limit_info.is_empty() {
					log_info!("📊 OpenAI rate limits: {}", rate_limit_info.join(" | "));
				}
			}
			_ => {
				// Generic rate limit display for other providers
				if !rate_limit_headers.is_empty() {
					let info: Vec<String> = rate_limit_headers
						.iter()
						.map(|(k, v)| format!("{}: {}", k, v))
						.collect();
					log_info!("📊 {} rate limits: {}", exchange.provider, info.join(" | "));
				}
			}
		}
	}
}

// Helper function to format provider errors with better context
pub fn format_provider_error(provider_name: &str, error: &anyhow::Error) -> String {
	let error_str = error.to_string();

	// Check if this is a status code error (like "520 <unknown status code>")
	if error_str.contains("API error") && error_str.contains("<unknown status code>") {
		// Extract status code and provide better context
		if let Some(status_start) = error_str.find("error ") {
			if let Some(status_end) = error_str[status_start + 6..].find(' ') {
				let status_code = &error_str[status_start + 6..status_start + 6 + status_end];

				// Provide context for common status codes
				let context = match status_code {
					"520" => "Server overloaded - this usually indicates the provider is experiencing high traffic. Try again in a few moments.",
					"429" => "Rate limit exceeded - you're making requests too quickly. Wait a moment before trying again.",
					"503" => "Service temporarily unavailable - the provider's servers are temporarily down.",
					"502" | "504" => "Gateway error - temporary connectivity issue with the provider.",
					"500" => "Internal server error - temporary issue on the provider's side.",
					_ => "Server error - temporary issue with the provider.",
				};

				return format!("HTTP {} - {}", status_code, context);
			}
		}
	}

	// Check for other common error patterns and provide better context
	if error_str.contains("rate limit") || error_str.contains("Rate limit") {
		return "Rate limit exceeded - you're making requests too quickly. Wait a moment before trying again.".to_string();
	}

	if error_str.contains("timeout") || error_str.contains("Timeout") {
		return "Request timed out - the provider took too long to respond. Try again.".to_string();
	}

	if error_str.contains("API key")
		|| error_str.contains("authentication")
		|| error_str.contains("unauthorized")
	{
		return format!(
			"Authentication failed - check your {} API key configuration.",
			provider_name
		);
	}

	if error_str.contains("overloaded") || error_str.contains("capacity") {
		return "Provider is currently overloaded - try again in a few moments.".to_string();
	}

	// For other errors, return the original message but cleaned up
	error_str
}

// Helper function to handle API errors with provider-specific messages
pub fn handle_api_error(
	chat_session: &mut ChatSession,
	user_message_index: usize,
	model: &str,
	error: &anyhow::Error,
	mode: OutputMode,
) {
	// Remove user message on API failure
	if user_message_index < chat_session.session.messages.len() {
		chat_session.session.messages.truncate(user_message_index);
		log_debug!("Removed user message due to API call failure");
	}

	// Print error with provider context

	// Print error with provider context
	// Extract provider name from the model string
	let provider_name =
		if let Ok((provider, _)) = crate::providers::ProviderFactory::parse_model(model) {
			provider
		} else {
			"unknown provider".to_string()
		};

	// Format error message with better context
	let error_message = format_provider_error(&provider_name, error);
	if mode.should_suppress_cli_output() {
		log_info!("Error calling {}: {}", provider_name, error_message);
		return;
	}

	println!(
		"\n{}: {}",
		format!("Error calling {}", provider_name).bright_red(),
		error_message
	);

	// Provider-specific help message
	match provider_name.to_lowercase().as_str() {
		"openrouter" => {
			println!("{}", "Make sure OpenRouter API key is set in the config or as OPENROUTER_API_KEY environment variable.".yellow());
		}
		"anthropic" => {
			println!("{}", "Make sure Anthropic API key is set in the config or as ANTHROPIC_API_KEY environment variable.".yellow());
		}
		"openai" => {
			println!("{}", "Make sure OpenAI API key is set in the config or as OPENAI_API_KEY environment variable.".yellow());
		}
		"google" => {
			println!("{}", "Make sure Google credentials are set in the config or as GOOGLE_APPLICATION_CREDENTIALS environment variable.".yellow());
		}
		"amazon" => {
			println!(
				"{}",
				"Make sure AWS credentials are configured properly for Amazon Bedrock access."
					.yellow()
			);
		}
		"cloudflare" => {
			println!("{}", "Make sure Cloudflare API key is set in the config or as CLOUDFLARE_API_KEY environment variable.".yellow());
		}
		_ => {
			println!(
				"{}",
				"Make sure the API key for this provider is properly configured.".yellow()
			);
		}
	}
}
