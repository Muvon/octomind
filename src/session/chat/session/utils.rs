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

// Utility functions for session display

use crate::config::Config;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Standard project instructions file, auto-loaded at session start when present
pub const AGENTS_FILE: &str = "AGENTS.md";

// Utility function to format numbers in a human-readable format
pub fn format_number(number: u64) -> String {
	if number == 0 {
		return "0".to_string();
	}

	if number < 1_000 {
		number.to_string()
	} else if number < 10_000 {
		// For numbers 1K-9.99K, show one decimal place
		let k = number as f64 / 1_000.0;
		if k.fract() == 0.0 {
			format!("{}K", k as u64)
		} else {
			format!("{:.1}K", k)
		}
	} else if number < 1_000_000 {
		// For numbers 10K-999K, show whole K
		format!("{}K", number / 1_000)
	} else if number < 10_000_000 {
		// For numbers 1M-9.99M, show one decimal place
		let m = number as f64 / 1_000_000.0;
		if m.fract() == 0.0 {
			format!("{}M", m as u64)
		} else {
			format!("{:.1}M", m)
		}
	} else if number < 1_000_000_000 {
		// For numbers 10M-999M, show whole M
		format!("{}M", number / 1_000_000)
	} else {
		// For numbers 1B+, show one decimal place
		let b = number as f64 / 1_000_000_000.0;
		if b.fract() == 0.0 {
			format!("{}B", b as u64)
		} else {
			format!("{:.1}B", b)
		}
	}
}

/// Generate initial session messages (welcome + instructions if available)
/// Returns vector of messages to be inserted after system message
pub async fn get_initial_messages(
	config: &Config,
	role: &str,
	current_dir: &std::path::Path,
) -> Result<Vec<crate::session::Message>> {
	let mut initial_messages = Vec::new();

	// 1. Generate welcome message (assistant role)
	let role_config = config.get_role_config_struct(role);
	let welcome_message = crate::session::helper_functions::process_placeholders_async_with_role(
		&role_config.welcome,
		current_dir,
		Some(role),
	)
	.await;

	let welcome_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: welcome_message,
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		..Default::default()
	};
	initial_messages.push(welcome_msg);

	// 2. Generate instructions message if AGENTS.md exists (user role)
	let instructions_path = current_dir.join(AGENTS_FILE);
	if instructions_path.exists() {
		if let Ok(instructions_content) = std::fs::read_to_string(&instructions_path) {
			if instructions_content.trim().is_empty() {
				log_debug!("Skipping empty instructions file {}", AGENTS_FILE);
			} else {
				let processed_instructions =
					crate::session::helper_functions::process_placeholders_async_with_role(
						&instructions_content,
						current_dir,
						Some(role),
					)
					.await;

				let instructions_msg = crate::session::Message {
					role: "user".to_string(),
					// Wrap in <instructions> so task/compression/learning logic treats
					// this as system-managed (is_real_user_task_message) and never
					// mistakes the project instructions for a genuine user task. Mirrors
					// the non-interactive path in prompt_setup.rs.
					content: format!(
						"<instructions>\n{}\n</instructions>",
						processed_instructions
					),
					timestamp: std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					cached: false,
					..Default::default()
				};
				initial_messages.push(instructions_msg);

				log_info!(
					"Added {} content as user message with variable processing",
					AGENTS_FILE
				);
			}
		} else {
			log_debug!("Failed to read {}", AGENTS_FILE);
		}
	}

	Ok(initial_messages)
}

#[cfg(test)]
#[path = "utils_tests.rs"]
mod tests;
