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

// New session command handler — starts a fresh session with unified naming.

use super::super::core::{generate_session_name, ChatSession};
use super::CommandResult;
use anyhow::Result;

pub fn handle_new(session: &mut ChatSession, params: &[&str]) -> Result<CommandResult> {
	// Generate a session name using the same format as `octomind run`:
	// YYMMDD-basename-HHMM-uuid4short
	let new_session_name = generate_session_name();

	// Set the session name so the main loop's Exit handler picks it up
	// when initializing the new session via SessionInitParams::with_name().
	session.session.info.name = new_session_name.clone();

	// If a title argument was provided, set it as the display title for the
	// new session — same mechanism as /rename.
	if !params.is_empty() {
		let title = params.join(" ");
		if let Err(e) = crate::session::titles::set_session_title(
			&new_session_name,
			Some(&title),
			Some(&session.role),
			Some(&session.session.info.model),
		) {
			crate::log_debug!("Failed to set title for new session: {}", e);
		}
	}

	// Exit signals the main loop to save the current session and initialize
	// a fresh one using the name we set above.
	Ok(CommandResult::Exit)
}
