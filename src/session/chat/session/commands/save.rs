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

// Save command handler

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub fn handle_save(session: &mut ChatSession) -> Result<CommandResult> {
	match session.save() {
		Ok(_) => Ok(CommandResult::HandledWithOutput(CommandOutput::Save {
			success: true,
			message: Some("Session saved successfully".to_string()),
			session_id: Some(session.session.info.name.clone()),
		})),
		Err(e) => Ok(CommandResult::HandledWithOutput(CommandOutput::Save {
			success: false,
			message: Some(format!("Failed to save session: {}", e)),
			session_id: None,
		})),
	}
}
