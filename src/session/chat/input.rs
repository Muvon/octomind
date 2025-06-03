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

// User input handling module

use anyhow::Result;
use colored::*;
use rustyline::error::ReadlineError;
use rustyline::{Cmd, Event, EventHandler, KeyEvent, Modifiers};
use rustyline::{CompletionType, Config as RustylineConfig, EditMode, Editor};
use std::path::PathBuf;

use crate::log_info;

// Get the history file path
fn get_history_file_path() -> Result<PathBuf> {
	// Use system-wide data directory
	let data_dir = crate::directories::get_octomind_data_dir()?;
	Ok(data_dir.join("history"))
}

// Read user input with support for multiline input, command completion, and persistent history
pub fn read_user_input(estimated_cost: f64) -> Result<String> {
	// Configure rustyline
	let config = RustylineConfig::builder()
		.completion_type(CompletionType::List)
		.edit_mode(EditMode::Emacs)
		.auto_add_history(true) // Automatically add lines to history
		.bell_style(rustyline::config::BellStyle::None) // No bell
		.build();

	// Create editor with our custom helper
	let mut editor = Editor::with_config(config)?;

	// Add command completion
	use crate::session::chat_helper::CommandHelper;
	editor.set_helper(Some(CommandHelper::new()));

	// Set up custom key bindings for accepting hints
	// Ctrl+E to accept hint (complete-hint command)
	editor.bind_sequence(
		Event::KeySeq(vec![KeyEvent::new('e', Modifiers::CTRL)]),
		EventHandler::Simple(Cmd::CompleteHint),
	);

	// Right arrow to accept hint when at end of line
	// Using escape sequence for right arrow key: \x1b[C
	editor.bind_sequence(
		Event::KeySeq(vec![
			KeyEvent::new('\x1b', Modifiers::empty()),
			KeyEvent::new('[', Modifiers::empty()),
			KeyEvent::new('C', Modifiers::empty()),
		]),
		EventHandler::Simple(Cmd::CompleteHint),
	);

	// Ctrl+J to insert newline for multi-line input
	editor.bind_sequence(
		Event::KeySeq(vec![KeyEvent::new('j', Modifiers::CTRL)]),
		EventHandler::Simple(Cmd::Newline),
	);

	// Load persistent history
	let history_path = get_history_file_path()?;
	if history_path.exists() {
		if let Err(e) = editor.load_history(&history_path) {
			// Don't fail if history can't be loaded, just log it
			log_info!(
				"Could not load history from {}: {}",
				history_path.display(),
				e
			);
		}
	}

	// Set prompt with colors if terminal supports them and include cost estimation
	let prompt = if estimated_cost > 0.0 {
		format!("[~${:.2}] > ", estimated_cost)
			.bright_blue()
			.to_string()
	} else {
		"> ".bright_blue().to_string()
	};

	// Read line with command completion and history search (Ctrl+R)
	match editor.readline(&prompt) {
		Ok(line) => {
			// Add to in-memory history (auto_add_history is true, but we also save to file)
			let _ = editor.add_history_entry(line.clone());

			// Save history to persistent file
			// This includes ALL inputs - both regular inputs and commands starting with '/'
			if let Err(e) = editor.save_history(&history_path) {
				// Don't fail if history can't be saved, just log it
				log_info!(
					"Could not save history to {}: {}",
					history_path.display(),
					e
				);
			}

			// Log user input only if it's not a command (doesn't start with '/')
			if !line.trim().starts_with('/') {
				let _ = crate::session::logger::log_user_request(&line);
			}

			Ok(line)
		}
		Err(ReadlineError::Interrupted) => {
			// Ctrl+C
			println!("\nCancelled");
			Ok(String::new())
		}
		Err(ReadlineError::Eof) => {
			// Ctrl+D - Show session file path before exiting
			println!("\nExiting session...");

			// Show session file path if available
			if let Ok(sessions_dir) = crate::session::get_sessions_dir() {
				println!("Session files saved in: {}", sessions_dir.display());
			}

			log_info!("Session preserved for future reference.");
			Ok("/exit".to_string())
		}
		Err(err) => {
			println!("Error: {:?}", err);
			Ok(String::new())
		}
	}
}
