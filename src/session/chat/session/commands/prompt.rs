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

// Prompt command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use crate::config::Config;
use anyhow::Result;

pub async fn handle_prompt(
	session: &mut ChatSession,
	config: &Config,
	_role: &str,
	params: &[&str],
) -> Result<CommandResult> {
	// Handle /prompt command for sending predefined prompt templates
	if params.is_empty() {
		// Show available prompts
		let prompts_data: Vec<serde_json::Value> = config
			.prompts
			.iter()
			.map(|p| {
				serde_json::json!({
					"name": p.name,
					"description": p.description
				})
			})
			.collect();

		return Ok(CommandResult::HandledWithOutput(CommandOutput::Prompt {
			data: serde_json::json!({
				"action": "list",
				"prompts": prompts_data
			}),
		}));
	}

	let prompt_name = params[0];

	// Find the prompt configuration
	let prompt_config = if let Some(p) = config.prompts.iter().find(|p| p.name == prompt_name) {
		p
	} else {
		let available_prompts: Vec<&str> = config.prompts.iter().map(|p| p.name.as_str()).collect();
		return Ok(CommandResult::HandledWithOutput(CommandOutput::Prompt {
			data: serde_json::json!({
				"action": "execute",
				"success": false,
				"error": format!("Prompt template not found: {}", prompt_name),
				"available_prompts": available_prompts
			}),
		}));
	};

	// Process the prompt template (support variable substitution if needed)
	let processed_prompt = process_prompt_template(&prompt_config.prompt, config, _role)?;

	// CRITICAL FIX: Don't add the message here, just store it as pending
	// The main loop will pick it up and process it as normal user input
	session.pending_prompt = Some(processed_prompt.clone());

	Ok(CommandResult::HandledWithOutput(CommandOutput::Prompt {
		data: serde_json::json!({
			"action": "execute",
			"success": true,
			"prompt_name": prompt_name,
			"prompt_content": processed_prompt
		}),
	}))
}

/// Process prompt template with variable substitution
fn process_prompt_template(template: &str, _config: &Config, _role: &str) -> Result<String> {
	// For now, return template as-is
	// Future enhancement: Add variable substitution similar to system prompts
	// Could support variables like {role}, {model}, {timestamp}, etc.
	Ok(template.to_string())
}
