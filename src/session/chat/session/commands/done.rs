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

// /done command handler - finalize current task with context reduction and initial message restoration

use super::super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

/// Finalize current task with context reduction and initial message restoration
///
/// This command:
/// 1. Performs context reduction (summarization)
/// 2. Clears plan data
/// 3. Re-adds welcome message and custom instructions file
/// 4. Resets session for fresh layered processing
/// 5. Returns first_message_processed reset flag
pub async fn handle_done(
	session: &mut ChatSession,
	config: &Config,
	role: &str,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<(bool, bool)> {
	// Returns (exit_flag, reset_first_message_processed)
	println!(
		"{}",
		"🎯 /done command initiated - Finalizing current task...".bright_blue()
	);

	// Clear plan data
	if let Err(e) = crate::mcp::dev::plan::clear_plan_data().await {
		crate::log_debug!("Failed to clear plan data: {}", e);
	}

	// Apply reducer functionality to optimize context
	let result = crate::session::chat::context_reduction::perform_context_reduction(
		session,
		config,
		role,
		operation_cancelled,
	)
	.await;

	if let Err(e) = result {
		println!(
			"{}: {}",
			"❌ /done command failed - Error performing context reduction".bright_red(),
			e
		);
	} else {
		println!(
			"{}",
			"✅ Task finalized and context optimized. Re-adding initial messages...".bright_cyan()
		);

		// CRITICAL FIX: Re-add initial messages (welcome + custom instructions)
		// Get current directory - use thread-local if set (ACP/WebSocket), otherwise process cwd
		let current_dir = crate::mcp::get_thread_working_directory();
		match crate::session::chat::session::get_initial_messages(config, role, &current_dir).await
		{
			Ok(initial_messages) => {
				let system_msg_count = session
					.session
					.messages
					.iter()
					.take_while(|m| m.role == "system")
					.count();

				// Insert initial messages after system message(s)
				for (i, msg) in initial_messages.into_iter().enumerate() {
					session.session.messages.insert(system_msg_count + i, msg);
				}

				// Save the session with re-added messages
				if let Err(e) = session.save() {
					println!("{}: {}", "Failed to save session".bright_red(), e);
				} else {
					println!(
						"{}",
						"✅ /done complete: Welcome message and custom instructions file re-added."
							.bright_green()
					);
				}
			}
			Err(e) => {
				println!(
					"{}: {}",
					"Failed to re-add initial messages".bright_yellow(),
					e
				);
			}
		}

		println!(
			"{}",
			"\n🚀 Next message will be processed through the full layered architecture."
				.bright_green()
		);

		// EditorConfig formatting has been removed to simplify dependencies
		// Users can apply EditorConfig formatting manually or through their IDE
	}

	// Return flags: don't exit session, reset first_message_processed to false
	Ok((false, true))
}
