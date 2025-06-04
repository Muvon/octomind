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

// Context reduction for session optimization

use super::animation::show_loading_animation;
use crate::config::Config;
use crate::session::chat::session::ChatSession;
use anyhow::Result;
use colored::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Process context reduction - smart truncation with summarization
/// Simply adds a summarization prompt and lets the normal session flow handle it
pub async fn perform_context_reduction(
	chat_session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: Arc<AtomicBool>,
) -> Result<()> {
	println!("{}", "Summarizing conversation context...".cyan());

	// Check if there's anything to summarize (exclude system message)
	let conversation_messages = chat_session
		.session
		.messages
		.iter()
		.filter(|m| m.role != "system")
		.count();

	if conversation_messages == 0 {
		println!("{}", "No conversation to summarize".yellow());
		return Ok(());
	}

	// Store original message count for logging
	let original_message_count = chat_session.session.messages.len();

	// Simply add the summarization prompt as a user message
	let summarization_prompt = "Please memorize critical and important info for future reference first and after create a concise summary of our conversation that preserves all important technical details, decisions made, files modified, and context needed for future development. Focus on actionable information and key outcomes.";

	chat_session.add_user_message(summarization_prompt)?;

	// Create a task to show loading animation with current cost
	let animation_cancel = operation_cancelled.clone();
	let current_cost = chat_session.session.info.total_cost;
	let animation_task = tokio::spawn(async move {
		let _ = show_loading_animation(animation_cancel, current_cost).await;
	});

	// Use the same API flow as the normal session
	let api_result = crate::session::chat_completion_with_provider(
		&chat_session.session.messages,
		&chat_session.model,
		chat_session.temperature,
		config,
	)
	.await;

	// Stop the animation
	operation_cancelled.store(true, Ordering::SeqCst);
	let _ = animation_task.await;

	// Process the response with the normal flow (handles tool calls, etc.)
	let response_result = match api_result {
		Ok(response) => {
			// Use the normal process_response flow which handles tool calls automatically
			let process_result = super::response::process_response(
				response.content.clone(),
				response.exchange,
				response.tool_calls,
				response.finish_reason,
				chat_session,
				config,
				role, // Use the current role instead of hardcoding "developer"
				operation_cancelled.clone(),
			)
			.await;

			match process_result {
				Ok(()) => Ok(response.content),
				Err(e) => Err(e),
			}
		}
		Err(e) => Err(e),
	};

	match response_result {
		Ok(summary_content) => {
			// Log restoration point for recovery
			let _ = crate::session::logger::log_restoration_point(
				&chat_session.session.info.name,
				"Context summarization",
				&summary_content,
			);

			// Log to session file as well
			if let Some(session_file) = &chat_session.session.session_file {
				let restoration_data = serde_json::json!({
					"type": "context_reduction",
					"summary": summary_content,
					"original_message_count": original_message_count,
					"timestamp": std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs()
				});
				let restoration_json = serde_json::to_string(&restoration_data)?;
				let _ = crate::session::append_to_session_file(
					session_file,
					&format!("RESTORATION_POINT: {}", restoration_json),
				);
			}

			println!("{}", "Context summarization complete".bright_green());

			// SMART TRUNCATION: Keep only system message + summary as assistant message
			let system_message = chat_session
				.session
				.messages
				.iter()
				.find(|m| m.role == "system")
				.cloned();

			// Clear all messages
			chat_session.session.messages.clear();

			// Restore system message
			if let Some(system) = system_message {
				chat_session.session.messages.push(system);
			}

			// Add the summary as an assistant message (this is our new context)
			chat_session
				.session
				.add_message("assistant", &summary_content);
			let last_index = chat_session.session.messages.len() - 1;
			chat_session.session.messages[last_index].cached = true; // Mark for caching

			// Reset token tracking for fresh start
			chat_session.session.current_non_cached_tokens = 0;
			chat_session.session.current_total_tokens = 0;

			// Update cache checkpoint time
			chat_session.session.last_cache_checkpoint_time = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs();

			println!(
				"{}",
				"Session context reduced to essential summary".bright_green()
			);
			println!(
				"{}",
				"You can now continue the conversation with optimized context".bright_cyan()
			);

			// Auto-commit with octocode if available
			if let Err(e) = auto_commit_with_octocode().await {
				// Don't fail the entire operation if commit fails, just warn
				println!("{}: {}", "Warning: Auto-commit failed".bright_yellow(), e);
			}

			// Save the updated session
			chat_session.save()?;

			Ok(())
		}
		Err(e) => {
			// Remove the summarization prompt since it failed
			if let Some(last_msg) = chat_session.session.messages.last() {
				if last_msg.role == "user"
					&& last_msg.content.contains("Please create a concise summary")
				{
					chat_session.session.messages.pop();
				}
			}

			println!(
				"{}: {}",
				"Error during context summarization".bright_red(),
				e
			);
			Err(anyhow::anyhow!("Context summarization failed: {}", e))
		}
	}
}

/// Auto-commit changes using octocode if the binary is available
async fn auto_commit_with_octocode() -> Result<()> {
	// Check if octocode binary is available in PATH
	let octocode_check = tokio::process::Command::new("which")
		.arg("octocode")
		.output()
		.await;

	match octocode_check {
		Ok(output) if output.status.success() => {
			// octocode is available, proceed with commit
			println!(
				"{}",
				"🔄 Auto-committing changes with octocode...".bright_blue()
			);

			let commit_result = tokio::process::Command::new("octocode")
				.args(["commit", "-a", "-y"])
				.output()
				.await;

			match commit_result {
				Ok(output) => {
					if output.status.success() {
						let stdout = String::from_utf8_lossy(&output.stdout);
						if !stdout.trim().is_empty() {
							println!("{}", stdout.trim().bright_green());
						}
						println!(
							"{}",
							"✅ Changes auto-committed successfully".bright_green()
						);
					} else {
						let stderr = String::from_utf8_lossy(&output.stderr);
						if stderr.contains("no changes") || stderr.contains("nothing to commit") {
							println!("{}", "ℹ️  No changes to commit".bright_blue());
						} else {
							return Err(anyhow::anyhow!("octocode commit failed: {}", stderr));
						}
					}
				}
				Err(e) => {
					return Err(anyhow::anyhow!("Failed to execute octocode commit: {}", e));
				}
			}
		}
		Ok(_) => {
			// which command succeeded but octocode not found (empty output)
			println!(
				"{}",
				"ℹ️  octocode not found in PATH, skipping auto-commit".bright_blue()
			);
		}
		Err(_) => {
			// which command failed (probably on Windows or which is not available)
			// Try direct execution as fallback
			let direct_check = tokio::process::Command::new("octocode")
				.arg("--version")
				.output()
				.await;

			match direct_check {
				Ok(output) if output.status.success() => {
					// octocode is available, proceed with commit
					println!(
						"{}",
						"🔄 Auto-committing changes with octocode...".bright_blue()
					);

					let commit_result = tokio::process::Command::new("octocode")
						.args(["commit", "-a", "-y"])
						.output()
						.await;

					match commit_result {
						Ok(output) => {
							if output.status.success() {
								let stdout = String::from_utf8_lossy(&output.stdout);
								if !stdout.trim().is_empty() {
									println!("{}", stdout.trim().bright_green());
								}
								println!(
									"{}",
									"✅ Changes auto-committed successfully".bright_green()
								);
							} else {
								let stderr = String::from_utf8_lossy(&output.stderr);
								if stderr.contains("no changes")
									|| stderr.contains("nothing to commit")
								{
									println!("{}", "ℹ️  No changes to commit".bright_blue());
								} else {
									return Err(anyhow::anyhow!(
										"octocode commit failed: {}",
										stderr
									));
								}
							}
						}
						Err(e) => {
							return Err(anyhow::anyhow!(
								"Failed to execute octocode commit: {}",
								e
							));
						}
					}
				}
				_ => {
					// octocode not available
					println!(
						"{}",
						"ℹ️  octocode not available, skipping auto-commit".bright_blue()
					);
				}
			}
		}
	}

	Ok(())
}
