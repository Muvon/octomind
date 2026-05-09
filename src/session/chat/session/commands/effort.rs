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

// Reasoning-effort command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::{Config, ReasoningEffortConfig};
use anyhow::Result;

const VALID: &[&str] = &["low", "medium", "high", "xhigh", "max"];

pub fn handle_effort(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> Result<CommandResult> {
	if params.is_empty() {
		let current = session.reasoning_effort.unwrap_or(config.reasoning_effort);
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Effort {
				old_effort: None,
				new_effort: current.as_str().to_string(),
				changed: false,
				saved: None,
				save_error: None,
			},
		)));
	}

	let arg = params[0];
	let parsed = match ReasoningEffortConfig::parse(arg) {
		Some(e) => e,
		None => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Error {
					error: format!("Invalid reasoning effort: '{}'", arg),
					context: Some(serde_json::json!({ "valid": VALID })),
				},
			)));
		}
	};

	let old_effort = session
		.reasoning_effort
		.unwrap_or(config.reasoning_effort)
		.as_str()
		.to_string();

	if let Some(session_file) = &session.session.session_file {
		if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
			let command_line = format!("/effort {}", parsed.as_str());
			let _ = crate::session::logger::log_session_command(session_name, &command_line);
		}
	}

	session.reasoning_effort = Some(parsed);

	let (saved, save_error) = match session.save() {
		Ok(_) => (Some(true), None),
		Err(e) => (Some(false), Some(e.to_string())),
	};

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Effort {
			old_effort: Some(old_effort),
			new_effort: parsed.as_str().to_string(),
			changed: true,
			saved,
			save_error,
		},
	)))
}
