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

/// Outcome of a /done invocation. Callers (CLI, ACP, WebSocket) decide how to
/// surface this — keep this layer transport-agnostic. Writing to stdout here
/// corrupts the ACP JSON-RPC framing on stdio.
#[derive(Debug, Clone)]
pub enum DoneOutcome {
	Compressed,
	NothingToCompress,
	Failed(String),
}

/// Where `/done` sends its pre-compression transcript for lesson extraction.
#[derive(Debug, Clone, Copy)]
pub enum LessonHandoff {
	/// In-process task: the caller's process outlives it (ACP, WebSocket), so
	/// the prompt is never blocked on the LLM round-trip.
	Task,
	/// Detached `octomind distill` child: the CLI process may end right after
	/// `/done` (a one-shot run, or `/exit` at the next prompt), and the dying
	/// runtime would abort an in-process task.
	DetachedProcess,
}

/// Force-compress conversation context, bypassing all automatic threshold/cooldown/cost guards.
/// The user explicitly requested compression, so we always run it.
pub async fn handle_done(
	session: &mut ChatSession,
	config: &Config,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
	handoff: LessonHandoff,
) -> Result<DoneOutcome> {
	// The session's own live role, the same key recall reads.
	let role = session.role.clone();
	// `/done` compresses before it starts learning. Preserve the raw transcript
	// now; otherwise extraction sees only the generated summary and cannot cite
	// the user's actual words or the original tool evidence. The detached child
	// takes its copy at spawn (a snapshot file), so it is handed off right here.
	let learning_snapshot = match (config.supervisor.learning.enabled, handoff) {
		(false, _) => None,
		(true, LessonHandoff::DetachedProcess) => {
			crate::supervisor::learning::extract::extract_lessons_before_exit(session, config, role.clone(), None);
			// The child is already running: a compression cancelled below must
			// not make /exit extract the same transcript again.
			session.learning_extracted = true;
			None
		}
		(true, LessonHandoff::Task) => Some((
			session.session.messages.clone(),
			session.session.info.name.clone(),
			session.learning_outcome,
		)),
	};
	let outcome = match crate::session::chat::conversation_compression::check_and_compress_conversation(
		session,
		config,
		operation_cancelled,
		crate::session::chat::conversation_compression::CompressionTrigger::Done,
	)
	.await
	{
		Ok(true) => DoneOutcome::Compressed,
		Ok(false) => DoneOutcome::NothingToCompress,
		Err(e) => DoneOutcome::Failed(e.to_string()),
	};

	crate::log_debug!("/done: outcome={:?}", outcome);

	// Lesson extraction never blocks the prompt on the LLM round-trip; the
	// handoff decides what keeps it alive (see `LessonHandoff`).
	if config.supervisor.learning.enabled {
		if let Some((messages, session_name, outcome)) = learning_snapshot {
			let _ = crate::supervisor::learning::extract::spawn_lesson_extraction_snapshot(
				messages,
				config,
				role.clone(),
				None,
				session_name,
				outcome,
				session.session.session_file.clone(),
			);
		}
		// Mark as extracted so /exit and Ctrl+D don't double-extract.
		session.learning_extracted = true;
		// Reset so next user message triggers fresh injection with new query.
		session.learning_injected = false;
		session.clear_active_memory_pack();
		session.pending_recall = false;
		// `/done` is an explicit task boundary: refresh the immutable runtime
		// snapshot so promotions completed since session start become eligible,
		// never in the middle of a tool batch.
		crate::session::guardrails::init_for_session();
		crate::supervisor::learning::evolution::init_for_session(&role);
		crate::mcp::runtime::skill_auto::init_pool(role.split(':').next().unwrap_or(&role));
	}

	Ok(outcome)
}
