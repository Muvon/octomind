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

// Utility functions for formatting responses and content

use regex::Regex;

// Utility function to format time in a human-readable format
pub fn format_duration(milliseconds: u64) -> String {
	if milliseconds == 0 {
		return "0ms".to_string();
	}

	let ms = milliseconds % 1000;
	let seconds = (milliseconds / 1000) % 60;
	let minutes = (milliseconds / 60000) % 60;
	let hours = milliseconds / 3600000;

	let mut parts = Vec::new();

	if hours > 0 {
		parts.push(format!("{}h", hours));
	}
	if minutes > 0 {
		parts.push(format!("{}m", minutes));
	}
	if seconds > 0 {
		parts.push(format!("{}s", seconds));
	}
	if ms > 0 || parts.is_empty() {
		if parts.is_empty() {
			parts.push(format!("{}ms", ms));
		} else if ms >= 100 {
			// Only show milliseconds if >= 100ms when other units are present
			parts.push(format!("{}ms", ms));
		}
	}

	parts.join(" ")
}

// Function to remove function_calls blocks from content
pub fn remove_function_calls(content: &str) -> String {
	// Use multiple regex patterns to catch different function call formats
	let patterns = [
		r#"<(antml:)?function_calls>\s*(.+?)\s*</(antml:)?function_calls>"#,
		r#"```(json)?\s*\[?\s*\{\s*"tool_name":.+?\}\s*\]?\s*```"#,
		r#"^\s*\{\s*"tool_name":.+?\}\s*$"#,
	];

	let mut result = content.to_string();

	for pattern in patterns {
		if let Ok(re) = Regex::new(pattern) {
			result = re.replace_all(&result, "").to_string();
		}
	}

	// Also remove "I'll use the X tool" phrases that often accompany function calls
	if let Ok(re) = Regex::new(r#"(?i)I'?ll use the \w+ tool[^\n]*"#) {
		result = re.replace_all(&result, "").to_string();
	}

	result.trim().to_string()
}
