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

//! /analyze — start a localhost HTTP bridge serving the current session log,
//! print the octomind.run viewer URL. The browser fetches from the bridge
//! using a short-lived token; data never traverses the public network.

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::session::share;
use anyhow::Result;

pub async fn handle_analyze(session: &ChatSession) -> Result<CommandResult> {
	let session_file = match session.session.session_file.as_ref() {
		Some(p) => p.clone(),
		None => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: "No session file available to analyze.".to_string(),
					context: Some(serde_json::json!({
						"hint": "Sessions are auto-saved after each interaction. Send a message first."
					})),
				},
			)));
		}
	};

	match share::start_bridge(session_file).await {
		Ok(info) => {
			let host = share::web_host();
			let bridge = format!("127.0.0.1:{}", info.port);
			let url = format!(
				"{}/analyze?b={}&t={}",
				host.trim_end_matches('/'),
				bridge,
				info.token
			);
			Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Analyze {
					url,
					port: info.port,
					token: info.token,
				},
			)))
		}
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Failed to start analyze bridge: {}", e),
				context: Some(serde_json::json!({
					"hint": "Most likely a port-binding issue. Try again."
				})),
			},
		))),
	}
}
