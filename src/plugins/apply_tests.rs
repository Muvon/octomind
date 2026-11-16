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

use super::*;
use crate::plugins::protocol::{event, RequestBefore, RequestPhase, TurnKind};
use crate::plugins::test_support::{connect_stub, StubPlugin};
use crate::plugins::EventContext;
use serde_json::{json, Value};
use std::time::Duration;

const ROLE: &str = "developer";
/// How long a slow stub plugin takes to answer.
const SLOW_ANSWER: Duration = Duration::from_millis(150);

/// The sums of a report; a plugin must send every one of them.
fn totals(chat_calls: u64, cost: f64) -> Value {
	json!({"chat_calls": chat_calls, "decision_calls": 0, "wall_blocking_ms": 0, "api_ms": 0,
		"input_tokens": 0, "output_tokens": 0, "reasoning_tokens": 0, "cost": cost})
}

/// A chat session with a stub plugin switched on for it. The stub answers
/// `request.before` with the given answers, oldest first.
struct Rig {
	chat: ChatSession,
	stub: StubPlugin,
	id: String,
	dir: tempfile::TempDir,
}

async fn rig(answers: Vec<Value>) -> Rig {
	rig_with(StubPlugin::new(&[event::REQUEST_BEFORE]).queue(answers)).await
}

async fn rig_with(stub: StubPlugin) -> Rig {
	let id = format!("apply-{}", uuid::Uuid::new_v4());
	let dir = tempfile::tempdir().unwrap();
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let mut chat = ChatSession::for_tests(Vec::new());
	chat.session.info.name = id.clone();
	Rig { chat, stub, id, dir }
}

impl Rig {
	/// Send `request.before` and return what the plugin left for the caller.
	async fn send(&self) -> Effects {
		let ctx = EventContext {
			session_id: &self.id,
			parent_session_id: None,
			role: ROLE,
			workdir: self.dir.path(),
			project: "proj",
			messages: &[],
			detached: false,
			status: None,
			plan: None,
			tools: None,
			active_skills: Vec::new(),
		};
		let payload = RequestBefore {
			phase: RequestPhase::TurnStart,
			turn: TurnKind::User,
			free_context_tokens: None,
			pending_async: false,
		};
		crate::plugins::request_before(&ctx, payload).await
	}

	/// Send an event and apply its answer, as a call site does.
	async fn answered_event(&mut self) {
		let effects = self.send().await;
		apply(&mut self.chat, effects).await.unwrap();
	}
}

fn tasks() -> Value {
	json!([
		{"title": "implement", "done_when": "the code is written"},
		{"title": "verify", "done_when": "the tests pass"},
	])
}

#[tokio::test]
async fn note_notice_usage_and_plan_start_are_all_applied() {
	let mut rig = rig(vec![json!({
		"note": ["Plan now:\n{plan_checklist}"],
		"notice": ["plugin says hello"],
		"usage": [{"label": "judge", "calls": 2, "input_tokens": 10, "output_tokens": 3, "cost": 0.5}],
		"plan": {"action": "start", "title": "Ship it", "tasks": tasks()},
	})])
	.await;
	let id = rig.id.clone();
	crate::session::context::with_session_id(id.clone(), async {
		let effects = rig.send().await;
		apply(&mut rig.chat, effects).await.unwrap();
		assert!(plan::has_active_plan());
	})
	.await;

	assert_eq!(rig.chat.plugin_notes.len(), 1);
	let note = &rig.chat.plugin_notes[0];
	assert!(!note.contains(PLAN_CHECKLIST_PLACEHOLDER), "{note}");
	assert!(
		note.starts_with("Plan now:\n") && note.contains("implement") && note.contains("verify"),
		"{note}"
	);

	let judged = &rig.chat.session.info.supervisor_accounting.plugin_usage["judge"];
	assert_eq!((judged.calls, judged.cost), (2, 0.5));
	assert_eq!(rig.chat.session.info.total_cost, 0.5);
	assert!(crate::plugins::take_notices(&id).is_empty(), "the notice was shown");
	assert!(crate::plugins::take_usage(&id).is_empty(), "the usage was booked");
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn a_plan_change_that_fails_is_reported_to_the_plugin_that_sent_it() {
	let mut rig = rig(vec![
		json!({"plan": {"action": "advance", "summary": "nothing is open"}}),
		json!({}),
	])
	.await;
	let id = rig.id.clone();
	crate::session::context::with_session_id(id.clone(), async {
		rig.answered_event().await;
		assert!(!plan::has_active_plan());
		rig.answered_event().await;
	})
	.await;

	let sent = rig.stub.sent(event::REQUEST_BEFORE);
	assert!(sent[0].get("plan_error").is_none());
	let error = sent[1]["plan_error"].as_str().expect("the plugin hears why");
	assert!(error.contains("No active plan"), "{error}");
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn a_revision_moves_the_start_of_the_current_phase() {
	let mut rig = rig(vec![
		json!({"plan": {"action": "start", "title": "Ship it", "tasks": tasks()}}),
		json!({"plan": {"action": "revise", "reason": "new route", "tasks": [{"title": "redo", "done_when": "it works"}]}}),
	])
	.await;
	let id = rig.id.clone();
	rig.chat.session.messages = vec![crate::session::Message::default(); 3];
	crate::session::context::with_session_id(id.clone(), async {
		rig.answered_event().await;
		rig.answered_event().await;
		assert_eq!(plan::get_current_task_start_index(), Some(3));
		assert!(plan::render_plan_checklist().unwrap().contains("redo"));
	})
	.await;
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn pinned_blocks_are_stored_with_the_session() {
	let mut rig = rig(vec![
		json!({"pin": [{"name": "goal", "text": "keep this", "items": ["one"]}]}),
		json!({"pin": [{"name": "goal", "text": "keep that"}]}),
	])
	.await;
	let id = rig.id.clone();

	rig.answered_event().await;
	let pins = &rig.chat.session.info.plugin_pins;
	assert_eq!(
		(pins.len(), pins[0].name.as_str(), pins[0].text.as_str()),
		(1, "goal", "keep this")
	);

	rig.answered_event().await;
	assert_eq!(rig.chat.session.info.plugin_pins[0].text, "keep that");
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn settle_without_a_session_shows_notices_and_keeps_the_usage() {
	let rig = rig(vec![json!({
		"notice": ["heard"],
		"usage": [{"label": "judge", "calls": 1, "input_tokens": 1, "output_tokens": 1, "cost": 0.25}],
	})])
	.await;
	rig.send().await;
	settle(&rig.id, None);
	assert_eq!(crate::plugins::take_usage(&rig.id).len(), 1);
	assert!(crate::plugins::take_notices(&rig.id).is_empty());
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn an_answer_without_effects_changes_nothing() {
	let mut rig = rig(vec![json!({})]).await;
	let id = rig.id.clone();
	rig.answered_event().await;
	assert!(rig.chat.plugin_notes.is_empty());
	assert!(rig.chat.session.info.plugin_pins.is_empty());
	assert!(rig.chat.session.info.supervisor_accounting.plugin_usage.is_empty());
	crate::plugins::end_session(&id);
}

#[test]
fn queued_notes_reach_the_history_in_order_and_dropped_notes_do_not() {
	let mut chat = ChatSession::for_tests(Vec::new());
	chat.queue_plugin_notes(vec!["first".into(), "second".into()]);
	chat.queue_plugin_notes(vec!["third".into()]);
	chat.deliver_plugin_notes().unwrap();
	assert!(chat.plugin_notes.is_empty());
	let written: Vec<&str> = chat.session.messages.iter().map(|m| m.content.as_str()).collect();
	assert_eq!(written.len(), 3);
	assert!(written[0].contains("first") && written[1].contains("second") && written[2].contains("third"));
	assert!(
		written.iter().all(|text| text.starts_with("<system-note>")),
		"{written:?}"
	);

	chat.queue_plugin_notes(vec!["never written".into()]);
	chat.drop_plugin_notes();
	chat.deliver_plugin_notes().unwrap();
	assert_eq!(chat.session.messages.len(), 3);
}

#[test]
fn a_new_user_turn_resets_the_plugin_state_of_the_turn() {
	let mut chat = ChatSession::for_tests(Vec::new());
	chat.plugin_continuations = 3;
	chat.turn_mutated_paths.insert("/work/a.rs".into());
	chat.plugin_verified = Some(true);
	chat.reset_plugin_turn_state();
	assert_eq!(chat.plugin_continuations, 0);
	assert!(chat.turn_mutated_paths.is_empty());
	assert_eq!(chat.plugin_verified, None);
	assert_eq!(chat.repeat_guard.identical_rounds(), 0);
}

#[tokio::test]
async fn usage_settled_without_a_session_is_booked_by_the_next_apply() {
	let mut rig = rig(vec![json!({
		"usage": [{"label": "judge", "calls": 1, "input_tokens": 4, "output_tokens": 1, "cost": 0.25}],
	})])
	.await;
	let id = rig.id.clone();
	let effects = rig.send().await;
	assert_eq!(effects, Effects::default());

	settle(&id, None);

	apply(&mut rig.chat, Effects::default()).await.unwrap();
	let judged = &rig.chat.session.info.supervisor_accounting.plugin_usage["judge"];
	assert_eq!((judged.calls, judged.cost), (1, 0.25));
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn a_report_is_booked_by_apply_and_survives_saving_and_resuming() {
	let mut rig = rig(vec![json!({
		"report": {"mechanics": {"check": {"chat_calls": 2, "outcome": {"kept_going": 1}}}, "totals": totals(2, 0.5)},
	})])
	.await;
	let id = rig.id.clone();
	let file = tempfile::Builder::new().suffix(".jsonl.zst").tempfile().unwrap();
	rig.chat.session.session_file = Some(file.path().to_path_buf());
	rig.answered_event().await;

	let booked = rig.chat.session.info.supervisor_accounting.plugin_reports.clone();
	assert_eq!(booked.len(), 1);
	assert_eq!(booked[rig.stub.name()].totals.chat_calls, 2);
	assert!(crate::plugins::take_reports(&id).is_empty(), "the report was booked");

	rig.chat.session.save().unwrap();
	let resumed = crate::session::load_session(&file.path().to_path_buf()).unwrap();
	assert_eq!(resumed.info.supervisor_accounting.plugin_reports, booked);
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn settle_without_a_session_keeps_the_report() {
	let rig = rig(vec![json!({"report": {"mechanics": {}, "totals": totals(0, 0.0)}})]).await;
	rig.send().await;
	settle(&rig.id, None);
	assert_eq!(crate::plugins::take_reports(&rig.id).len(), 1);
	crate::plugins::end_session(&rig.id);
}

#[tokio::test]
async fn a_result_note_is_queued_apart_from_a_note_and_both_get_the_plan_checklist() {
	let mut rig = rig(Vec::new()).await;
	let id = rig.id.clone();
	let effects = Effects {
		notes: vec!["Next:\n{plan_checklist}".into()],
		result_notes: vec!["Right after the results:\n{plan_checklist}".into()],
		plan: Some(PlanUpdate::Start {
			title: "Ship it".into(),
			tasks: vec![PlanTaskSpec {
				title: "implement".into(),
				done_when: "the code is written".into(),
			}],
		}),
	};

	crate::session::context::with_session_id(id.clone(), async {
		apply(&mut rig.chat, effects).await.unwrap();
	})
	.await;

	assert_eq!(rig.chat.plugin_notes.len(), 1);
	assert_eq!(rig.chat.plugin_result_notes.len(), 1);
	assert!(rig.chat.plugin_notes[0].starts_with("Next:\n"));
	assert!(rig.chat.plugin_result_notes[0].starts_with("Right after the results:\n"));
	for note in rig.chat.plugin_notes.iter().chain(&rig.chat.plugin_result_notes) {
		assert!(
			!note.contains(PLAN_CHECKLIST_PLACEHOLDER) && note.contains("implement"),
			"{note}"
		);
	}
	crate::plugins::end_session(&id);
}

#[test]
fn result_notes_are_written_alone_by_their_own_call() {
	let mut chat = ChatSession::for_tests(Vec::new());
	chat.queue_plugin_notes(vec!["a note".into()]);
	chat.queue_plugin_result_notes(vec!["a result note".into()]);

	chat.deliver_plugin_result_notes().unwrap();

	let written: Vec<&str> = chat.session.messages.iter().map(|m| m.content.as_str()).collect();
	assert_eq!(written.len(), 1);
	assert!(written[0].contains("a result note"));
	assert_eq!(chat.plugin_notes, vec!["a note".to_string()], "the note still waits");
	assert!(chat.plugin_result_notes.is_empty());
}

#[test]
fn result_notes_that_are_still_waiting_are_written_before_the_notes() {
	let mut chat = ChatSession::for_tests(Vec::new());
	chat.queue_plugin_notes(vec!["a note".into()]);
	chat.queue_plugin_result_notes(vec!["a result note".into()]);

	chat.deliver_plugin_notes().unwrap();

	let written: Vec<&str> = chat.session.messages.iter().map(|m| m.content.as_str()).collect();
	assert_eq!(written.len(), 2);
	assert!(
		written[0].contains("a result note") && written[1].contains("a note"),
		"{written:?}"
	);
}

#[tokio::test]
async fn the_time_the_agent_waited_is_booked_by_settle_once() {
	let mut rig = rig_with(StubPlugin::new(&[event::REQUEST_BEFORE]).delay(SLOW_ANSWER)).await;
	let id = rig.id.clone();

	rig.answered_event().await;

	let booked = rig.chat.session.info.supervisor_accounting.blocking_wall_time_ms;
	assert!(booked >= SLOW_ANSWER.as_millis() as u64, "{booked}");
	settle(&id, Some(&mut rig.chat.session));
	assert_eq!(
		rig.chat.session.info.supervisor_accounting.blocking_wall_time_ms,
		booked
	);
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn a_turn_counts_the_time_waited_even_when_nothing_settled_it_before_the_turn_ended() {
	let mut rig = rig_with(StubPlugin::new(&[event::REQUEST_BEFORE]).delay(SLOW_ANSWER)).await;
	let id = rig.id.clone();

	rig.chat.begin_turn_timing();
	rig.send().await;
	rig.chat.finish_turn_timing();

	let turn = &rig.chat.session.info.supervisor_accounting.turn_time;
	assert_eq!(turn.completed, 1);
	assert!(
		turn.blocking_supervisor_time_ms >= SLOW_ANSWER.as_millis() as u64,
		"{turn:?}"
	);
	crate::plugins::end_session(&id);
}
