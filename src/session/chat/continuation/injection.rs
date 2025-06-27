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

// Session continuation injection - injects summary request when limits reached

use crate::log_info;
use crate::session::chat::continuation::constants::SUMMARY_REQUEST_PROMPT;
use crate::session::chat::continuation::detection::ContinuationParams;
use anyhow::Result;

/// Inject summary request into current session
/// This can happen during ANY processing step, not just user input
pub fn inject_summary_request(params: &mut ContinuationParams) -> Result<()> {
	// Log token limit reached (less visible to user)
	log_info!("Token limit reached during processing - requesting work summary...");

	// Add summary request as user message
	let summary_message = crate::session::Message {
		role: "user".to_string(),
		content: SUMMARY_REQUEST_PROMPT.to_string(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		tool_calls: None,
		tool_call_id: None,
		name: None,
		images: None,
	};

	params.chat_session.session.messages.push(summary_message);
	params.chat_session.continuation_pending = true;

	log_info!("Summary request injected into conversation flow");

	Ok(())
}
