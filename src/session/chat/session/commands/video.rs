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

// Video command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub async fn handle_video(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	// Handle /video command for attaching videos
	if let Err(error) = session.ensure_model_supports_video() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Video {
				video_attached: false,
				path: None,
				error: Some(error.to_string()),
			},
		)));
	}

	if params.is_empty() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Video {
				video_attached: false,
				path: None,
				error: None,
			},
		)));
	}

	let video_path = params.join(" ");
	match session.attach_video_from_path(&video_path).await {
		Ok(_) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Video {
				video_attached: true,
				path: Some(video_path),
				error: None,
			},
		))),
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Video {
				video_attached: false,
				path: Some(video_path),
				error: Some(format!("Failed to attach video: {}", e)),
			},
		))),
	}
}
