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

// Tool call processing module - extracted from response.rs for better modularity

use crate::session::chat::tool_error_tracker::ToolErrorTracker;

pub struct ToolProcessor {
	pub error_tracker: ToolErrorTracker,
}

impl ToolProcessor {
	pub fn new() -> Self {
		Self {
			error_tracker: ToolErrorTracker::new(3),
		}
	}
}

impl Default for ToolProcessor {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
#[path = "tool_processor_tests.rs"]
mod tests;
