use crossterm::{
	event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
	terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io::{stdout, Write};

pub struct Prompt {
	buffer: String,
	prefix: String,
}

impl Prompt {
	pub fn new() -> Self {
		Self {
			buffer: String::new(),
			prefix: "...".to_string(),
		}
	}

	pub fn read_line(&mut self) -> Option<String> {
		enable_raw_mode().unwrap();
		self.buffer.clear();

		loop {
			if let Event::Key(key) = event::read().unwrap() {
				if key.kind == KeyEventKind::Press {
					match key.code {

						KeyCode::Enter => {
							if key.modifiers.contains(KeyModifiers::CONTROL) {
								println!();
								print!("{}", self.prefix);
								stdout().flush().unwrap();
								self.buffer.push('\n');
							} else {
								disable_raw_mode().unwrap();
								println!();
								return Some(self.buffer.clone());
							}
						}

						KeyCode::Char(c) => {
							// Check for Ctrl+D
							if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'd' {
								disable_raw_mode().unwrap();
								println!();
								return None;
							}
							self.buffer.push(c);
							print!("{}", c);
							stdout().flush().unwrap();
						}
						KeyCode::Backspace => {
							if !self.buffer.is_empty() {
								if self.buffer.ends_with('\n') {
									// Move cursor up and clear the prefix
									print!("\x1B[A\x1B[2K");
									// Print the current line again
									print!("\r{}", self.buffer.lines().last().unwrap_or(""));
								} else {
									print!("\x08 \x08");
								}
								self.buffer.pop();
								stdout().flush().unwrap();
							}
						}
						KeyCode::Esc => {
							disable_raw_mode().unwrap();
							return None;
						}
						_ => {}
					}
				}
			}
		}
	}
}

