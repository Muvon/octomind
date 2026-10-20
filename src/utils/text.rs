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

// Text manipulation utilities

/// Collapse a string to a single line, replace newlines and carriage returns
/// with spaces, and cap its length at `max` characters (including `…` on overflow).
pub fn truncate(s: &str, max: usize) -> String {
	let s = s.replace(['\n', '\r'], " ");
	if s.chars().count() <= max {
		s
	} else {
		let head: String = s.chars().take(max.saturating_sub(1)).collect();
		format!("{head}…")
	}
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
