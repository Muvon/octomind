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

//! Applies the effects every answered event can carry.

use super::host::Effects;
use super::protocol::{PlanTaskSpec, PlanUpdate};
use crate::mcp::core::plan;
use crate::mcp::core::plan::PlanTaskDirective;
use crate::session::chat::session::ChatSession;
use crate::session::Session;
use anyhow::Result;

/// A note that holds this text gets the plan checklist in its place, rendered
/// after the plan change of the same answer.
const PLAN_CHECKLIST_PLACEHOLDER: &str = "{plan_checklist}";

/// Apply what an answered event left for the session: queue its notes, apply a
/// plan change, show notices, book usage, refresh registered rules and pins.
pub async fn apply(chat_session: &mut ChatSession, effects: Effects) -> Result<()> {
	let session_id = chat_session.session.info.name.clone();

	if let Some(update) = effects.plan {
		if let Err(error) = apply_plan(chat_session, update) {
			crate::plugins::record_plan_error(&session_id, &format!("{error:#}"));
		}
	}

	if !effects.notes.is_empty() || !effects.result_notes.is_empty() {
		let checklist = plan::render_plan_checklist().unwrap_or_default();
		let with_checklist = |notes: Vec<String>| -> Vec<String> {
			notes
				.into_iter()
				.map(|note| note.replace(PLAN_CHECKLIST_PLACEHOLDER, &checklist))
				.collect()
		};
		chat_session.queue_plugin_result_notes(with_checklist(effects.result_notes));
		chat_session.queue_plugin_notes(with_checklist(effects.notes));
	}

	settle(&session_id, Some(&mut chat_session.session));

	let pins = crate::plugins::pinned_blocks(&session_id);
	if pins != chat_session.session.info.plugin_pins {
		chat_session.session.info.plugin_pins = pins;
	}
	Ok(())
}

/// The part of [`apply`] that needs no chat session: show notices, book usage
/// and the time the agent waited into `session`, refresh registered rules.
pub fn settle(session_id: &str, session: Option<&mut Session>) {
	for notice in super::take_notices(session_id) {
		crate::session::notice::supervisor_notice(&notice);
	}

	// Without a session the usage and reports stay in the host: the next `apply` that has one books them.
	if let Some(session) = session {
		// The time the agent waited is what octomind measured, not what plugins say they spent.
		session.info.supervisor_accounting.blocking_wall_time_ms += super::take_waited_ms(session_id);
		for (plugin, usage) in &super::take_usage(session_id) {
			crate::session::accounting::record_plugin_usage(session, plugin, usage);
		}
		for (plugin, report) in super::take_reports(session_id) {
			session.info.supervisor_accounting.plugin_reports.insert(plugin, report);
		}
	}

	crate::session::guardrails::sync_plugin_rules(session_id);
	crate::mcp::runtime::skill_auto::sync_plugin_skill_dirs(session_id);
}

fn apply_plan(chat_session: &ChatSession, update: PlanUpdate) -> Result<()> {
	match update {
		PlanUpdate::Start { title, tasks } => plan::sidecar_start(&title, &directives(tasks)),
		PlanUpdate::Advance { summary } => plan::sidecar_advance(&summary),
		PlanUpdate::Revise { reason, tasks } => {
			plan::sidecar_revise(&reason, &directives(tasks))?;
			// The work after a revision is judged on its own window of messages.
			plan::set_current_task_start_index(chat_session.get_message_count());
			Ok(())
		}
		PlanUpdate::Finish { summary } => plan::sidecar_finish(&summary),
	}
}

fn directives(tasks: Vec<PlanTaskSpec>) -> Vec<PlanTaskDirective> {
	tasks
		.into_iter()
		.map(|task| PlanTaskDirective {
			title: task.title,
			done_when: task.done_when,
		})
		.collect()
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
