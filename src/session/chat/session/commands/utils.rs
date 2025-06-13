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

// Utility functions for command handlers

use crate::config::Config;

// Helper function to get the actual server name for a tool using the same logic as execution
pub async fn get_tool_server_name_async(tool_name: &str, config: &Config) -> String {
	let tool_server_map = crate::mcp::build_tool_server_map(config).await;

	if let Some(target_server) = tool_server_map.get(tool_name) {
		target_server.name.clone()
	} else {
		// Fallback to category guess if no server found, but ensure we still show the tool
		crate::mcp::guess_tool_category(tool_name).to_string()
	}
}

// Format numbers with thousand separators
pub fn format_number(n: u64) -> String {
	n.to_string()
		.chars()
		.rev()
		.collect::<Vec<_>>()
		.chunks(3)
		.map(|chunk| chunk.iter().collect::<String>())
		.collect::<Vec<_>>()
		.join(",")
		.chars()
		.rev()
		.collect()
}
