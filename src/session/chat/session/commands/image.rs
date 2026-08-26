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

// Image command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub async fn handle_image(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	// Handle /image command for attaching images
	if let Err(error) = session.ensure_model_supports_vision() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Image {
				image_attached: false,
				path: None,
				error: Some(error.to_string()),
			},
		)));
	}

	if params.is_empty() {
		// Check clipboard for images
		if let Ok(true) = session.try_attach_from_clipboard().await {
			// Image was found and attached from clipboard
			return Ok(CommandResult::HandledWithOutput(Box::new(
				CommandOutput::Image {
					image_attached: true,
					path: Some("clipboard".to_string()),
					error: None,
				},
			)));
		}

		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Image {
				image_attached: false,
				path: None,
				error: None,
			},
		)));
	}

	let image_path = params.join(" ");
	match session.attach_image_from_path(&image_path).await {
		Ok(_) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Image {
				image_attached: true,
				path: Some(image_path),
				error: None,
			},
		))),
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Image {
				image_attached: false,
				path: Some(image_path),
				error: Some(format!("Failed to attach image: {}", e)),
			},
		))),
	}
}
