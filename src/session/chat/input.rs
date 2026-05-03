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

// User input handling module

use crate::config::Config;
use crate::mcp::get_available_functions;
use crate::session::estimate_full_context_tokens;
use crate::session::history::{append_to_session_history_file, load_session_history_from_file};
use anyhow::Result;
use colored::*;
use reedline::{
	default_emacs_keybindings, ColumnarMenu, EditCommand, Emacs, FileBackedHistory, History,
	HistoryItem, KeyCode, KeyModifiers, Keybindings, MenuBuilder, Reedline, ReedlineEvent,
	ReedlineMenu, Signal,
};
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Result of user input operation
#[derive(Debug)]
pub enum InputResult {
	/// Normal text input from user, plus any blobs auto-attached via Ctrl+V.
	Text(
		String,
		Vec<crate::session::chat::reedline_adapter::PendingClipboardItem>,
	),
	/// Input was cancelled (Ctrl+C)
	Cancelled,
	/// User wants to exit (Ctrl+D)
	Exit,
	/// Add message to context without sending (Ctrl+G), plus any auto-attached blobs.
	AddWithoutSending(
		String,
		Vec<crate::session::chat::reedline_adapter::PendingClipboardItem>,
	),
}

use crate::log_info;

fn display_shortcuts_help() {
	println!();
	println!(
		"{}",
		"╭─ Keyboard Shortcuts ─────────────────────────────────────╮".bright_cyan()
	);
	println!(
		"{}",
		"│ /           - Commands (type /help for list)            │".bright_black()
	);
	println!(
		"{}",
		"│ @           - Fuzzy file completion (e.g., @src/ma)     │".bright_black()
	);
	println!(
		"{}",
		"│ Tab         - Complete command/file                     │".bright_black()
	);
	println!(
		"{}",
		"│ Shift+Tab   - Search history                            │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+J      - Insert newline (multi-line input)         │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+G      - Add message without sending to API        │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+V      - Auto-attach clipboard image / video       │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+E      - Accept hint / Exit reverse search         │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+R      - Search command history                    │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+C      - Cancel current operation                  │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+D      - Exit session                              │".bright_black()
	);
	println!(
		"{}",
		"│ Ctrl+P/N    - Navigate command history                  │".bright_black()
	);
	println!(
		"{}",
		"│ →           - Accept hint (when at end of line)         │".bright_black()
	);
	println!(
		"{}",
		"╰──────────────────────────────────────────────────────────╯".bright_cyan()
	);
	println!();

	let _ = std::io::stdout().flush();
}

fn calculate_context_percentage(
	current_context_tokens: u64,
	max_session_tokens_threshold: usize,
) -> Option<f64> {
	if max_session_tokens_threshold > 0 {
		Some(
			(current_context_tokens as f64 / max_session_tokens_threshold as f64 * 100.0)
				.min(100.0),
		)
	} else {
		None
	}
}

fn add_completion_menu_keybindings(keybindings: &mut Keybindings) {
	keybindings.add_binding(
		KeyModifiers::NONE,
		KeyCode::Tab,
		ReedlineEvent::UntilFound(vec![
			ReedlineEvent::Menu("completion_menu".to_string()),
			ReedlineEvent::MenuNext,
		]),
	);
}

/// Calculate current context tokens for the session
/// This uses actual message count + system prompt + tools, NOT lifetime accumulated tokens
pub async fn calculate_current_context_tokens(
	messages: &[crate::session::Message],
	config: &Config,
	_role: &str,
) -> u64 {
	// Get available tools
	let tools = get_available_functions(config).await;

	// Calculate actual context tokens
	estimate_full_context_tokens(messages, Some(&tools)) as u64
}
pub fn read_user_input(
	estimated_cost: f64,
	octomind_config: &Config,
	role: &str,
	current_context_tokens: u64,
	max_session_tokens_threshold: usize,
	session_id: &str,
	inbox_pending: Arc<std::sync::Mutex<Option<String>>>,
) -> Result<InputResult> {
	// Create reedline with in-memory history and preloaded role history
	let mut history = FileBackedHistory::new(1000).expect("Error configuring history");
	if let Ok(lines) = load_session_history_from_file(role) {
		for line in lines {
			let _ = history.save(HistoryItem {
				id: None,
				start_timestamp: None,
				command_line: line,
				session_id: None,
				hostname: None,
				cwd: None,
				duration: None,
				exit_status: None,
				more_info: None,
			});
		}
	}
	let history = Box::new(history);

	let mut keybindings = default_emacs_keybindings();
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('j'),
		ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
	);
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('u'),
		ReedlineEvent::Edit(vec![
			EditCommand::CutFromLineStart,
			EditCommand::CutToLineEnd,
		]),
	);
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('a'),
		ReedlineEvent::Edit(vec![EditCommand::MoveToLineStart { select: false }]),
	);
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('e'),
		ReedlineEvent::Edit(vec![EditCommand::MoveToLineEnd { select: false }]),
	);
	// Note: Ctrl+G is handled in edit_mode.rs via line_state flag (no buffer modification)
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('e'),
		ReedlineEvent::HistoryHintComplete,
	);
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('p'),
		ReedlineEvent::UntilFound(vec![
			ReedlineEvent::MenuPrevious,
			ReedlineEvent::PreviousHistory,
		]),
	);
	keybindings.add_binding(
		KeyModifiers::CONTROL,
		KeyCode::Char('n'),
		ReedlineEvent::UntilFound(vec![ReedlineEvent::MenuNext, ReedlineEvent::NextHistory]),
	);
	keybindings.add_binding(
		KeyModifiers::NONE,
		KeyCode::Char('@'),
		ReedlineEvent::Multiple(vec![
			ReedlineEvent::Edit(vec![EditCommand::InsertChar('@')]),
			ReedlineEvent::Menu("completion_menu".to_string()),
		]),
	);
	add_completion_menu_keybindings(&mut keybindings);
	let config = Arc::new(octomind_config.clone());
	let role_name = role.to_string();
	let buffer_empty = Arc::new(AtomicBool::new(true));
	let reverse_search_active = Arc::new(AtomicBool::new(false));
	let hint_available = Arc::new(AtomicBool::new(false));
	let line_state = Arc::new(std::sync::Mutex::new(
		crate::session::chat::reedline_adapter::LineState::default(),
	));
	// One ExternalPrinter shared between reedline (display) and edit_mode (Ctrl+V notifications).
	// `ExternalPrinter` is `Clone` and shares its internal channel, so cloning before moving
	// into reedline lets the Ctrl+V handler send notifications via the same render path.
	let printer = reedline::ExternalPrinter::<String>::new(5);
	let printer_for_edit = printer.clone();

	let edit_mode = Box::new(crate::session::chat::EmacsWithShortcutHelp::new(
		Emacs::new(keybindings),
		buffer_empty.clone(),
		reverse_search_active.clone(),
		hint_available.clone(),
		line_state.clone(),
		printer_for_edit,
	));

	let completion_menu = Box::new(
		ColumnarMenu::default()
			.with_name("completion_menu")
			.with_columns(4)
			.with_column_padding(2),
	);

	let line_editor = Reedline::create()
		.with_history(history)
		.with_completer(Box::new(
			crate::session::chat::reedline_adapter::ReedlineAdapter::new(
				config.clone(),
				role_name.clone(),
				buffer_empty.clone(),
				hint_available.clone(),
				line_state.clone(),
			),
		))
		.with_menu(ReedlineMenu::EngineCompleter(completion_menu))
		.with_highlighter(Box::new(
			crate::session::chat::reedline_adapter::ReedlineAdapter::new(
				config.clone(),
				role_name.clone(),
				buffer_empty.clone(),
				hint_available.clone(),
				line_state.clone(),
			),
		))
		.with_hinter(Box::new(
			crate::session::chat::reedline_adapter::ReedlineAdapter::new(
				config,
				role_name.clone(),
				buffer_empty,
				hint_available,
				line_state.clone(),
			),
		))
		.with_quick_completions(true)
		.use_bracketed_paste(true)
		.with_edit_mode(edit_mode);

	// Set up external printer for inbox notifications.
	// A background thread polls the inbox_pending flag and sends a
	// one-shot notification that reedline renders above the prompt.
	// `printer` was created earlier and shared with edit_mode via clone.
	let sender = printer.sender();
	let inbox_slot = inbox_pending;
	std::thread::spawn(move || {
		let mut notified = false;
		loop {
			std::thread::sleep(std::time::Duration::from_millis(100));
			let preview = inbox_slot.lock().ok().and_then(|g| g.clone());
			if let Some(preview) = preview {
				if !notified {
					let msg = format!(
						"\x1b[33m📨 Inbox message received ({preview}) — press Enter to process\x1b[0m"
					);
					if sender.send(msg).is_err() {
						break; // receiver dropped, reedline is gone
					}
					notified = true;
				}
			} else {
				notified = false;
			}
		}
	});
	let mut line_editor = line_editor
		.with_external_printer(printer)
		.with_poll_interval(std::time::Duration::from_millis(100));

	// Set prompt with cost and context percentage
	let prompt_text = if estimated_cost > 0.0 {
		let context_pct =
			calculate_context_percentage(current_context_tokens, max_session_tokens_threshold);

		if let Some(pct) = context_pct {
			format!("[${:.2}|{:.1}%]", estimated_cost, pct)
		} else {
			format!("[${:.2}|∞]", estimated_cost)
		}
	} else if max_session_tokens_threshold > 0 {
		// No cost but still show context percentage
		let context_pct =
			calculate_context_percentage(current_context_tokens, max_session_tokens_threshold);
		if let Some(pct) = context_pct {
			format!("[{:.1}%]", pct)
		} else {
			String::new()
		}
	} else {
		String::new()
	};
	let prompt_left = if prompt_text.is_empty() {
		String::new()
	} else {
		format!("{} ", prompt_text).bright_blue().to_string()
	};
	let prompt = crate::session::chat::ChatPrompt::new(
		prompt_left,
		"〉".bright_blue().to_string(),
		reverse_search_active,
	);

	// Clone line_state for use in the loop (original moved into edit_mode)
	let line_state_for_check = line_state.clone();

	// Read line with reedline
	loop {
		match line_editor.read_line(&prompt) {
			Ok(Signal::Success(line)) => {
				if line == "__show_shortcuts__" {
					display_shortcuts_help();
					continue;
				}
				if line.trim() == "?" {
					display_shortcuts_help();
					continue;
				}

				// Check if this is an "add without sending" request (Ctrl+G)
				// and drain any clipboard blobs auto-attached via Ctrl+V.
				// The flag and queue are set in edit_mode.rs.
				let (add_without_sending, clipboard_items) =
					if let Ok(mut state) = line_state_for_check.lock() {
						let flag = state.add_without_sending;
						state.add_without_sending = false;
						let items = std::mem::take(&mut state.pending_clipboard);
						(flag, items)
					} else {
						(false, Vec::new())
					};

				// Check if line starts with whitespace (bash-like behavior)
				// If it does, skip adding to history (both in-memory and persistent)
				let starts_with_whitespace = line.starts_with(char::is_whitespace);

				if !starts_with_whitespace {
					// Add to in-memory history (reedline handles this automatically)
					// Append to persistent file using role-based thread-safe method
					// This includes ALL inputs - both regular inputs and commands starting with '/'
					if let Err(e) = append_to_session_history_file(&role_name, &line) {
						// Don't fail if history can't be saved, just log it
						log_info!(
							"Could not append to history file for role '{}': {}",
							role,
							e
						);
					}
				}

				// User message persistence handled by ChatSession::add_user_message.

				// Auto-wrap multiline pastes (3+ lines) in <log>...</log> so the
				// AI receives them as structured context rather than raw text, and
				// so skill auto-activation ignores the pasted content.
				let line = if line.lines().count() >= 3 {
					format!("<log>\n{}\n</log>", line)
				} else {
					line
				};

				return if add_without_sending {
					Ok(InputResult::AddWithoutSending(line, clipboard_items))
				} else {
					Ok(InputResult::Text(line, clipboard_items))
				};
			}
			Ok(Signal::CtrlC) => {
				// Ctrl+C - Return cancellation result
				return Ok(InputResult::Cancelled);
			}
			Ok(Signal::CtrlD) => {
				// Ctrl+D - Show resume command
				let resume_cmd = format!("octomind run --resume {}", session_id).bright_cyan();
				println!("\nTo continue this session, run: {}", resume_cmd);

				// Debug logging for session preservation
				if let Ok(sessions_dir) = crate::session::get_sessions_dir() {
					crate::log_debug!("Session files saved in: {}", sessions_dir.display());
				}
				crate::log_debug!("Session preserved for future reference.");
				return Ok(InputResult::Exit);
			}
			Ok(_) => {
				// reedline Signal is non-exhaustive — handle future variants gracefully
				continue;
			}
			Err(err) => {
				let msg = format!("{err:?}");
				// Terminal is broken/detached (e.g. another shell took over, resize event
				// while no TTY, or crossterm cursor-position timeout). Looping would spam
				// the same error endlessly — exit cleanly instead.
				if msg.contains("cursor position could not be read")
					|| msg.contains("not a terminal")
					|| msg.contains("inappropriate ioctl")
				{
					return Ok(InputResult::Exit);
				}
				log_info!("Reedline error: {}", msg);
				return Ok(InputResult::Text(String::new(), Vec::new()));
			}
		}
	}
}
