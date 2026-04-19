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

// Session message operations

use super::core::ChatSession;
use crate::config::Config;
use crate::session::ProviderExchange;
use crate::{log_debug, log_info};
use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;

impl ChatSession {
	// Sync runtime state from ChatSession fields to session.info (for persistence)
	fn sync_runtime_state(&mut self) {
		self.session.info.cache_next_user_message = self.cache_next_user_message;
		self.session.info.spending_threshold_checkpoint = self.spending_threshold_checkpoint;
		self.session.info.compression_hint_count = self.compression_hint_count;
		self.session.info.last_compression_hint_shown = self.last_compression_hint_shown;
	}

	// Save the session (syncs runtime state first)
	pub fn save(&mut self) -> Result<()> {
		self.sync_runtime_state();
		self.session.save()
	}

	// Check if spending threshold is exceeded and prompt user if needed
	pub fn check_spending_threshold(&mut self, config: &Config) -> Result<bool> {
		// If threshold is 0 or negative, feature is disabled
		if config.max_session_spending_threshold <= 0.0 {
			return Ok(true); // Continue without checking
		}

		let current_cost = self.session.info.total_cost;
		let threshold = config.max_session_spending_threshold;
		let cost_since_checkpoint = current_cost - self.spending_threshold_checkpoint;

		// Check if we've exceeded the threshold since last checkpoint
		if cost_since_checkpoint >= threshold {
			// In ACP/WebSocket mode stdout/stderr are reserved for protocol — auto-decline silently
			if crate::logging::tracing_setup::is_structured_output_mode() {
				return Ok(false);
			}

			use colored::*;
			use std::io::{self, Write};

			println!();
			println!(
				"{}",
				"⚠️  SPENDING THRESHOLD REACHED ⚠️".bright_yellow().bold()
			);
			println!(
				"{} ${:.5}",
				"Current session cost:".bright_cyan(),
				current_cost
			);
			println!("{} ${:.5}", "Threshold:".bright_cyan(), threshold);
			println!(
				"{} ${:.5}",
				"Cost since last checkpoint:".bright_cyan(),
				cost_since_checkpoint
			);
			println!();
			println!(
				"{}",
				"Continuing may result in additional charges.".bright_yellow()
			);

			// Auto-decline in non-interactive mode (run command, piped input, etc.)
			if !std::io::stdin().is_terminal() {
				println!(
					"{}",
					"Spending threshold reached but automatically declining in non-interactive mode. Stopping execution.".bright_red()
				);
				return Ok(false);
			}

			// Interactive mode - ask user for confirmation
			print!(
				"{}",
				"Do you want to continue? (y/N): ".bright_white().bold()
			);
			io::stdout().flush()?;

			let mut input = String::new();
			io::stdin().read_line(&mut input)?;
			let response = input.trim().to_lowercase();

			if response == "y" || response == "yes" {
				// User confirmed, reset checkpoint to current cost
				self.spending_threshold_checkpoint = current_cost;
				println!(
					"{}",
					"✓ Continuing session. Threshold checkpoint reset.".bright_green()
				);
				println!();
				Ok(true)
			} else {
				println!(
					"{}",
					"✗ Session cancelled by user due to spending threshold.".bright_red()
				);
				Ok(false)
			}
		} else {
			Ok(true) // Under threshold, continue
		}
	}

	// Check if request spending threshold is exceeded and stop execution if needed
	pub fn check_request_spending_threshold(&mut self, config: &Config) -> Result<bool> {
		// If threshold is 0 or negative, feature is disabled
		if config.max_request_spending_threshold <= 0.0 {
			return Ok(true); // Continue without checking
		}

		let current_cost = self.session.info.total_cost;
		let threshold = config.max_request_spending_threshold;
		let cost_since_request_start = current_cost - self.request_spending_checkpoint;

		// Check if we've exceeded the threshold since request start
		if cost_since_request_start >= threshold {
			// In ACP/WebSocket mode stdout/stderr are reserved for protocol — suppress UI output
			if !crate::logging::tracing_setup::is_structured_output_mode() {
				use colored::*;

				println!();
				println!(
					"{}",
					"⚠️  REQUEST SPENDING THRESHOLD EXCEEDED ⚠️"
						.bright_red()
						.bold()
				);
				println!(
					"{} ${:.5}",
					"Current request cost:".bright_cyan(),
					cost_since_request_start
				);
				println!("{} ${:.5}", "Threshold:".bright_cyan(), threshold);
				println!(
					"{} ${:.5}",
					"Total session cost:".bright_cyan(),
					current_cost
				);
				println!();
				println!(
					"{}",
					"Request execution stopped to prevent overspending.".bright_red()
				);
				println!();
			}

			return Ok(false); // Stop execution
		}

		Ok(true) // Under threshold, continue
	}

	// Initialize request spending checkpoint at the start of a new request
	pub fn start_request_spending_tracking(&mut self) {
		self.request_spending_checkpoint = self.session.info.total_cost;
	}

	// Write the initial SUMMARY entry the first time we touch the session file.
	// Called before the first message write so the file always starts with metadata.
	fn ensure_file_initialized(&mut self) -> Result<()> {
		if let Some(session_file) = &self.session.session_file {
			if !session_file.exists() {
				let summary_entry = serde_json::json!({
					"type": "SUMMARY",
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					"session_info": &self.session.info
				});
				let session_file = session_file.clone();
				crate::session::append_to_session_file(
					&session_file,
					&serde_json::to_string(&summary_entry)?,
				)?;
			}
		}
		Ok(())
	}
	// Add a system message
	pub fn add_system_message(&mut self, content: &str) -> Result<()> {
		// Lazily create the session file with its initial SUMMARY on first write
		// Must happen BEFORE logger which also writes to the same file
		self.ensure_file_initialized()?;

		// Log to raw session log
		let _ = crate::session::logger::log_system_message(&self.session.info.name, content);

		// Add message to session
		self.session.add_message("system", content);

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}

		Ok(())
	}

	// Add a user message
	pub fn add_user_message(&mut self, content: &str) -> Result<()> {
		// Log to raw session log
		let _ = crate::session::logger::log_user_input(&self.session.info.name, content);

		// Add message to session with image if available
		let mut message = self.session.add_message("user", content);

		// Attach pending image if available
		if let Some(image_attachment) = self.take_pending_image() {
			message.images = Some(vec![image_attachment]);
			// Update the message in the session
			if let Some(last_msg) = self.session.messages.last_mut() {
				last_msg.images = message.images.clone();
			}
			if !crate::logging::tracing_setup::is_structured_output_mode() {
				println!("{}", "📎 Image attached to message".bright_green());
			}
		}

		// Attach pending video if available
		if let Some(video_attachment) = self.take_pending_video() {
			message.videos = Some(vec![video_attachment]);
			// Update the message in the session
			if let Some(last_msg) = self.session.messages.last_mut() {
				last_msg.videos = message.videos.clone();
			}
			if !crate::logging::tracing_setup::is_structured_output_mode() {
				println!("{}", "🎬 Video attached to message".bright_green());
			}
		}

		// Check if we should cache this user message
		if self.cache_next_user_message {
			let supports_caching = crate::session::model_supports_caching(&self.session.info.model);
			if supports_caching {
				let cache_manager = crate::session::cache::CacheManager::new();
				if let Ok(true) = cache_manager
					.apply_cache_to_current_user_message(&mut self.session, supports_caching)
				{
					if !crate::logging::tracing_setup::is_structured_output_mode() {
						use colored::*;
						println!(
							"{}",
							"✓ Current user message marked for caching".bright_green()
						);
					}
				}
			}
			// Reset the flag after applying (or attempting to apply) cache
			self.cache_next_user_message = false;
		}

		// Log the user message if not already logged from input
		if !content.starts_with("<fnr>") {
			let _ = crate::session::logger::log_user_request(content);
		}

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}

		Ok(())
	}

	// Add a tool message
	pub fn add_tool_message(
		&mut self,
		content: &str,
		tool_call_id: &str,
		tool_name: &str,
		_config: &Config,
	) -> Result<()> {
		// Log to raw session log
		let _ = crate::session::logger::log_tool_result(
			&self.session.info.name,
			tool_call_id,
			&serde_json::json!({"output": content}),
			0, // No timing info available in this context
		);

		// Create the tool message
		let tool_message = crate::session::Message {
			role: "tool".to_string(),
			content: content.to_string(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_call_id: Some(tool_call_id.to_string()),
			name: Some(tool_name.to_string()),
			..Default::default()
		};

		// Add message to session
		self.session.messages.push(tool_message);

		// Update token tracking for auto-cache threshold logic
		// Tool messages count as "input" for the next API call, so we track them as non-cached input tokens
		let tool_content_tokens = crate::session::estimate_tokens(content) as u64;
		let tool_overhead_tokens = 8; // Rough estimate for role + tool_call_id + name overhead

		// Update the session's current token tracking
		// This ensures tool message tokens are counted toward auto-cache thresholds
		// Tool messages are input tokens (they go to the API as input), not output tokens
		let tool_input_tokens = tool_content_tokens + tool_overhead_tokens;
		self.session.info.current_total_tokens += tool_input_tokens;
		self.session.info.current_non_cached_tokens += tool_input_tokens;

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&self.session.messages.last().unwrap())?;
			crate::session::append_to_session_file(session_file, &message_json)?;
		}

		Ok(())
	}

	// Add an assistant message
	pub fn add_assistant_message(
		&mut self,
		content: &str,
		exchange: Option<ProviderExchange>,
		config: &Config,
		role: &str,
	) -> Result<()> {
		// Log to raw session log
		let _ = crate::session::logger::log_assistant_response(&self.session.info.name, content);

		// Log raw API exchange if available
		if let Some(ref ex) = exchange {
			let _ = crate::session::logger::log_api_request(&self.session.info.name, &ex.request);
			let _ = crate::session::logger::log_api_response(
				&self.session.info.name,
				&ex.response,
				ex.usage.as_ref(),
			);
		}

		// Add message to session
		let message = self.session.add_message("assistant", content);
		self.last_response = content.to_string();

		// Log the raw exchange if available (legacy)
		if let Some(ex) = &exchange {
			let _ = crate::session::logger::log_raw_exchange(&self.session.info.name, ex);
		}

		// Update token counts and estimated costs if we have usage data
		if let Some(ex) = &exchange {
			if let Some(usage) = &ex.usage {
				// Track API time if available
				if let Some(api_time_ms) = usage.request_time_ms {
					self.session.info.total_api_time_ms += api_time_ms;
				}

				// CACHE-AWARE COMPRESSION: Track API calls for amortized cost analysis
				// Each API call = potential cache write/read, critical for compression economics
				self.session.info.total_api_calls += 1;

				// Update session token counts using octolib data directly
				let cache_manager = crate::session::cache::CacheManager::new();
				cache_manager.update_token_tracking(
					&mut self.session,
					usage.input_tokens, // Non-cached input tokens from API
					usage.output_tokens,
					usage.cache_read_tokens,
					usage.cache_write_tokens,
					usage.reasoning_tokens,
				);

				// Check if we should automatically move the cache marker
				let cache_manager = crate::session::cache::CacheManager::new();
				let supports_caching =
					crate::session::model_supports_caching(&self.session.info.model);
				if let Ok(true) = cache_manager.check_and_apply_auto_cache_threshold(
					&mut self.session,
					config,
					supports_caching,
					role,
				) {
					log_info!(
						"{}",
						"Auto-cache threshold reached - cache checkpoint applied."
					);
				}

				// If OpenRouter provided cost data, use it directly
				if let Some(cost) = usage.cost {
					// OpenRouter credits = dollars, use the value directly
					self.session.info.total_cost += cost;
					self.estimated_cost = self.session.info.total_cost;

					// Log the actual cost received from the API for debugging
					log_debug!(
						"Adding ${:.5} from OpenRouter API (total now: ${:.5})",
						cost,
						self.session.info.total_cost
					);

					// Check if there's a raw usage object with additional fields
					if let Some(raw_usage) = ex.response.get("usage") {
						log_debug!("Raw usage from response:");
						if let Ok(raw_str) = serde_json::to_string_pretty(raw_usage) {
							log_debug!("{}", raw_str);
						}
					}
				} else {
					// No explicit cost data, look at the raw response to check if it contains cost data
					let cost_from_raw = ex
						.response
						.get("usage")
						.and_then(|u| u.get("cost"))
						.and_then(|c| c.as_f64());

					if let Some(cost) = cost_from_raw {
						// Use the cost value directly
						self.session.info.total_cost += cost;
						self.estimated_cost = self.session.info.total_cost;

						// Log that we had to fetch cost from raw response
						log_debug!(
							"Using cost from raw response: ${:.5} (total now: ${:.5})",
							cost,
							self.session.info.total_cost
						);
					} else {
						// Provider did not provide cost data - this is normal for some providers (e.g., Ollama)
						let provider_name = &ex.provider;
						log_debug!("{} did not provide cost data.", provider_name);

						// Dump the raw response JSON to debug
						log_debug!("Raw {} response:", provider_name);
						if let Ok(resp_str) = serde_json::to_string_pretty(&ex.response) {
							log_debug!("{}", resp_str);
						}

						// Check if usage tracking was explicitly requested (OpenRouter-specific)
						if provider_name == "openrouter" {
							let has_usage_flag = ex
								.request
								.get("usage")
								.and_then(|u| u.get("include"))
								.and_then(|i| i.as_bool())
								.unwrap_or(false);

							log_debug!(
								"{} request had usage.include flag: {}",
								provider_name,
								has_usage_flag
							);
							if !has_usage_flag {
								log_debug!(
									"Make sure usage.include=true is set for {} to get cost data",
									provider_name
								);
							}
						}
					}
				}

				// Update session duration
				let current_time = std::time::SystemTime::now()
					.duration_since(std::time::UNIX_EPOCH)
					.unwrap_or_default()
					.as_secs();
				let start_time = self.session.info.created_at;
				self.session.info.duration_seconds = current_time - start_time;
			}
		}

		// Save to session file
		if let Some(session_file) = &self.session.session_file {
			let message_json = serde_json::to_string(&message)?;
			crate::session::append_to_session_file(session_file, &message_json)?;

			// If we have a raw exchange, save it inline in session file for complete restoration
			if let Some(ex) = exchange {
				// Save API request and response as separate prefixed lines for debugging
				let _ =
					crate::session::logger::log_api_request(&self.session.info.name, &ex.request);
				let _ = crate::session::logger::log_api_response(
					&self.session.info.name,
					&ex.response,
					ex.usage.as_ref(),
				);
			}

			// Log session stats snapshot after each assistant response
			let _ = crate::session::logger::log_session_stats(
				&self.session.info.name,
				&self.session.info,
			);
		}

		Ok(())
	}
}
