// User input handling module

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::{Editor, Config as RustylineConfig, CompletionType, EditMode};
use colored::*;

// Read user input with support for multiline input and command completion
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

	// Set prompt with colors if terminal supports them and include cost estimation
	let prompt = if estimated_cost > 0.0 {
		format!("[~${:.2}] > ", estimated_cost).bright_blue().to_string()
	} else {
		"> ".bright_blue().to_string()
	};

	// Read line with command completion
	match editor.readline(&prompt) {
		Ok(line) => {
			// Add to history
			let _ = editor.add_history_entry(line.clone());
			Ok(line)
		},
		Err(ReadlineError::Interrupted) => {
			// Ctrl+C
			println!("\nCancelled");
			Ok(String::new())
		},
		Err(ReadlineError::Eof) => {
			// Ctrl+D
			println!("\nExiting session.");
			Ok("/exit".to_string())
		},
		Err(err) => {
			println!("Error: {:?}", err);
			Ok(String::new())
		}
	}
}