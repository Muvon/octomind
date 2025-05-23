// Session message operations

use super::core::ChatSession;
use crate::config::Config;
use crate::{log_info, log_debug};
use crate::session::openrouter;
use anyhow::Result;
use colored::Colorize;

impl ChatSession {
	// Save the session
	pub fn save(&self) -> Result<()> {
		self.session.save()
	}

	// Add a system message
	pub fn add_system_message(&mut self, content: &str) -> Result<()> {
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
		// Add message to session
		self.session.add_message("user", content);

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

	// Add an assistant message
	pub fn add_assistant_message(&mut self, content: &str, exchange: Option<openrouter::OpenRouterExchange>, config: &Config) -> Result<()> {
		// Add message to session
		let message = self.session.add_message("assistant", content);
		self.last_response = content.to_string();

		// Log the assistant response
		let _ = crate::session::logger::log_assistant_response(content);

		// Log the raw exchange if available
		if let Some(ex) = &exchange {
			let _ = crate::session::logger::log_raw_exchange(ex);
		}

		// Update token counts and estimated costs if we have usage data
		if let Some(ex) = &exchange {
			if let Some(usage) = &ex.usage {
				// Calculate regular and cached tokens
				let mut regular_prompt_tokens = usage.prompt_tokens;
				let mut cached_tokens = 0;

				// Check prompt_tokens_details for cached_tokens first
				if let Some(details) = &usage.prompt_tokens_details {
					if let Some(cached) = details.get("cached_tokens") {
						if let serde_json::Value::Number(num) = cached {
							if let Some(num_u64) = num.as_u64() {
								cached_tokens = num_u64;
								// Adjust regular tokens to account for cached tokens
								regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
							}
						}
					}
				}

				// Fall back to breakdown field if prompt_tokens_details didn't have cached tokens
				if cached_tokens == 0 && usage.prompt_tokens > 0 {
					if let Some(breakdown) = &usage.breakdown {
						if let Some(cached) = breakdown.get("cached") {
							if let serde_json::Value::Number(num) = cached {
								if let Some(num_u64) = num.as_u64() {
									cached_tokens = num_u64;
									// Adjust regular tokens to account for cached tokens
									regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
								}
							}
						}
					}
				}

				// Check for cached tokens in the base API response for models that report differently
				if cached_tokens == 0 && usage.prompt_tokens > 0 {
					if let Some(response) = &ex.response.get("usage") {
						if let Some(cached) = response.get("cached_tokens") {
							if let Some(num) = cached.as_u64() {
								cached_tokens = num;
								regular_prompt_tokens = usage.prompt_tokens.saturating_sub(cached_tokens);
							}
						}
					}
				}

				// Update session token counts
				self.session.info.input_tokens += regular_prompt_tokens;
				self.session.info.output_tokens += usage.completion_tokens;
				self.session.info.cached_tokens += cached_tokens;

				// Update current token tracking for auto-cache threshold logic
				// Only count input tokens, not completion tokens
				self.session.current_non_cached_tokens += regular_prompt_tokens;
				self.session.current_total_tokens += regular_prompt_tokens + cached_tokens;

				// Check if we should automatically move the cache marker
				if let Ok(true) = self.session.check_auto_cache_threshold(config) {
					log_info!("{}", "Auto-cache threshold reached - adding cache checkpoint at last user message.");
				}

				// If OpenRouter provided cost data, use it directly
				if let Some(cost) = usage.cost {
					// OpenRouter credits = dollars, use the value directly
					self.session.info.total_cost += cost;
					self.estimated_cost = self.session.info.total_cost;

					// Log the actual cost received from the API for debugging
					if config.openrouter.log_level.is_debug_enabled() {
						println!("Debug: Adding ${:.5} from OpenRouter API (total now: ${:.5})",
							cost, self.session.info.total_cost);

						// Check if there's a raw usage object with additional fields
						if let Some(raw_usage) = ex.response.get("usage") {
							log_debug!("Raw usage from response:");
							if let Ok(raw_str) = serde_json::to_string_pretty(raw_usage) {
								log_debug!("{}", raw_str);
							}
						}
					}
				} else {
					// No explicit cost data, look at the raw response to check if it contains cost data
					let cost_from_raw = ex.response.get("usage")
						.and_then(|u| u.get("cost"))
						.and_then(|c| c.as_f64());

					if let Some(cost) = cost_from_raw {
						// Use the cost value directly
						self.session.info.total_cost += cost;
						self.estimated_cost = self.session.info.total_cost;

						// Log that we had to fetch cost from raw response
						if config.openrouter.log_level.is_debug_enabled() {
							println!("Debug: Using cost from raw response: ${:.5} (total now: ${:.5})",
								cost, self.session.info.total_cost);
						}
					} else {
						// ERROR - OpenRouter did not provide cost data
						println!("{}", "ERROR: OpenRouter did not provide cost data. Make sure usage.include=true is set!".bright_red());

						// Dump the raw response JSON to debug
						if config.openrouter.log_level.is_debug_enabled() {
							println!("{}", "Raw OpenRouter response:".bright_red());
							if let Ok(resp_str) = serde_json::to_string_pretty(&ex.response) {
								println!("{}", resp_str);
							}

							// Check if usage tracking was explicitly requested
							let has_usage_flag = ex.request.get("usage")
								.and_then(|u| u.get("include"))
								.and_then(|i| i.as_bool())
								.unwrap_or(false);

							println!("{} {}", "Request had usage.include flag:".bright_yellow(), has_usage_flag);
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

			// If we have a raw exchange, save it as well
			if let Some(ex) = exchange {
				let exchange_json = serde_json::to_string(&ex)?;
				crate::session::append_to_session_file(session_file, &exchange_json)?;
			}
		}

		Ok(())
	}
}
