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

// Session command handler

use super::super::core::ChatSession;
use anyhow::Result;
use colored::Colorize;

pub fn handle_session(session: &mut ChatSession, params: &[&str]) -> Result<bool> {
	// Handle session switching
	if params.is_empty() {
		// If no session name provided, create a new session with a random name
		// Use the same timestamp-based naming convention as in the main function
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		let new_session_name = format!("session_{}", timestamp);

		println!(
			"{}",
			format!("Creating new session: {}", new_session_name).bright_green()
		);

		// Save current session before switching - no need to save here
		// The main loop will handle saving before switching

		// Set the session name to return
		session.session.info.name = new_session_name;
		Ok(true)
	} else {
		// Get the session name from the parameters
		let new_session_name = params.join(" ");
		let current_session_path = session.session.session_file.clone();

		// Check if we're already in this session
		if let Some(current_path) = &current_session_path {
			if current_path
				.file_stem()
				.and_then(|s| s.to_str())
				.unwrap_or("")
				== new_session_name
			{
				println!("{}", "You are already in this session.".blue());
				return Ok(false);
			}
		}

		// Return a signal to the main loop with the session name to switch to
		// We'll use a specific return code that tells the main loop to switch sessions
		session.session.info.name = new_session_name;
		Ok(true)
	}
}
