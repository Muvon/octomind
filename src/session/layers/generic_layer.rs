use crate::config::Config;
use crate::session::{Message, Session};
use super::layer_trait::{Layer, LayerConfig, LayerResult};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use async_trait::async_trait;
use colored::Colorize;

/// Generic layer implementation that can work with any layer configuration
/// This replaces the need for specific layer type implementations
pub struct GenericLayer {
	config: LayerConfig,
}

impl GenericLayer {
	pub fn new(config: LayerConfig) -> Self {
		Self { config }
	}

	/// Create messages for the API based on the layer configuration
	fn create_messages(
		&self,
		input: &str,
		session: &Session,
		session_model: &str,
	) -> Vec<Message> {
		let mut messages = Vec::new();

		// Get the effective system prompt for this layer
		let system_prompt = self.config.get_effective_system_prompt();

		// Get the effective model for this layer
		let effective_model = self.config.get_effective_model(session_model);

		// Only mark system messages as cached if the model supports it
		let should_cache = crate::session::model_utils::model_supports_caching(&effective_model);

		messages.push(Message {
			role: "system".to_string(),
			content: system_prompt,
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: should_cache,
			tool_call_id: None,
			name: None,
			tool_calls: None,
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
			tool_call_id: None,
			name: None,
			tool_calls: None,
		});

		messages
	}

	/// Execute MCP tool calls for this layer using its own MCP configuration
	async fn execute_layer_tool_calls(
		&self,
		tool_calls: &[crate::mcp::McpToolCall],
		config: &Config,
	) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
		let mut results = Vec::new();
		let mut total_tool_time_ms = 0;

		for tool_call in tool_calls {
			println!("{} {}", "Tool call:".yellow(), tool_call.tool_name);

			// Check if this tool is allowed for this layer
			if !self.config.mcp.allowed_tools.is_empty() &&
				!self.config.mcp.allowed_tools.contains(&tool_call.tool_name) {
				println!("{} {} {}", "Tool".red(), tool_call.tool_name, "not allowed for this layer".red());
				continue;
			}

			// Create a layer-specific config that only includes this layer's MCP servers
			let layer_config = self.config.get_merged_config_for_layer(config);

			// Execute the tool call using the layer-specific configuration
			match crate::mcp::execute_layer_tool_call(tool_call, &layer_config, &self.config).await {
				Ok((result, tool_time_ms)) => {
					results.push(result);
					total_tool_time_ms += tool_time_ms;
				},
				Err(e) => {
					println!("{} {}", "Tool execution error:".red(), e);
					continue;
				}
			}
		}

		Ok((results, total_tool_time_ms))
	}
}

#[async_trait]
impl Layer for GenericLayer {
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
		// Track total layer processing time
		let layer_start = std::time::Instant::now();
		let mut total_api_time_ms = 0;
		let mut total_tool_time_ms = 0;

		// Check if operation was cancelled
		if operation_cancelled.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("Operation cancelled"));
		}

		// Get the effective model for this layer
		let effective_model = self.config.get_effective_model(&session.info.model);

		// Create messages for this layer
		let messages = self.create_messages(input, session, &session.info.model);

		// Create a merged config that uses this layer's MCP settings
		let layer_config = self.config.get_merged_config_for_layer(config);

		// Call the model with the layer's effective model and temperature
		let response = crate::session::chat_completion_with_provider(
			&messages,
			&effective_model,
			self.config.temperature,
			&layer_config
		).await?;

		let (output, exchange, direct_tool_calls, _finish_reason) = (
			response.content,
			response.exchange,
			response.tool_calls,
			response.finish_reason
		);

		// Track API time from the exchange
		if let Some(ref usage) = exchange.usage {
			if let Some(api_time) = usage.request_time_ms {
				total_api_time_ms += api_time;
			}
		}

		// Check if the layer response contains tool calls and if MCP is enabled for this layer
		if !self.config.mcp.server_refs.is_empty() {
			// First try to use directly returned tool calls, then fall back to parsing if needed
			let tool_calls = if let Some(ref calls) = direct_tool_calls {
				calls
			} else {
				&crate::mcp::parse_tool_calls(&output)
			};

			// If there are tool calls, process them using this layer's MCP configuration
			if !tool_calls.is_empty() {
				let output_clone = output.clone();

				// Execute all tool calls and collect results using layer-specific MCP config
				let (tool_results, tool_execution_time) = self.execute_layer_tool_calls(tool_calls, config).await?;
				total_tool_time_ms += tool_execution_time;

				// If we have results, send them back to the model to get a final response
				if !tool_results.is_empty() {
					println!("{}", "Processing tool results...".cyan());

					// Create a new session context for tool result processing
					let mut layer_session = messages.clone();

					// Add assistant's response with tool calls
					layer_session.push(Message {
						role: "assistant".to_string(),
						content: output_clone,
						timestamp: std::time::SystemTime::now()
							.duration_since(std::time::UNIX_EPOCH)
							.unwrap_or_default()
							.as_secs(),
						cached: false,
						tool_call_id: None,
						name: None,
						tool_calls: None,
					});

					// Add each tool result as a tool message
					for tool_result in &tool_results {
						layer_session.push(Message {
							role: "tool".to_string(),
							content: serde_json::to_string(&tool_result.result).unwrap_or_default(),
							timestamp: std::time::SystemTime::now()
								.duration_since(std::time::UNIX_EPOCH)
								.unwrap_or_default()
								.as_secs(),
							cached: false,
							tool_call_id: Some(tool_result.tool_id.clone()),
							name: Some(tool_result.tool_name.clone()),
							tool_calls: None,
						});
					}

					// Call the model again with tool results using this layer's model and config
					match crate::session::chat_completion_with_provider(
						&layer_session,
						&effective_model,
						self.config.temperature,
						&layer_config
					).await {
						Ok(response) => {
							// Track API time from the second exchange
							if let Some(ref usage) = response.exchange.usage {
								if let Some(api_time) = usage.request_time_ms {
									total_api_time_ms += api_time;
								}
							}

							// Extract token usage if available
							let token_usage = response.exchange.usage.clone();

							// Calculate total layer processing time
							let layer_duration = layer_start.elapsed();
							let total_time_ms = layer_duration.as_millis() as u64;

							// Return the result with the updated output and time tracking
							return Ok(LayerResult {
								output: response.content,
								exchange: response.exchange,
								token_usage,
								tool_calls: response.tool_calls,
								api_time_ms: total_api_time_ms,
								tool_time_ms: total_tool_time_ms,
								total_time_ms,
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

		// Calculate total layer processing time
		let layer_duration = layer_start.elapsed();
		let total_time_ms = layer_duration.as_millis() as u64;

		// Return the result with time tracking
		Ok(LayerResult {
			output,
			exchange,
			token_usage,
			tool_calls: direct_tool_calls,
			api_time_ms: total_api_time_ms,
			tool_time_ms: total_tool_time_ms,
			total_time_ms,
		})
	}
}
