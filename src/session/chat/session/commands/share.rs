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

//! /share — upload the current session log and open the receipt URL.

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::session::share;
use anyhow::Result;

pub async fn handle_share(session: &ChatSession) -> Result<CommandResult> {
	let session_file = match session.session.session_file.as_ref() {
		Some(p) => p.clone(),
		None => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: "No session file available to share.".to_string(),
					context: Some(serde_json::json!({
						"hint": "Sessions are auto-saved after each interaction. Try sending a message first."
					})),
				},
			)));
		}
	};

	match share::share_session(&session_file).await {
		Ok(result) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Share {
				id: result.id,
				url: result.url,
			},
		))),
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Failed to share session: {}", e),
				context: Some(serde_json::json!({
					"hint": "Override the host with OCTOMIND_SHARE_URL=<url> if you're testing against a local dev server."
				})),
			},
		))),
	}
}
