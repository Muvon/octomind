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

use super::{CommandOutput, CommandResult};
use anyhow::Result;
use arboard::Clipboard;

pub fn handle_copy(last_response: &str) -> Result<CommandResult> {
	if last_response.is_empty() {
		return Ok(CommandResult::HandledWithOutput(CommandOutput::Copy {
			copied: false,
			length: None,
		}));
	}

	match Clipboard::new() {
		Ok(mut clipboard) => match clipboard.set_text(last_response) {
			Ok(_) => Ok(CommandResult::HandledWithOutput(CommandOutput::Copy {
				copied: true,
				length: Some(last_response.len()),
			})),
			Err(_) => Ok(CommandResult::HandledWithOutput(CommandOutput::Copy {
				copied: false,
				length: None,
			})),
		},
		Err(_) => Ok(CommandResult::HandledWithOutput(CommandOutput::Copy {
			copied: false,
			length: None,
		})),
	}
}
