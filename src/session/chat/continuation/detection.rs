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

// Session continuation detection - checks when to trigger continuation

use crate::config::Config;
use crate::session::chat::session::ChatSession;

/// Parameters for continuation processing
pub struct ContinuationParams<'a> {
	pub chat_session: &'a mut ChatSession,
	pub config: &'a Config,
	pub current_tokens: usize,
}

impl<'a> ContinuationParams<'a> {
	pub fn new(
		chat_session: &'a mut ChatSession,
		config: &'a Config,
		current_tokens: usize,
	) -> Self {
		Self {
			chat_session,
			config,
			current_tokens,
		}
	}
}

/// Check if we should trigger session continuation using adaptive threshold
pub fn should_trigger_continuation(params: &ContinuationParams) -> bool {
	if params.config.max_session_tokens_threshold == 0 || params.chat_session.continuation_pending {
		return false;
	}

	// Use existing adaptive threshold logic from context_truncation
	let effective_threshold =
		crate::session::chat::context_truncation::calculate_effective_threshold(
			params.chat_session,
			params.config,
		);
	params.current_tokens >= effective_threshold
}

/// Check if we're currently in continuation process
pub fn is_continuation_in_progress(chat_session: &ChatSession) -> bool {
	chat_session.continuation_pending
}
