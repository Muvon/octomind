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
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_truncate(
	session: &mut ChatSession,
	config: &Config,
	role: &str,
) -> Result<CommandResult> {
	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&session.session.messages);

	// Use the simple boundary truncation logic for manual /truncate command
	match crate::session::chat::perform_simple_boundary_truncation(
		session,
		config,
		current_tokens,
		role,
	)
	.await
	{
		Ok(()) => {
			// Calculate new token count after truncation
			let new_tokens = crate::session::estimate_message_tokens(&session.session.messages);
			let tokens_saved = current_tokens.saturating_sub(new_tokens);

			Ok(CommandResult::HandledWithOutput(CommandOutput::Truncate {
				success: true,
				tokens_before: current_tokens,
				tokens_after: new_tokens,
				tokens_saved,
			}))
		}
		Err(e) => Ok(CommandResult::HandledWithOutput(CommandOutput::Error {
			error: format!("Truncation failed: {}", e),
			context: Some(serde_json::json!({
				"tokens_before": current_tokens
			})),
		})),
	}
}
