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

// Thinking block display utilities

use crate::providers::ThinkingBlock;
use colored::Colorize;

/// Display thinking block in CLI: a dim `· thinking` header followed by the
/// body in dim italic. No trailing rule — block ends when normal output
/// resumes. Matches the `·` prefix used by info-style status lines elsewhere.
pub fn display_thinking(thinking: &ThinkingBlock) {
	println!("{}", "· thinking".bright_black());
	println!("{}", thinking.content.bright_black().italic());
}

#[cfg(test)]
#[path = "thinking_display_tests.rs"]
mod tests;
