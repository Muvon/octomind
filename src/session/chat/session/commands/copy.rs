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

// Copy command handler

use anyhow::Result;
use arboard::Clipboard;
use colored::Colorize;

pub fn handle_copy(last_response: &str) -> Result<bool> {
	if last_response.is_empty() {
		println!(
			"{}",
			"No response to copy. Send a message first.".bright_yellow()
		);
	} else {
		match Clipboard::new() {
			Ok(mut clipboard) => match clipboard.set_text(last_response) {
				Ok(_) => {
					println!("{}", "Last response copied to clipboard.".bright_green());
				}
				Err(e) => {
					println!("{}: {}", "Failed to copy to clipboard".bright_red(), e);
				}
			},
			Err(e) => {
				println!("{}: {}", "Failed to access clipboard".bright_red(), e);
			}
		}
	}
	Ok(false)
}
