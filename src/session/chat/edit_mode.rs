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

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use reedline::{EditMode, Emacs, ExternalPrinter, PromptEditMode, ReedlineEvent, ReedlineRawEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use crate::session::chat::reedline_adapter::{LineState, PendingClipboardItem};
use crate::session::image::ImageProcessor;
use crate::session::video::VideoProcessor;

pub struct EmacsWithShortcutHelp {
	emacs: Emacs,
	buffer_empty: Arc<AtomicBool>,
	reverse_search_active: Arc<AtomicBool>,
	hint_available: Arc<AtomicBool>,
	line_state: Arc<Mutex<LineState>>,
	/// Clone of the input loop's `ExternalPrinter`, used to print the
	/// "📎 attached" notification line above the prompt without disturbing
	/// the typing buffer.
	notifier: ExternalPrinter<String>,
	meta_pending: bool,
}

impl EmacsWithShortcutHelp {
	pub fn new(
		emacs: Emacs,
		buffer_empty: Arc<AtomicBool>,
		reverse_search_active: Arc<AtomicBool>,
		hint_available: Arc<AtomicBool>,
		line_state: Arc<Mutex<LineState>>,
		notifier: ExternalPrinter<String>,
	) -> Self {
		Self {
			emacs,
			buffer_empty,
			reverse_search_active,
			hint_available,
			line_state,
			notifier,
			meta_pending: false,
		}
	}

	/// Push the captured blob onto the pending queue and surface a notification
	/// (label + inline graphics for images) above the prompt via `ExternalPrinter`.
	/// Used by both the Ctrl+V keystroke arm and the bracketed-paste arm.
	fn attach_and_notify(&self, item: PendingClipboardItem) {
		let label = match &item {
			PendingClipboardItem::Image(att) => format_image_label(att),
			PendingClipboardItem::Video(att) => format_video_label(att),
		};

		// Image preview: hand-built Kitty graphics (q=2 silent) or iTerm2 OSC 1337.
		// Both are full-quality and emit no terminal response, so reedline's input
		// stream stays clean. Returns None on terminals without graphics support
		// → falls back to text-only label. The `u32` is the estimated cell-row
		// count: graphics escapes contain no `\n`s, but reedline's ExternalPrinter
		// counts newlines to know how far to scroll the prompt — without padding,
		// the redraw lands on top of the rendered image when the input buffer
		// already has typed content.
		let preview: Option<(String, u32)> = match &item {
			PendingClipboardItem::Image(att) => ImageProcessor::render_inline_escape(att),
			PendingClipboardItem::Video(_) => None,
		};

		if let Ok(mut state) = self.line_state.lock() {
			state.pending_clipboard.push(item);
		}

		let payload = match preview {
			Some((esc, rows)) => {
				// Pad with `rows` newlines so reedline's prompt redraw scrolls
				// past the rendered image instead of overwriting it.
				let pad = "\n".repeat(rows as usize);
				format!(
					"\x1b[36m{}\x1b[0m\n{}{}",
					label,
					esc.trim_end_matches('\n'),
					pad
				)
			}
			None => format!("\x1b[36m{}\x1b[0m", label),
		};
		let _ = self.notifier.print(payload);
	}
}

/// Probe the clipboard synchronously. A video file reference takes priority
/// over the image representation, because macOS Finder exposes BOTH a file
/// URL (NSURL pasteboard / `«class furl»`) AND a thumbnail icon when a video
/// file is copied — without this ordering we would attach the thumbnail and
/// silently drop the actual video. Falls back to the clipboard image otherwise.
/// Returns `None` if nothing usable is on the clipboard.
fn try_capture_clipboard() -> Option<PendingClipboardItem> {
	if let Some(video) = try_capture_clipboard_video() {
		return Some(PendingClipboardItem::Video(video));
	}

	if let Ok(Some(image)) = ImageProcessor::load_from_clipboard() {
		return Some(PendingClipboardItem::Image(image));
	}

	None
}

/// Resolve a clipboard reference to a supported video file.
///
/// On macOS, Finder copies put the absolute file path on the NSURL pasteboard
/// (`«class furl»`) while the text pasteboard holds only the bare filename.
/// We query NSURL via `osascript` first, then fall back to a text-path probe
/// for users who manually copied an absolute path or `file://` URL.
fn try_capture_clipboard_video() -> Option<crate::session::video::VideoAttachment> {
	#[cfg(target_os = "macos")]
	if let Some(att) = try_capture_clipboard_video_furl_macos() {
		return Some(att);
	}

	try_capture_clipboard_video_text()
}

/// macOS-only: ask AppleScript for the POSIX path of a file URL on the
/// clipboard. Returns `None` if no `furl` is present or it isn't a supported
/// video file.
#[cfg(target_os = "macos")]
fn try_capture_clipboard_video_furl_macos() -> Option<crate::session::video::VideoAttachment> {
	let output = std::process::Command::new("osascript")
		.args([
			"-e",
			"try",
			"-e",
			"POSIX path of (the clipboard as «class furl»)",
			"-e",
			"end try",
		])
		.output()
		.ok()?;
	if !output.status.success() {
		return None;
	}
	let path_str = String::from_utf8(output.stdout).ok()?;
	let trimmed = path_str.trim();
	if trimmed.is_empty() {
		return None;
	}
	let path = std::path::PathBuf::from(trimmed);
	if !path.is_file() || !VideoProcessor::is_supported_video(&path) {
		return None;
	}
	VideoProcessor::load_from_path(&path).ok()
}

/// Inspect the text clipboard for a single path/file URL pointing to a
/// supported video file. Used as a fallback for users who copied a typed
/// path (e.g. from a terminal) and on platforms without a file-URL pasteboard.
fn try_capture_clipboard_video_text() -> Option<crate::session::video::VideoAttachment> {
	let mut clipboard = arboard::Clipboard::new().ok()?;
	let text = clipboard.get_text().ok()?;
	let trimmed = text.trim();
	if trimmed.is_empty() || trimmed.contains('\n') {
		return None;
	}

	let path_str = trimmed.strip_prefix("file://").unwrap_or(trimmed);
	let expanded = if let Some(rest) = path_str.strip_prefix("~/") {
		dirs::home_dir().map(|h| h.join(rest))
	} else {
		Some(std::path::PathBuf::from(path_str))
	}?;

	if !expanded.is_file() || !VideoProcessor::is_supported_video(&expanded) {
		return None;
	}

	VideoProcessor::load_from_path(&expanded).ok()
}

fn format_image_label(att: &crate::session::image::ImageAttachment) -> String {
	let dims = att
		.dimensions
		.map(|(w, h)| format!("{}×{}", w, h))
		.unwrap_or_else(|| "?×?".to_string());
	let size = att.size_bytes.map(format_size).unwrap_or_default();
	let suffix = if size.is_empty() {
		String::new()
	} else {
		format!(", {}", size)
	};
	format!("📎 Image attached ({}{}) — keep typing", dims, suffix)
}

fn format_video_label(att: &crate::session::video::VideoAttachment) -> String {
	let dims = att
		.dimensions
		.map(|(w, h)| format!("{}×{}", w, h))
		.unwrap_or_else(|| att.media_type.clone());
	let size = att.size_bytes.map(format_size).unwrap_or_default();
	let suffix = if size.is_empty() {
		String::new()
	} else {
		format!(", {}", size)
	};
	let name = match &att.source_type {
		crate::session::video::SourceType::File(p) => p
			.file_name()
			.and_then(|n| n.to_str())
			.map(|s| format!(" {}", s))
			.unwrap_or_default(),
		_ => String::new(),
	};
	format!(
		"🎬 Video attached{} ({}{}) — keep typing",
		name, dims, suffix
	)
}

fn format_size(bytes: u64) -> String {
	let kb = bytes as f64 / 1024.0;
	if kb >= 1024.0 {
		format!("{:.1} MB", kb / 1024.0)
	} else {
		format!("{:.0} KB", kb)
	}
}

impl EditMode for EmacsWithShortcutHelp {
	fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
		let event: Event = event.into();

		// Bracketed paste (e.g. Cmd+V on macOS): the terminal reads the clipboard's
		// text representation and forwards it wrapped in `ESC[200~ … ESC[201~`. Image
		// bytes never reach the PTY — but `arboard` can read the OS clipboard out-of-band.
		// Probe it before letting the text paste through; if an image (or video file
		// path) is present, attach it and swallow the textual paste (which is usually
		// just a filename hint or empty).
		if let Event::Paste(text) = &event {
			if let Some(item) = try_capture_clipboard() {
				self.attach_and_notify(item);
				return ReedlineEvent::None;
			}
			// Auto-wrap multiline pastes (3+ lines) in <log>...</log> so the AI
			// receives them as structured context rather than raw text, and so
			// skill auto-activation ignores the pasted content. Wrapping happens
			// at paste time on the pasted chunk only — typed-in newlines and
			// pasted text mixed with typing are preserved verbatim.
			if text.lines().count() >= 3 {
				let wrapped = format!("<log>\n{}\n</log>", text);
				return ReedlineEvent::Edit(vec![reedline::EditCommand::InsertString(wrapped)]);
			}
			// No image / no recognizable video path / short paste — fall through;
			// reedline will insert the text as-is.
		}

		if let Event::Key(KeyEvent {
			code, modifiers, ..
		}) = event
		{
			if modifiers == KeyModifiers::NONE && code == KeyCode::Esc {
				self.meta_pending = true;
				return ReedlineEvent::None;
			}
			if self.meta_pending {
				self.meta_pending = false;
				match code {
					KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Backspace => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::BackspaceWord]);
					}
					KeyCode::Char('d') | KeyCode::Char('D') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::CutWordRight]);
					}
					KeyCode::Char('b') | KeyCode::Char('B') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordLeft {
							select: false,
						}]);
					}
					KeyCode::Char('f') | KeyCode::Char('F') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordRight {
							select: false,
						}]);
					}
					_ => {}
				}
			}
			if modifiers.contains(KeyModifiers::ALT) && modifiers.contains(KeyModifiers::CONTROL) {
				match code {
					KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Backspace => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::BackspaceWord]);
					}
					KeyCode::Char('d') | KeyCode::Char('D') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::CutWordRight]);
					}
					KeyCode::Char('b') | KeyCode::Char('B') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordLeft {
							select: false,
						}]);
					}
					KeyCode::Char('f') | KeyCode::Char('F') => {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveWordRight {
							select: false,
						}]);
					}
					_ => {}
				}
			}
			if code == KeyCode::Char('a') && modifiers == KeyModifiers::CONTROL {
				return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveToLineStart {
					select: false,
				}]);
			}
			if code == KeyCode::Char('e') && modifiers == KeyModifiers::CONTROL {
				if self.reverse_search_active.load(Ordering::SeqCst) {
					return ReedlineEvent::Enter;
				}
				if self.hint_available.load(Ordering::SeqCst) {
					return ReedlineEvent::HistoryHintComplete;
				}
				return ReedlineEvent::Edit(vec![reedline::EditCommand::MoveToLineEnd {
					select: false,
				}]);
			}
			if code == KeyCode::Char('u') && modifiers == KeyModifiers::CONTROL {
				let state = self.line_state.lock().ok();
				if let Some(state) = state {
					let cursor = crate::utils::truncation::floor_char_boundary(
						&state.buffer,
						state.cursor.min(state.buffer.len()),
					);
					let line_start = state.buffer[..cursor]
						.rfind('\n')
						.map(|idx| idx + 1)
						.unwrap_or(0);
					if cursor == line_start && line_start > 0 {
						return ReedlineEvent::Edit(vec![reedline::EditCommand::Backspace]);
					}
				}
				return ReedlineEvent::Edit(vec![reedline::EditCommand::CutFromLineStart]);
			}
			if code == KeyCode::Char('?')
				&& modifiers == KeyModifiers::NONE
				&& self.buffer_empty.load(Ordering::SeqCst)
			{
				return ReedlineEvent::ExecuteHostCommand("__show_shortcuts__".to_string());
			}

			// Ctrl+G: Add message to context without sending to API
			// Set flag in line_state and submit - no buffer modification needed
			if code == KeyCode::Char('g') && modifiers == KeyModifiers::CONTROL {
				if let Ok(mut state) = self.line_state.lock() {
					state.add_without_sending = true;
				}
				return ReedlineEvent::Submit;
			}

			// Ctrl+V: auto-attach clipboard image/video without disturbing the
			// typing buffer. Falls through to default paste when the clipboard
			// holds neither an image nor a video file path.
			if code == KeyCode::Char('v') && modifiers == KeyModifiers::CONTROL {
				if let Some(item) = try_capture_clipboard() {
					self.attach_and_notify(item);
					return ReedlineEvent::None;
				}
				// Fall through: no usable blob — let default Ctrl+V (text paste) run.
			}

			if code == KeyCode::Char('c') && modifiers == KeyModifiers::CONTROL {
				if self.reverse_search_active.load(Ordering::SeqCst) {
					return ReedlineEvent::Esc;
				}
				return ReedlineEvent::CtrlC;
			}

			// Pass through to default emacs handler
		}

		match ReedlineRawEvent::try_from(event) {
			Ok(raw_event) => self.emacs.parse_event(raw_event),
			Err(()) => ReedlineEvent::None,
		}
	}

	fn edit_mode(&self) -> PromptEditMode {
		self.emacs.edit_mode()
	}
}
