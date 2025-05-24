// Implementation of a command completer for rustyline
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::Validator;
use rustyline::Helper;
use std::borrow::Cow::{self, Borrowed, Owned};
use colored::*;

#[derive(Default)]
struct CommandCompleter {
	commands: Vec<String>,
}

impl CommandCompleter {
	fn new() -> Self {
		let commands = crate::session::chat::COMMANDS.iter().map(|&s| s.to_string()).collect();
		Self { commands }
	}
}

impl Completer for CommandCompleter {
	type Candidate = Pair;

	fn complete(
		&self,
		line: &str,
		_pos: usize,
		_ctx: &rustyline::Context<'_>,
	) -> Result<(usize, Vec<Self::Candidate>), ReadlineError> {
		// Only complete if the line starts with a slash
		if !line.starts_with('/') {
			return Ok((0, vec![]));
		}

		let candidates: Vec<Pair> = self.commands
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

// We need to implement these traits to make CommandHelper work with rustyline
impl Hinter for CommandCompleter {
	type Hint = String;

	fn hint(&self, line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
		if line.is_empty() || !line.starts_with('/') {
			return None;
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
			// Check if this is a valid command
			let is_valid_command = self.commands.iter().any(|cmd| line == cmd || cmd.starts_with(line));
			
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

	fn highlight_char(&self, _line: &str, _pos: usize) -> bool {
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

	fn highlight_char(&self, line: &str, pos: usize) -> bool {
		self.completer.highlight_char(line, pos)
	}

	fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
		self.completer.highlight_hint(hint)
	}
}

impl Validator for CommandHelper {}
