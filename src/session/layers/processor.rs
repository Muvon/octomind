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

use crate::config::Config;
use crate::session::{Message, Session};
use super::layer_trait::{Layer, LayerConfig, LayerResult};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use async_trait::async_trait;
use colored::Colorize;

// Base processor that handles common functionality for all layers
pub struct LayerProcessor {
	pub config: LayerConfig,
}

impl LayerProcessor {
	pub fn new(config: LayerConfig) -> Self {
		Self { config }
	}

	// Create messages for the OpenRouter API based on the layer
	pub fn create_messages(
		&self,
		input: &str,
		session: &Session,
	) -> Vec<Message> {
		let mut messages = Vec::new();

		// Get the effective model and system prompt for this layer
		let effective_model = self.config.get_effective_model(&session.info.model);
		let system_prompt = self.config.get_effective_system_prompt();

		// Only mark system messages as cached if the model supports it
		let should_cache = crate::session::model_utils::model_supports_caching(&effective_model);

		messages.push(Message {
			role: "system".to_string(),
			content: system_prompt,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: should_cache, // Only cache if model supports it
			tool_call_id: None, // No tool_call_id for system messages
			name: None, // No name for system messages
			tool_calls: None, // No tool_calls for system messages
		});

		// Prepare input based on input_mode using the trait's prepare_input method
		let processed_input = self.prepare_input(input, session);

		// Add user message with the processed input
		messages.push(Message {
			role: "user".to_string(),
			content: processed_input,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: None, // No tool_call_id for user messages
			name: None, // No name for user messages
			tool_calls: None, // No tool_calls for user messages
		});

		messages
	}
}

// Async implementation of the Layer trait for LayerProcessor
#[async_trait]
impl Layer for LayerProcessor {
	fn name(&self) -> &str {
		&self.config.name
	}

	fn config(&self) -> &LayerConfig {
		&self.config
	}

	async fn process(
		&self,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: Arc<AtomicBool>
	) -> Result<LayerResult> {
		// Check if operation was cancelled
		if operation_cancelled.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("Operation cancelled"));
		}

		// Get the effective model for this layer
		let effective_model = self.config.get_effective_model(&session.info.model);

		// Create messages for this layer
		let messages = self.create_messages(input, session);

		// Call the model directly with session messages
		let response = crate::session::chat_completion_with_provider(
			&messages,
			&effective_model,
			self.config.temperature,
			config
		).await?;

		let (output, exchange, direct_tool_calls, _finish_reason) = (
			response.content,
			response.exchange,
			response.tool_calls,
			response.finish_reason
		);

		// Check if the layer response contains tool calls
		if !self.config.mcp.server_refs.is_empty() {
			// First try to use directly returned tool calls, then fall back to parsing if needed
			let tool_calls = if let Some(ref calls) = direct_tool_calls {
				calls
			} else {
				&crate::mcp::parse_tool_calls(&output)
			};

			// If there are tool calls, process them
			if !tool_calls.is_empty() {
				// Process tool calls within our isolated layer session
				let output_clone = output.clone();

				// Execute all tool calls and collect results
				let mut tool_results = Vec::new();

				for tool_call in tool_calls {
					println!("{} {}", "Tool call:".yellow(), tool_call.tool_name);

					// Check if tool is allowed for this layer
					if !self.config.mcp.allowed_tools.is_empty() &&
					!self.config.mcp.allowed_tools.contains(&tool_call.tool_name) {
						println!("{} {} {}", "Tool".red(), tool_call.tool_name, "not allowed for this layer".red());
						continue;
					}

					let result = match crate::mcp::execute_layer_tool_call(tool_call, config, &self.config).await {
						Ok((res, _tool_time_ms)) => res,  // Extract result from tuple
						Err(e) => {
							println!("{} {}", "Tool execution error:".red(), e);
							continue;
						}
					};

					// Add result to collection
					tool_results.push(result);
				}

				// If we have results, send them back to the model to get a final response
				if !tool_results.is_empty() {
					// Format the results in a way the model can understand
					println!("{}", "Processing tool results...".cyan());

					// Create a new session context for tool result processing
					let mut layer_session = messages.clone();

					// Add assistant's response with tool calls
					layer_session.push(crate::session::Message {
						role: "assistant".to_string(),
						content: output_clone,
						timestamp: std::time::SystemTime::now()
							.duration_since(std::time::UNIX_EPOCH)
							.unwrap_or_default()
							.as_secs(),
						cached: false,
						tool_call_id: None, // No tool_call_id for assistant messages
						name: None, // No name for assistant messages
						tool_calls: None, // No tool_calls for assistant messages
					});

					// Add each tool result as a tool message in standard OpenRouter format
					for tool_result in &tool_results {
						// Use standard OpenRouter format for tool messages
						layer_session.push(crate::session::Message {
							role: "tool".to_string(),
							content: serde_json::to_string(&tool_result.result).unwrap_or_default(),
							timestamp: std::time::SystemTime::now()
								.duration_since(std::time::UNIX_EPOCH)
								.unwrap_or_default()
								.as_secs(),
							cached: false,
							tool_call_id: Some(tool_result.tool_id.clone()), // Include the tool_call_id
							name: Some(tool_result.tool_name.clone()), // Include the tool name
							tool_calls: None, // No tool_calls for tool messages
						});
					}

					// Call the model again with tool results
					// Important: We use THIS LAYER'S model to process the function call results
					match crate::session::chat_completion_with_provider(
						&layer_session,
						&effective_model,
						self.config.temperature,
						config
					).await {
						Ok(response) => {
							// Extract token usage if available
							let token_usage = response.exchange.usage.clone();

							// Return the result with the updated output
							return Ok(LayerResult {
								output: response.content,
								exchange: response.exchange,
								token_usage,
								tool_calls: response.tool_calls,
								api_time_ms: 0, // TODO: Add time tracking to processor
								tool_time_ms: 0,
								total_time_ms: 0,
							});
						},
						Err(e) => {
							println!("{} {}", "Error processing tool results:".red(), e);
							// Continue with the original output
						}
					}
				}
			}
		}

		// Extract token usage if available
		let token_usage = exchange.usage.clone();

		// Return the result
		Ok(LayerResult {
			output,
			exchange,
			token_usage,
			tool_calls: direct_tool_calls,
			api_time_ms: 0, // TODO: Add time tracking to processor
			tool_time_ms: 0,
			total_time_ms: 0,
		})
	}
}
