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

// Model command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub fn handle_model(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	// Handle model command
	if params.is_empty() {
		// Build JSON output
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Model {
				old_model: None,
				new_model: session.model.clone(),
				changed: false,
				saved: None,
				save_error: None,
			},
		)));
	}

	// Change to a new model (runtime only)
	let new_model = params.join(" ");

	// Reject anything config validation would reject before touching the session —
	// the same `get_provider_for_model` check `Config::validate` runs on the main
	// model (empty provider or model, unknown provider, unsupported model). A value
	// like "/model gpt5" would otherwise be saved and fail on the next request,
	// with no path back short of editing the session file.
	if let Err(e) = crate::providers::ProviderFactory::get_provider_for_model(&new_model) {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Invalid model '{}': {}", new_model, e),
				context: Some(serde_json::json!({
					"expected": "provider:model",
					"example": "openrouter:anthropic/claude-3.5-sonnet"
				})),
			},
		)));
	}

	let old_model = session.model.clone();

	// Log the command execution
	if let Some(session_file) = &session.session.session_file {
		if let Some(session_name) = session_file.file_stem().and_then(|s| s.to_str()) {
			let command_line = format!("/model {}", new_model);
			let _ = crate::session::logger::log_session_command(session_name, &command_line);
		}
	}

	// Update session model (runtime only - don't update config)
	session.model = new_model.clone();
	session.session.info.model = new_model.clone();

	// Build JSON output
	let saved = match session.save() {
		Ok(_) => Some(true),
		Err(e) => {
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Model {
					old_model: Some(old_model),
					new_model,
					changed: true,
					saved: Some(false),
					save_error: Some(e.to_string()),
				},
			)));
		}
	};

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Model {
			old_model: Some(old_model),
			new_model,
			changed: true,
			saved,
			save_error: None,
		},
	)))
}
