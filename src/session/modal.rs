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

//! Terminal modal overlay system for displaying content above the input prompt

use std::io::{self, Write};

/// Display content above the current cursor position
///
/// Uses ANSI escape codes to save cursor, move up, display content, and restore cursor.
/// This creates a non-intrusive overlay that doesn't affect the input line.
///
/// # Arguments
/// * `lines` - Number of lines the content will occupy
/// * `content` - Closure that renders the content
pub fn show_overlay<F>(lines: usize, content: F)
where
	F: FnOnce(),
{
	print!("\x1B7"); // Save cursor position
	print!("\x1B[{}A", lines); // Move up N lines
	content(); // Render content
	print!("\x1B8"); // Restore cursor position
	let _ = io::stdout().flush();
}

/// Clear overlay content above the current cursor position
///
/// # Arguments
/// * `lines` - Number of lines to clear (must match the original overlay size)
pub fn clear_overlay(lines: usize) {
	print!("\x1B7"); // Save cursor position
	print!("\x1B[{}A", lines); // Move up N lines
	print!("\x1B[J"); // Clear from cursor to end of screen
	print!("\x1B8"); // Restore cursor position
	let _ = io::stdout().flush();
}

#[cfg(test)]
#[path = "modal_tests.rs"]
mod tests;
