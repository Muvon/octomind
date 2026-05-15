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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Last cost we displayed in the status line above the prompt. Used to
/// compute the `(+$delta)` segment on subsequent prompts. Stored as f64 bits.
static LAST_DISPLAYED_COST: AtomicU64 = AtomicU64::new(0);
/// Last context-tokens / max-threshold we showed. If nothing changed since the
/// previous prompt (e.g. user hit Enter on an empty input), we suppress the
/// status line — otherwise empty submits spam the screen with identical rows.
static LAST_DISPLAYED_CTX: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_DISPLAYED_MAX: AtomicU64 = AtomicU64::new(u64::MAX);

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
		"│ Ctrl+G      - Add message without sending (or retry on  │".bright_black()
	);
	println!(
		"{}",
		"│               empty input after a failed request)       │".bright_black()
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

/// Strip ANSI CSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	let mut in_esc = false;
	for c in s.chars() {
		if in_esc {
			if c.is_ascii_alphabetic() {
				in_esc = false;
			}
			continue;
		}
		if c == '\x1b' {
			in_esc = true;
			continue;
		}
		out.push(c);
	}
	out
}

/// Approximate display width: char count of ANSI-stripped string.
/// Underestimates for wide chars (CJK, emoji), which is the safe direction —
/// we only ever clear rows we know reedline rendered.
fn display_cols(s: &str) -> usize {
	strip_ansi(s).chars().count()
}

/// Number of terminal rows reedline used to render the prompt+input.
/// Returns 0 if it couldn't be determined (callers should skip redraw).
fn rendered_rows(
	prompt_left: &str,
	indicator: &str,
	multiline: &str,
	line: &str,
	term_w: usize,
) -> usize {
	if term_w == 0 {
		return 0;
	}
	let first_prefix = display_cols(prompt_left) + display_cols(indicator);
	let cont_prefix = display_cols(multiline);
	let mut rows = 0usize;
	for (i, l) in line.split('\n').enumerate() {
		let prefix = if i == 0 { first_prefix } else { cont_prefix };
		let combined = (prefix + display_cols(l)).max(1);
		rows += combined.div_ceil(term_w);
	}
	rows
}

/// Re-render a just-submitted user input as a styled history marker,
/// replacing reedline's plain rendering. Call immediately after
/// `Signal::Success` returns, while the cursor is on the line below the
/// rendered prompt+input.
///
/// Style: bright-blue left bar (▌) + italic message text. No background,
/// so it's resize-stable in scrollback and reads clearly on any terminal
/// theme.
///
/// No-op for empty input. Best-effort: silently bails if terminal size or
/// I/O fails.
fn highlight_submitted_input(prompt_left: &str, indicator: &str, multiline: &str, line: &str) {
	use crossterm::{cursor, queue, terminal};

	if line.trim().is_empty() {
		return;
	}
	let Ok((term_w_u16, _)) = crossterm::terminal::size() else {
		return;
	};
	let term_w = term_w_u16 as usize;
	if term_w < 4 {
		return;
	}

	let rows = rendered_rows(prompt_left, indicator, multiline, line, term_w);
	if rows == 0 {
		return;
	}

	let mut out = std::io::stdout();
	if queue!(
		out,
		cursor::MoveUp(rows as u16),
		cursor::MoveToColumn(0),
		terminal::Clear(terminal::ClearType::FromCursorDown),
	)
	.is_err()
	{
		return;
	}
	let _ = out.flush();

	// Bright-blue left bar (▍) + italic message text. One marker at the start
	// of the line is enough to make user messages stand out in history.
	// No background, so it's resize-stable and works on any terminal theme.
	let marker = "\x1b[94m▍\x1b[39m"; // bright blue ▍, then reset fg only
	let italic_on = "\x1b[3m";
	let reset = "\x1b[0m";

	// Continuation prefix aligns under the message text (▍=1 cell + space).
	let cont_pad = "  ";

	// Manually wrap each logical line to the available width so EVERY visual
	// row carries either the `▍` marker (first row) or the alignment pad
	// (continuation rows). If we let the terminal wrap, wrap-rows fall under
	// no prefix and the message visually merges with surrounding output.
	// Prefix width = 2 cells (`▍ ` or `  `); wrap budget is term_w - 2.
	let wrap_w = term_w.saturating_sub(2).max(1);

	let mut first_visual_row = true;
	for raw_line in line.split('\n') {
		// Split by char count, matching `display_cols`'s width model (safe
		// underestimate for wide chars — we only ever wrap earlier, never
		// past the right edge).
		let chars: Vec<char> = raw_line.chars().collect();
		if chars.is_empty() {
			// Preserve blank lines from explicit `\n\n` in user input.
			let prefix = if first_visual_row {
				format!("{} {}", marker, italic_on)
			} else {
				format!("{}{}", cont_pad, italic_on)
			};
			println!("{}{}", prefix, reset);
			first_visual_row = false;
			continue;
		}
		for chunk in chars.chunks(wrap_w) {
			let prefix = if first_visual_row {
				format!("{} {}", marker, italic_on)
			} else {
				format!("{}{}", cont_pad, italic_on)
			};
			let text: String = chunk.iter().collect();
			println!("{}{}{}", prefix, text, reset);
			first_visual_row = false;
		}
	}
	let _ = std::io::stdout().flush();
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
	// External break signal — flipped by the inbox-watcher thread below when a
	// scheduled / background message arrives while the user is idle, so the
	// session auto-processes it without requiring an Enter keypress.
	let break_signal = Arc::new(AtomicBool::new(false));
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
				buffer_empty.clone(),
				hint_available,
				line_state.clone(),
			),
		))
		.with_quick_completions(true)
		.use_bracketed_paste(true)
		.with_break_signal(break_signal.clone())
		.with_edit_mode(edit_mode);

	// Set up external printer for inbox notifications.
	// A background thread polls the inbox_pending flag and sends a
	// one-shot notification that reedline renders above the prompt.
	// When the user is idle (empty buffer, no reverse-search), it also flips
	// `break_signal` so reedline returns immediately and the main loop drains
	// the inbox without waiting for an Enter keypress.
	// `printer` was created earlier and shared with edit_mode via clone.
	let sender = printer.sender();
	let inbox_slot = inbox_pending;
	let break_signal_for_thread = break_signal.clone();
	let buffer_empty_for_thread = buffer_empty.clone();
	let reverse_search_for_thread = reverse_search_active.clone();
	std::thread::spawn(move || {
		let mut notified = false;
		loop {
			std::thread::sleep(std::time::Duration::from_millis(100));
			let preview = inbox_slot.lock().ok().and_then(|g| g.clone());
			if let Some(preview) = preview {
				if !notified {
					let msg = format!(
						"\x1b[33m📨 Inbox message received ({preview}) — processing...\x1b[0m"
					);
					if sender.send(msg).is_err() {
						break; // receiver dropped, reedline is gone
					}
					notified = true;
				}
				// Auto-fire only when the user isn't actively typing or searching.
				// Otherwise we'd interrupt them mid-input. Once they finish (Enter
				// or clear the buffer), the empty-input fallback in the main loop
				// will pick up the inbox message.
				let is_empty = buffer_empty_for_thread.load(std::sync::atomic::Ordering::Relaxed);
				let in_search =
					reverse_search_for_thread.load(std::sync::atomic::Ordering::Relaxed);
				if is_empty && !in_search {
					break_signal_for_thread.store(true, std::sync::atomic::Ordering::Relaxed);
				}
			} else {
				notified = false;
			}
		}
	});
	let mut line_editor = line_editor
		.with_external_printer(printer)
		.with_poll_interval(std::time::Duration::from_millis(100));

	// Print the persistent status line ABOVE the prompt:
	//   ▍ $0.48 (+$0.013) ▰▰▰▱▱ 54.2%
	//   ▍ 〉
	// The `▍` on both lines acts as a session-identity rail. Reedline only
	// owns the bottom row (`▍ 〉` + input); the status line is a plain
	// println we control and scrolls naturally into history.
	// Suppress when nothing has changed since the previous prompt — otherwise
	// an empty `<Enter>` submit (no API call, no cost change) spams an
	// identical status row each time.
	let last_cost_bits = LAST_DISPLAYED_COST.load(Ordering::Relaxed);
	let last_cost = f64::from_bits(last_cost_bits);
	let last_ctx = LAST_DISPLAYED_CTX.load(Ordering::Relaxed);
	let last_max = LAST_DISPLAYED_MAX.load(Ordering::Relaxed);
	let max_u64 = max_session_tokens_threshold as u64;
	let unchanged = last_ctx != u64::MAX
		&& estimated_cost.to_bits() == last_cost_bits
		&& current_context_tokens == last_ctx
		&& max_u64 == last_max;
	if !unchanged {
		let delta = if last_cost > 0.0 && estimated_cost > last_cost {
			Some(estimated_cost - last_cost)
		} else {
			None
		};
		let status_line = crate::session::chat::status_prefix::build_status_line(
			estimated_cost,
			current_context_tokens,
			max_u64,
			delta,
		);
		if !status_line.is_empty() {
			std::println!("{}", status_line);
		}
		LAST_DISPLAYED_COST.store(estimated_cost.to_bits(), Ordering::Relaxed);
		LAST_DISPLAYED_CTX.store(current_context_tokens, Ordering::Relaxed);
		LAST_DISPLAYED_MAX.store(max_u64, Ordering::Relaxed);
	}

	let prompt_left = String::new();
	// Prompt indicator is `▍ 〉` — the `▍` carries the session-identity rail
	// down from the status line. Trailing space inside the indicator means
	// reedline draws `▍ 〉` then immediately user input, with the wide `〉`
	// naturally providing visual separation from the typed text.
	let indicator = format!("{} {}", "▍".bright_blue(), "〉".bright_blue());
	let prompt = crate::session::chat::ChatPrompt::new(
		prompt_left.clone(),
		indicator.clone(),
		reverse_search_active,
	);

	// Clone line_state for use in the loop (original moved into edit_mode)
	let line_state_for_check = line_state.clone();

	// Flush any keypresses that piled up during animation. ECHO was already
	// off (CtrlCEchoGuard) so the user didn't see them, but the bytes still
	// sit in stdin's input queue — without this, reedline would consume
	// them as the next prompt's input.
	crate::utils::term_echo::drain_stdin();

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

				// Replace reedline's plain echo of the submitted input with the
				// `▍ italic` history marker. Pass the actual indicator we used
				// (`▍ 〉`, 4 visual cells) and matching 4-cell multiline pad
				// so row counting clears exactly what reedline rendered.
				highlight_submitted_input(&prompt_left, "▍ 〉", "    ", &line);

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
			Ok(Signal::ExternalBreak(buffer)) => {
				// Inbox-watcher thread flipped `break_signal` because a scheduled /
				// background message is waiting and the user buffer was empty.
				// Return empty Text so the main loop drains the inbox in its
				// empty-input fallback path. If by race the buffer is non-empty,
				// preserve what the user typed instead of dropping it.
				if buffer.trim().is_empty() {
					return Ok(InputResult::Text(String::new(), Vec::new()));
				}
				return Ok(InputResult::Text(buffer, Vec::new()));
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
