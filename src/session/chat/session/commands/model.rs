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
		return Ok(CommandResult::HandledWithOutput(Box::new(CommandOutput::Model {
			old_model: None,
			new_model: session.model.clone(),
			changed: false,
			saved: None,
			save_error: None,
		})));
	}

	let new_model = params.join(" ");

	// Reject anything config validation would reject before touching the session —
	// the same `get_provider_for_model` check `Config::validate` runs on the main
	// model (empty provider or model, unknown provider, unsupported model). A value
	// like "/model gpt5" would otherwise be saved and fail on the next request,
	// with no path back short of editing the session file.
	if let Err(e) = crate::providers::ProviderFactory::get_provider_for_model(&new_model) {
		return Ok(CommandResult::HandledWithOutput(Box::new(CommandOutput::Error {
			error: format!("Invalid model '{}': {}", new_model, e),
			context: Some(serde_json::json!({
				"expected": "provider:model",
				"example": "openrouter:anthropic/claude-3.5-sonnet"
			})),
		})));
	}

	let old_model = session.model.clone();

	// The selection belongs to this session, not the shared configuration.
	session.model = new_model.clone();
	session.session.info.model = new_model.clone();

	let save_result = if session.session.session_file.is_some() {
		crate::session::logger::log_session_command(&session.session.info.name, &format!("/model {}", new_model))
			.and_then(|()| session.save())
	} else {
		session.save()
	};
	let saved = match save_result {
		Ok(_) => Some(true),
		Err(e) => {
			return Ok(CommandResult::HandledWithOutput(Box::new(CommandOutput::Model {
				old_model: Some(old_model),
				new_model,
				changed: true,
				saved: Some(false),
				save_error: Some(e.to_string()),
			})));
		}
	};

	Ok(CommandResult::HandledWithOutput(Box::new(CommandOutput::Model {
		old_model: Some(old_model),
		new_model,
		changed: true,
		saved,
		save_error: None,
	})))
}
