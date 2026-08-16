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

// Rename command handler — sets a display title for the current session.

use super::super::core::ChatSession;
use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub fn handle_rename(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	let session_name = session.session.info.name.clone();

	// `/rename` with no argument clears the title.
	let new_title: Option<String> = if params.is_empty() {
		None
	} else {
		Some(params.join(" "))
	};

	match crate::session::titles::set_session_title(
		&session_name,
		new_title.as_deref(),
		Some(&session.role),
		Some(&session.session.info.model),
	) {
		Ok(applied) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Rename {
				session_name,
				title: applied,
			},
		))),
		Err(e) => Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Error {
				error: format!("Failed to rename session: {}", e),
				context: None,
			},
		))),
	}
}
