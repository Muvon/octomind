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

// Summarize command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_summarize(session: &mut ChatSession, config: &Config) -> Result<CommandResult> {
	// Estimate current token usage
	let current_tokens = crate::session::estimate_message_tokens(&session.session.messages);

	// Use the smart full summarization logic
	match crate::session::chat::perform_smart_full_summarization(session, config).await {
		Ok(()) => {
			// Calculate new token count after summarization
			let new_tokens = crate::session::estimate_message_tokens(&session.session.messages);
			let tokens_saved = current_tokens.saturating_sub(new_tokens);

			Ok(CommandResult::HandledWithOutput(CommandOutput::Summarize {
				success: true,
				tokens_before: current_tokens,
				tokens_after: new_tokens,
				tokens_saved,
				summary: true,
			}))
		}
		Err(e) => Ok(CommandResult::HandledWithOutput(CommandOutput::Error {
			error: format!("Summarization failed: {}", e),
			context: Some(serde_json::json!({
				"tokens_before": current_tokens
			})),
		})),
	}
}
