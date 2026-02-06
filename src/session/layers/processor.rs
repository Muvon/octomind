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

use super::layer_trait::{Layer, LayerConfig, LayerResult};
use crate::config::Config;
use crate::session::{Message, Session};
use anyhow::Result;
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
	pub fn create_messages(&self, input: &str, session: &Session) -> Vec<Message> {
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
			..Default::default()
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
			..Default::default()
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
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<LayerResult> {
		// Check if operation was cancelled
		if *operation_cancelled.borrow() {
			return Err(anyhow::anyhow!("Operation cancelled"));
		}

		// Track total processing time
		let total_start = std::time::Instant::now();
		let mut api_time_ms = 0u64;
		let mut tool_time_ms = 0u64;

		// Get the effective model for this layer
		let effective_model = self.config.get_effective_model(&session.info.model);

		// Create messages for this layer
		let messages = self.create_messages(input, session);

		// Track initial API call time
		let api_start = std::time::Instant::now();

		// Call the model directly with session messages
		let response = crate::session::chat_completion_with_provider(
			crate::session::ChatCompletionProviderParams {
				messages: &messages,
				model: &effective_model,
				temperature: self.config.temperature,
				top_p: self.config.top_p,
				top_k: self.config.top_k,
				max_tokens: self.config.max_tokens,
				config,
				max_retries: 0, // Default max_retries for layer processor
				cancellation_token: None,
			},
		)
		.await?;

		// Add initial API call time
		api_time_ms += api_start.elapsed().as_millis() as u64;

		let (output, exchange, direct_tool_calls, _finish_reason) = (
			response.content,
			response.exchange,
			response.tool_calls,
			response.finish_reason,
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

					// Check if tool is allowed for this layer using pattern-based validation
					let server_name =
						crate::mcp::tool_map::get_tool_server_name(&tool_call.tool_name)
							.unwrap_or_else(|| "unknown".to_string());

					if !self
						.config
						.mcp
						.is_tool_allowed(&tool_call.tool_name, &server_name)
					{
						println!(
							"{} {} {}",
							"Tool".red(),
							tool_call.tool_name,
							"not allowed for this layer".red()
						);
						continue;
					}

					// Create a layer-specific config that only includes this layer's MCP servers
					let layer_config = self.config.get_merged_config_for_layer(config);

					let result = match crate::mcp::execute_layer_tool_call(
						tool_call,
						&layer_config,
						&self.config,
						Some(operation_cancelled.clone()), // FIXED: Pass cancellation token
					)
					.await
					{
						Ok((res, single_tool_time_ms)) => {
							// Accumulate tool execution time
							tool_time_ms += single_tool_time_ms;
							res
						}
						Err(e) => {
							crate::log_error!("{} {}", "Tool execution error:", e);
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
						..Default::default()
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
							name: Some(tool_result.tool_name.clone()),       // Include the tool name
							..Default::default()
						});
					}

					// Call the model again with tool results
					// Important: We use THIS LAYER'S model to process the function call results
					let api_start_tool_processing = std::time::Instant::now();

					match crate::session::chat_completion_with_provider(
						crate::session::ChatCompletionProviderParams {
							messages: &layer_session,
							model: &effective_model,
							temperature: self.config.temperature,
							top_p: self.config.top_p,
							top_k: self.config.top_k,
							max_tokens: self.config.max_tokens,
							config,
							max_retries: 0,
							cancellation_token: None,
						},
					)
					.await
					{
						Ok(response) => {
							api_time_ms += api_start_tool_processing.elapsed().as_millis() as u64;

							// Extract token usage if available
							let token_usage = response.exchange.usage.clone();

							// Calculate total processing time
							let total_time_ms = total_start.elapsed().as_millis() as u64;

							// Return the result with the updated output
							return Ok(LayerResult {
								outputs: vec![response.content],
								exchange: response.exchange,
								token_usage,
								tool_calls: response.tool_calls,
								api_time_ms,
								tool_time_ms,
								total_time_ms,
							});
						}
						Err(e) => {
							crate::log_error!("{} {}", "Error processing tool results:", e);
							// Continue with the original output
						}
					}
				}
			}
		}

		// Extract token usage if available
		let token_usage = exchange.usage.clone();

		// Calculate total processing time
		let total_time_ms = total_start.elapsed().as_millis() as u64;

		// Return the result
		Ok(LayerResult {
			outputs: vec![output],
			exchange,
			token_usage,
			tool_calls: direct_tool_calls,
			api_time_ms,
			tool_time_ms,
			total_time_ms,
		})
	}
}
