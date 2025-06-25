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

// Truncate command handler

use super::super::core::ChatSession;
use super::utils::format_number;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

pub async fn handle_truncate(session: &mut ChatSession, config: &Config) -> Result<bool> {
	// Perform smart truncation processing once
	println!(
		"{}",
		"Performing simple boundary truncation...".bright_cyan()
	);

	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&session.session.messages);
	println!(
		"{}",
		format!(
			"Current context size: {} tokens",
			format_number(current_tokens as u64)
		)
		.bright_blue()
	);

	// Use the simple boundary truncation logic for manual /truncate command
	match crate::session::chat::perform_simple_boundary_truncation(session, config, current_tokens)
		.await
	{
		Ok(()) => {
			// Calculate new token count after truncation
			let new_tokens = crate::session::estimate_message_tokens(&session.session.messages);
			let tokens_saved = current_tokens.saturating_sub(new_tokens);

			if tokens_saved > 0 {
				println!(
                    "{}",
                    format!(
                        "Simple boundary truncation completed: {} tokens removed, new context size: {} tokens",
                        format_number(tokens_saved as u64),
                        format_number(new_tokens as u64)
                    )
                    .bright_green()
                );
			} else {
				println!(
					"{}",
					"No truncation needed - context size is within optimal range".bright_yellow()
				);
			}
		}
		Err(e) => {
			println!(
				"{}: {}",
				"Simple boundary truncation failed".bright_red(),
				e
			);
		}
	}

	Ok(false)
}
