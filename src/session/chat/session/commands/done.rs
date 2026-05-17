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

// /done command handler - force-compress conversation context regardless of thresholds

use super::super::core::ChatSession;
use crate::config::Config;
use anyhow::Result;
use colored::Colorize;

/// Force-compress conversation context, bypassing all automatic threshold/cooldown/cost guards.
/// The user explicitly requested compression, so we always run it.
pub async fn handle_done(
	session: &mut ChatSession,
	config: &Config,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
) -> Result<(bool, bool)> {
	let compressed =
		match crate::session::chat::conversation_compression::check_and_compress_conversation(
			session,
			config,
			operation_cancelled,
			crate::session::chat::conversation_compression::CompressionTrigger::Done,
		)
		.await
		{
			Ok(true) => {
				println!("{}", "✅ Conversation compressed.".bright_green());
				true
			}
			Ok(false) => {
				println!("{}", "ℹ️  Nothing to compress.".bright_cyan());
				false
			}
			Err(e) => {
				println!("{}: {}", "❌ Compression failed".bright_red(), e);
				false
			}
		};

	crate::log_debug!("/done: compression={}", compressed);

	// Fire-and-forget lesson extraction — do NOT block the prompt on the LLM round-trip.
	// Same pattern as /exit and Ctrl+D in main_loop.rs.
	if config.learning.enabled {
		let role = crate::config::get_thread_role().unwrap_or_default();
		crate::learning::extract::spawn_lesson_extraction(session, config, role, None);
		// Mark as extracted so /exit and Ctrl+D don't double-extract.
		session.learning_extracted = true;
		// Reset so next user message triggers fresh injection with new query.
		session.learning_injected = false;
	}

	// Returns (exit_flag, reset_first_message_processed)
	Ok((false, false))
}
