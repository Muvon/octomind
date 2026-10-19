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

// Animation helper functions

use std::time::Duration;

/// Format elapsed time in human-readable format
pub fn format_elapsed_time(elapsed: Duration) -> String {
	let total_secs = elapsed.as_secs();

	if total_secs < 60 {
		// Less than 1 minute: show seconds
		format!("{}s", total_secs)
	} else if total_secs < 3600 {
		// Less than 1 hour: show minutes and seconds
		let mins = total_secs / 60;
		let secs = total_secs % 60;
		if secs > 0 {
			format!("{}m {}s", mins, secs)
		} else {
			format!("{}m", mins)
		}
	} else {
		// 1 hour or more: show hours, minutes, and seconds
		let hours = total_secs / 3600;
		let mins = (total_secs % 3600) / 60;
		let secs = total_secs % 60;
		if mins > 0 && secs > 0 {
			format!("{}h {}m {}s", hours, mins, secs)
		} else if mins > 0 {
			format!("{}h {}m", hours, mins)
		} else if secs > 0 {
			format!("{}h {}s", hours, secs)
		} else {
			format!("{}h", hours)
		}
	}
}

#[cfg(test)]
#[path = "animation_tests.rs"]
mod tests;
