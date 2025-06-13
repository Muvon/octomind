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

// Save command handler

use super::super::core::ChatSession;
use anyhow::Result;
use colored::Colorize;

pub fn handle_save(session: &mut ChatSession) -> Result<bool> {
	if let Err(e) = session.save() {
		println!("{}: {}", "Failed to save session".bright_red(), e);
	} else {
		println!("{}", "Session saved successfully.".bright_green());
	}
	Ok(false)
}
