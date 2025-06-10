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

// Implementation of a command completer for rustyline
use colored::*;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::Validator;
use rustyline::Helper;
use std::borrow::Cow::{self, Borrowed, Owned};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct CommandCompleter {
	commands: Vec<String>,
}

impl CommandCompleter {
	fn new() -> Self {
		let commands = crate::session::chat::COMMANDS
			.iter()
			.map(|&s| s.to_string())
			.collect();
		Self { commands }
	}

	/// Check if the given file extension is a supported image format
	fn is_image_file(path: &str) -> bool {
		let supported_extensions = [
			".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".tiff", ".tif", ".ico", ".svg",
			".avif", ".heic", ".heif",
		];

		let path_lower = path.to_lowercase();
		supported_extensions
			.iter()
			.any(|ext| path_lower.ends_with(ext))
	}

	/// Expand tilde (~) to home directory
	fn expand_tilde(path: &str) -> PathBuf {
		if let Some(stripped) = path.strip_prefix("~/") {
			if let Some(home) = dirs::home_dir() {
				home.join(stripped)
			} else {
				PathBuf::from(path)
			}
		} else if path == "~" {
			dirs::home_dir().unwrap_or_else(|| PathBuf::from(path))
		} else {
			PathBuf::from(path)
		}
	}

	/// Custom file completion that handles absolute paths and tilde expansion
	fn complete_file_path(file_part: &str) -> Vec<Pair> {
		if file_part.is_empty() {
			// Show current directory contents
			return Self::list_directory_contents(".");
		}

		// Expand tilde if present
		let expanded_path = Self::expand_tilde(file_part);
		let expanded_str = expanded_path.to_string_lossy();

		// If the path ends with a separator, list contents of that directory
		if file_part.ends_with('/') || file_part.ends_with('\\') {
			return Self::list_directory_contents(&expanded_str);
		}

		// Determine the parent directory and filename part
		let (parent_dir, filename_part) = if let Some(parent) = expanded_path.parent() {
			let parent_str = parent.to_str().unwrap_or(".");
			// If parent is empty string (for relative paths without ./), use current directory
			let actual_parent = if parent_str.is_empty() {
				"."
			} else {
				parent_str
			};

			let filename_part = expanded_path
				.file_name()
				.and_then(|n| n.to_str())
				.unwrap_or("");

			(actual_parent, filename_part)
		} else {
			// No parent found, treat as current directory search
			(".", file_part)
		};

		let mut candidates = Self::list_directory_contents(parent_dir);

		// Filter candidates that start with the filename part
		if !filename_part.is_empty() {
			let filename_lower = filename_part.to_lowercase();
			candidates.retain(|candidate| {
				let name = Path::new(&candidate.replacement)
					.file_name()
					.and_then(|n| n.to_str())
					.unwrap_or("")
					.to_lowercase();
				name.starts_with(&filename_lower)
			});
		}

		// Adjust the replacement paths to be relative to the original input
		for candidate in &mut candidates {
			if file_part.starts_with("~/") {
				// Convert back to tilde notation
				if let Some(home) = dirs::home_dir() {
					if let Ok(relative) = PathBuf::from(&candidate.replacement).strip_prefix(&home)
					{
						candidate.replacement = format!("~/{}", relative.to_string_lossy());
					}
				}
			} else if file_part.starts_with('/') {
				// Keep absolute path
				// candidate.replacement is already correct
			} else {
				// For relative paths, make sure we maintain the relative nature
				if let Ok(relative) = PathBuf::from(&candidate.replacement)
					.strip_prefix(std::env::current_dir().unwrap_or_default())
				{
					candidate.replacement = relative.to_string_lossy().to_string();
				}
			}
		}

		candidates
	}

	/// List contents of a directory, returning both directories and image files
	fn list_directory_contents(dir_path: &str) -> Vec<Pair> {
		let mut candidates = Vec::new();

		if let Ok(entries) = fs::read_dir(dir_path) {
			for entry in entries.flatten() {
				let path = entry.path();
				let path_str = path.to_string_lossy().to_string();

				if path.is_dir() {
					// Add directory with trailing slash
					let display = format!(
						"{}/",
						path.file_name().and_then(|n| n.to_str()).unwrap_or("")
					);
					candidates.push(Pair {
						display: display.clone(),
						replacement: format!("{}/", path_str),
					});
				} else if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
					if Self::is_image_file(filename) {
						candidates.push(Pair {
							display: filename.to_string(),
							replacement: path_str,
						});
					}
				}
			}
		}

		// Sort: directories first, then files
		candidates.sort_by(|a, b| {
			let a_is_dir = a.replacement.ends_with('/');
			let b_is_dir = b.replacement.ends_with('/');

			match (a_is_dir, b_is_dir) {
				(true, false) => std::cmp::Ordering::Less,
				(false, true) => std::cmp::Ordering::Greater,
				_ => a.replacement.cmp(&b.replacement),
			}
		});

		candidates
	}

	/// Filter and prepare completion candidates for better UX with circular completion
	fn filter_and_limit_candidates(candidates: Vec<Pair>, _file_part: &str) -> Vec<Pair> {
		// For circular completion, limit to fewer total candidates for better UX
		const MAX_TOTAL: usize = 8; // Reasonable limit for circular experience

		let mut result = candidates;
		result.truncate(MAX_TOTAL);
		result
	}
}

impl Completer for CommandCompleter {
	type Candidate = Pair;

	fn complete(
		&self,
		line: &str,
		pos: usize,
		_ctx: &rustyline::Context<'_>,
	) -> Result<(usize, Vec<Self::Candidate>), ReadlineError> {
		// Handle /image command with file completion
		if line.starts_with("/image ") {
			let image_prefix = "/image ";
			let prefix_len = image_prefix.len();

			// Extract the file path part after "/image "
			let file_part = if line.len() > prefix_len {
				&line[prefix_len..]
			} else {
				""
			};

			// Calculate position within the file part (not used in our custom completion)
			let _file_pos = pos.saturating_sub(prefix_len);

			// Use our custom file completion that handles absolute paths and tilde expansion
			let candidates = Self::complete_file_path(file_part);
			let filtered_candidates = Self::filter_and_limit_candidates(candidates, file_part);

			// For file completion, we want to replace from the start of the file part
			Ok((prefix_len, filtered_candidates))
		} else if !line.starts_with('/') {
			// No completion for non-commands
			Ok((0, vec![]))
		} else {
			// Handle regular command completion
			let candidates: Vec<Pair> = self
				.commands
				.iter()
				.filter(|cmd| cmd.starts_with(line))
				.map(|cmd| Pair {
					display: cmd.clone(),
					replacement: cmd.clone(),
				})
				.collect();

			Ok((0, candidates))
		}
	}
}

// We need to implement these traits to make CommandHelper work with rustyline
impl Hinter for CommandCompleter {
	type Hint = String;

	fn hint(&self, line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
		if line.is_empty() || !line.starts_with('/') {
			return None;
		}

		// Special hint for /image command
		if line == "/image" {
			return Some(" <path_to_image>".to_string());
		}

		if line.starts_with("/image ") && line.len() > 7 {
			let file_part = &line[7..]; // "/image ".len() = 7
			if file_part.is_empty() {
				return Some("Start typing image file path...".to_string());
			}
			return None; // Let filename completer handle this
		}

		// Look for a command that starts with the current input
		self.commands
			.iter()
			.find(|cmd| cmd.starts_with(line))
			.map(|cmd| cmd[line.len()..].to_string())
	}
}

impl Highlighter for CommandCompleter {
	fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
		// Only apply highlighting to commands (lines starting with '/')
		if line.starts_with('/') {
			// Special handling for /image command with file path
			if line.starts_with("/image ") && line.len() > 7 {
				let image_cmd = "/image";
				let file_part = &line[7..]; // "/image ".len() = 7

				// Check if the file path points to a valid image
				if !file_part.is_empty()
					&& Path::new(file_part).exists()
					&& Self::is_image_file(file_part)
				{
					// Highlight valid image path in bright green
					return Owned(format!(
						"{} {}",
						image_cmd.green(),
						file_part.bright_green()
					));
				} else if !file_part.is_empty() {
					// Highlight invalid/non-existent path in yellow
					return Owned(format!("{} {}", image_cmd.green(), file_part.yellow()));
				} else {
					// Just the command part is green
					return Owned(format!("{} ", image_cmd.green()));
				}
			}

			// Check if this is a valid command
			let is_valid_command = self
				.commands
				.iter()
				.any(|cmd| line == cmd || cmd.starts_with(line));

			if is_valid_command {
				// Highlight valid commands in green
				Owned(line.green().to_string())
			} else {
				// Keep invalid commands normal colored
				Borrowed(line)
			}
		} else {
			Borrowed(line)
		}
	}

	fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
		false
	}

	fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
		// Make hints appear in dim gray color - like bash autocomplete
		Owned(hint.bright_black().to_string())
	}
}

impl Validator for CommandCompleter {}

// Helper for rustyline
pub struct CommandHelper {
	completer: CommandCompleter,
	hinter: Option<HistoryHinter>,
}

impl CommandHelper {
	pub fn new() -> Self {
		Self {
			completer: CommandCompleter::new(),
			hinter: Some(HistoryHinter {}),
		}
	}
}

// Implement Helper trait
impl Helper for CommandHelper {}

// Implement the required traits for rustyline helper
impl Completer for CommandHelper {
	type Candidate = Pair;

	fn complete(
		&self,
		line: &str,
		pos: usize,
		ctx: &rustyline::Context<'_>,
	) -> Result<(usize, Vec<Self::Candidate>), ReadlineError> {
		self.completer.complete(line, pos, ctx)
	}
}

impl Hinter for CommandHelper {
	type Hint = String;

	fn hint(&self, line: &str, pos: usize, ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
		if line.starts_with('/') {
			self.completer.hint(line, pos, ctx)
		} else if let Some(hinter) = &self.hinter {
			hinter.hint(line, pos, ctx)
		} else {
			None
		}
	}
}

impl Highlighter for CommandHelper {
	fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
		self.completer.highlight(line, pos)
	}

	fn highlight_char(&self, line: &str, pos: usize, kind: CmdKind) -> bool {
		self.completer.highlight_char(line, pos, kind)
	}

	fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
		self.completer.highlight_hint(hint)
	}
}

impl Validator for CommandHelper {}
