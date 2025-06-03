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

// Tool error tracking to detect loops and patterns

use std::collections::HashMap;

// Structure to track tool call errors to detect loops
#[derive(Default)]
pub struct ToolErrorTracker {
	tool_errors: HashMap<String, HashMap<String, usize>>,
	max_consecutive_errors: usize,
}

impl ToolErrorTracker {
	pub fn new(max_errors: usize) -> Self {
		Self {
			tool_errors: HashMap::new(),
			max_consecutive_errors: max_errors,
		}
	}

	// Record an error for a tool and return true if we've hit the error threshold
	pub fn record_error(&mut self, tool_name: &str) -> bool {
		// Get the nested hash map for this tool, creating it if it doesn't exist
		let server_map = self.tool_errors.entry(tool_name.to_string()).or_default();

		// For now, we use a special key to track errors. In the future this could be server-specific
		let curr_server = "current_server".to_string();

		// Increment the error count for this tool on this server
		let count = server_map.entry(curr_server).or_insert(0);
		*count += 1;

		*count >= self.max_consecutive_errors
	}

	// Record a successful tool call, resetting the error counter for this tool from any server
	pub fn record_success(&mut self, tool_name: &str) {
		if let Some(server_map) = self.tool_errors.get_mut(tool_name) {
			server_map.clear(); // Clear all server counts for this tool
		}
	}

	// Get the current error count for a specific tool
	pub fn get_error_count(&self, tool_name: &str) -> usize {
		if let Some(server_map) = self.tool_errors.get(tool_name) {
			if let Some(count) = server_map.get("current_server") {
				return *count;
			}
		}
		0
	}

	// Not used for now, but kept for future extensibility
	#[allow(dead_code)]
	pub fn reset(&mut self) {
		self.tool_errors.clear();
	}

	// Public getter for max_consecutive_errors
	pub fn max_consecutive_errors(&self) -> usize {
		self.max_consecutive_errors
	}
}
