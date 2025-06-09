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
use rustyline::{
	Cmd, ConditionalEventHandler, Event, EventHandler, KeyEvent, Modifiers, RepeatCount,
};
use rustyline::{CompletionType, Config as RustylineConfig, EditMode, Editor};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;

// Custom event handler for smart Ctrl+E behavior
struct SmartCtrlEHandler;

impl ConditionalEventHandler for SmartCtrlEHandler {
	fn handle(
		&self,
		_evt: &Event,
		_n: RepeatCount,
		_positive: bool,
		ctx: &rustyline::EventContext,
	) -> Option<Cmd> {
		// Check if there's a hint available using the EventContext
		if ctx.has_hint() {
			// There's a hint, so complete it
			Some(Cmd::CompleteHint)
		} else {
			// No hint, use default Emacs behavior (move to end of line)
			// Return None to let the default key binding take effect
			None
		}
	}
}
use std::path::PathBuf;

use crate::log_info;

// Global mutex for history file operations to prevent race conditions
lazy_static::lazy_static! {
	static ref HISTORY_MUTEX: Mutex<()> = Mutex::new(());
}

// Get the history file path
fn get_history_file_path() -> Result<PathBuf> {
	// Use system-wide data directory
	let data_dir = crate::directories::get_octomind_data_dir()?;
	Ok(data_dir.join("history"))
}

// Encode a string for safe storage in history file
// Handles backslashes and newlines properly to avoid conflicts
fn encode_history_line(line: &str) -> String {
	line.chars()
		.map(|c| match c {
			'\\' => "\\\\".to_string(),
			'\n' => "\\n".to_string(),
			c => c.to_string(),
		})
		.collect()
}

// Decode a string from history file back to original form
// Reverses the encoding done by encode_history_line
fn decode_history_line(encoded: &str) -> String {
	let mut result = String::new();
	let mut chars = encoded.chars().peekable();

	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.peek() {
				Some('\\') => {
					chars.next(); // consume the second backslash
					result.push('\\');
				}
				Some('n') => {
					chars.next(); // consume the 'n'
					result.push('\n');
				}
				_ => {
					// Invalid escape sequence, just keep the backslash
					result.push(c);
				}
			}
		} else {
			result.push(c);
		}
	}

	result
}

// Append a single line to history file in thread-safe manner
// Encodes newlines to preserve multiline entries as single history records
fn append_to_history_file(line: &str) -> Result<()> {
	let _lock = HISTORY_MUTEX.lock().unwrap();
	let history_path = get_history_file_path()?;

	// Ensure file exists with version marker
	if !history_path.exists() {
		let mut file = OpenOptions::new()
			.create(true)
			.truncate(true)
			.write(true)
			.open(&history_path)?;
		file.flush()?;
	}

	let mut file = OpenOptions::new()
		.create(true)
		.append(true)
		.open(&history_path)?;

	let encoded_line = encode_history_line(line);
	writeln!(file, "{}", encoded_line)?;
	file.flush()?;

	Ok(())
}

// Load history from file, handling concurrent access safely
// Decodes newlines to restore multiline entries as single history records
fn load_history_from_file() -> Result<Vec<String>> {
	let _lock = HISTORY_MUTEX.lock().unwrap();
	let history_path = get_history_file_path()?;

	if !history_path.exists() {
		return Ok(Vec::new());
	}

	let file = std::fs::File::open(&history_path)?;
	let reader = BufReader::new(file);

	let mut history = Vec::new();
	for line in reader.lines() {
		let line = line?;
		if line.trim().is_empty() || line.starts_with("#") {
			continue; // Skip empty lines and comments
		}

		let decoded_line = decode_history_line(&line);
		history.push(decoded_line);
	}

	Ok(history)
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

	// Set up custom key bindings
	// Ctrl+E: Smart behavior - ONLY accepts hints when available,
	// otherwise falls back to default Emacs behavior (move to end of line)
	editor.bind_sequence(
		Event::KeySeq(vec![KeyEvent::new('e', Modifiers::CTRL)]),
		EventHandler::Conditional(Box::new(SmartCtrlEHandler)),
	);

	// Tab also accepts hints as alternative
	editor.bind_sequence(
		Event::KeySeq(vec![KeyEvent::new('\t', Modifiers::empty())]),
		EventHandler::Simple(Cmd::CompleteHint),
	);

	// Right arrow to accept hint when at end of line
	editor.bind_sequence(
		Event::KeySeq(vec![
			KeyEvent::new('\x1b', Modifiers::empty()),
			KeyEvent::new('[', Modifiers::empty()),
			KeyEvent::new('C', Modifiers::empty()),
		]),
		EventHandler::Simple(Cmd::CompleteHint),
	);

	// Tab to accept hints as well
	editor.bind_sequence(
		Event::KeySeq(vec![KeyEvent::new('\t', Modifiers::empty())]),
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

	// Load persistent history using our safe method
	match load_history_from_file() {
		Ok(history_lines) => {
			for line in history_lines {
				let _ = editor.add_history_entry(line);
			}
		}
		Err(e) => {
			log_info!("Could not load history: {}", e);
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

			// Append to persistent file using thread-safe append-only method
			// This includes ALL inputs - both regular inputs and commands starting with '/'
			if let Err(e) = append_to_history_file(&line) {
				// Don't fail if history can't be saved, just log it
				log_info!("Could not append to history file: {}", e);
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

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Write;

	#[test]
	fn test_multiline_history_encoding_decoding() {
		// Test data with various edge cases
		let test_cases = vec![
			"Simple single line",
			"Line with\nmultiple\nlines",
			"Line with \\backslash",
			"Line with \\n literal",
			"Complex case:\nLine 1\nLine 2 with \\backslash\nLine 3",
			"Edge case: \\\\n (backslash followed by literal n)",
			"", // Empty line
		];

		for original in test_cases {
			// Test encoding and decoding using our new functions
			let encoded = encode_history_line(original);
			let decoded = decode_history_line(&encoded);

			assert_eq!(
				original, decoded,
				"Encoding/decoding failed for: {:?}",
				original
			);
		}
	}

	#[test]
	fn test_complete_history_workflow() -> Result<()> {
		use std::env;
		use std::fs;

		// Create a temporary file for testing complete workflow
		let temp_dir = env::temp_dir();
		let temp_file_path = temp_dir.join("octomind_test_complete_history");

		// Clean up any existing test file
		let _ = fs::remove_file(&temp_file_path);

		// Test multiline input using the actual append function logic
		let multiline_input = "This is line 1\nThis is line 2\nThis is line 3";

		// Simulate append_to_history_file workflow
		{
			// Create file with version marker (simulate first append)
			let mut file = OpenOptions::new()
				.create(true)
				.truncate(true)
				.write(true)
				.open(&temp_file_path)?;
			file.flush()?;

			// Append encoded multiline entry
			let mut file = OpenOptions::new().append(true).open(&temp_file_path)?;
			let encoded = encode_history_line(multiline_input);
			writeln!(file, "{}", encoded)?;
			file.flush()?;
		}

		// Simulate load_history_from_file workflow
		let loaded_history = {
			let file = std::fs::File::open(&temp_file_path)?;
			let reader = BufReader::new(file);

			let mut history = Vec::new();
			for line in reader.lines() {
				let line = line?;
				if line.trim().is_empty() || line.starts_with("#") {
					continue; // Skip empty lines and comments
				}

				let decoded = decode_history_line(&line);
				history.push(decoded);
			}
			history
		};

		// Clean up test file
		let _ = fs::remove_file(&temp_file_path);

		// Verify the multiline input was preserved as a single entry
		assert_eq!(
			loaded_history.len(),
			1,
			"Should have exactly one history entry"
		);
		assert_eq!(
			loaded_history[0], multiline_input,
			"Multiline content should be preserved exactly"
		);

		Ok(())
	}
}
