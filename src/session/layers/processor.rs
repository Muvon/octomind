use crate::config::Config;
use crate::session::{Message, Session, openrouter};
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
		_session: &Session,
	) -> Vec<Message> {
		let mut messages = Vec::new();

		// System message with layer-specific prompt
		// Only mark system messages as cached if the model supports it
		let should_cache = crate::session::model_utils::model_supports_caching(&self.config.model);

		// Process placeholders in the system prompt
		let processed_prompt = if self.config.system_prompt.contains("%{") {
			// Process placeholders if they exist
			let project_dir = std::env::current_dir().unwrap_or_default();
			crate::session::process_placeholders(&self.config.system_prompt, &project_dir)
		} else {
			// No placeholders, use the prompt as is
			self.config.system_prompt.clone()
		};

		messages.push(Message {
			role: "system".to_string(),
			content: processed_prompt,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: should_cache, // Only cache if model supports it
			tool_call_id: None, // No tool_call_id for system messages
			name: None, // No name for system messages
			tool_calls: None, // No tool_calls for system messages
		});

		// Add user message with the input
		messages.push(Message {
			role: "user".to_string(),
			content: input.to_string(),
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

		// Prepare input based on input_mode
		let processed_input = self.prepare_input(input, session);

		// Create a separate session context for this layer
		// This ensures each layer operates in isolation
		let mut layer_session = Vec::new();

		// Create messages for this layer and add to isolated session
		let messages = self.create_messages(&processed_input, session);
		layer_session.extend(messages.clone());

		// Call the model directly with session messages
		let (output, exchange, direct_tool_calls, _finish_reason) = openrouter::chat_completion(
			messages.clone(),
			&self.config.model,
			self.config.temperature,
			config
		).await?;

		// Check if the layer response contains tool calls
		if config.mcp.enabled && self.config.enable_tools {
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
					if !self.config.allowed_tools.is_empty() &&
					!self.config.allowed_tools.contains(&tool_call.tool_name) {
						println!("{} {} {}", "Tool".red(), tool_call.tool_name, "not allowed for this layer".red());
						continue;
					}

					let result = match crate::mcp::execute_layer_tool_call(tool_call, config, &self.config).await {
						Ok(res) => res,
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

					// Add the original messages to our layer session
					layer_session.extend(messages.clone());

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
					match openrouter::chat_completion(
						layer_session.clone(),
						&self.config.model,
						self.config.temperature,
						config
					).await {
						Ok((new_output, new_exchange, next_tool_calls, _finish_reason)) => {
							// Extract token usage if available
							let token_usage = new_exchange.usage.clone();

							// Return the result with the updated output
							return Ok(LayerResult {
								output: new_output,
								exchange: new_exchange,
								token_usage,
								tool_calls: next_tool_calls,
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
		})
	}
}
