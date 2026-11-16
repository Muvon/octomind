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

//! Tool names are unique per test: the tool registries are process-wide.

use super::*;
use crate::mcp::core::plan::{ExecutionPlan, PlanStatus, TaskStatus};
use crate::mcp::get_thread_working_directory;
use crate::mcp::tool_shape::{register_tool_command_shape, register_tool_read_only_hint};
use crate::session::self_report::{ParsedSelfReport, SelfReport};
use serde_json::{json, Value};

const STATUS_LINE: &str = r#"Done with the parser.
<sup>{"state":"progressing","focus":"fix the parser","next":"run its tests","carry":["fixtures/tabs.md"],"plan":null,"memories":[]}</sup>"#;

fn message(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

fn call(tool: &str, parameters: Value) -> McpToolCall {
	McpToolCall {
		tool_name: tool.to_string(),
		parameters,
		tool_id: format!("id-{tool}"),
	}
}

fn outcome(tool: &str, parameters: Value, workdir: &Path) -> ToolOutcome {
	let call = call(tool, parameters);
	let result = McpToolResult::success(tool.to_string(), call.tool_id.clone(), "ok".to_string());
	tool_outcome(&call, &result, workdir, false, false, None)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[test]
fn a_status_line_becomes_a_wire_status() {
	let status = status(STATUS_LINE).expect("a status");

	assert_eq!(status.state, "progressing");
	assert_eq!(status.focus.as_deref(), Some("fix the parser"));
	assert_eq!(status.next.as_deref(), Some("run its tests"));
	assert_eq!(status.carry, vec!["fixtures/tabs.md".to_string()]);
	assert!(status.fields.is_empty());
}

#[test]
fn a_message_without_a_status_line_has_no_status() {
	assert_eq!(status("Just prose, nothing to parse."), None);
}

#[test]
fn an_empty_next_action_is_left_out() {
	let done = r#"<sup>{"state":"done","focus":"all green","next":null,"carry":[],"plan":null,"memories":[]}</sup>"#;

	let status = status(done).expect("a status");

	assert_eq!(status.state, "done");
	assert_eq!(status.next, None);
	assert!(status.carry.is_empty());
}

#[test]
fn registered_status_fields_travel_with_the_status() {
	let mut report = self_report::parse_self_report_handoff(STATUS_LINE).expect("a report");
	report.fields.insert("confidence".to_string(), json!(0.8));
	report.fields.insert("risk".to_string(), json!("low"));

	let status = wire_status(report);

	assert_eq!(status.fields.len(), 2);
	assert_eq!(status.fields["confidence"], json!(0.8));
	assert_eq!(status.fields["risk"], json!("low"));
}

#[test]
fn every_state_keeps_its_wire_name() {
	for (state, name) in [
		(SelfReport::Exploring, "exploring"),
		(SelfReport::Progressing, "progressing"),
		(SelfReport::Blocked, "blocked"),
		(SelfReport::NeedInput, "need_input"),
		(SelfReport::Done, "done"),
	] {
		let report = ParsedSelfReport {
			state,
			handoff: Default::default(),
			used_memories: Vec::new(),
			fields: Default::default(),
		};
		assert_eq!(wire_status(report).state, name);
	}
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

fn unique_session(label: &str) -> String {
	format!("payload-{label}-{}", std::process::id())
}

/// `tasks` as `(title, description, valid_if)`.
fn plan_of(tasks: &[(&str, &str, Option<String>)], current: usize) -> ExecutionPlan {
	ExecutionPlan {
		title: "Ship the parser".to_string(),
		tasks: tasks
			.iter()
			.enumerate()
			.map(
				|(index, (title, description, valid_if))| crate::mcp::core::plan::PlanTask {
					title: title.to_string(),
					description: description.to_string(),
					details: String::new(),
					summary: None,
					status: if index < current {
						TaskStatus::Completed
					} else {
						TaskStatus::InProgress
					},
					completed_at: None,
					message_range: None,
					phase: None,
					valid_if: valid_if.clone(),
				},
			)
			.collect(),
		current_task_index: current,
		created_at: chrono::Utc::now(),
		status: PlanStatus::Active,
	}
}

#[tokio::test]
async fn without_a_plan_there_is_no_plan_state() {
	crate::session::context::with_session_id(unique_session("noplan"), async {
		assert_eq!(plan_state(), None);
	})
	.await;
}

#[tokio::test]
async fn an_open_plan_is_sent_with_its_progress() {
	let session = unique_session("open");
	crate::session::context::with_session_id(session.clone(), async {
		let plan = plan_of(
			&[
				("Read", "Done when: the parser is understood", None),
				("Fix", "Done when: tabs parse", None),
				("Verify", "tests pass", None),
			],
			1,
		);
		crate::session::context::get_plan_storage(&session)
			.lock()
			.unwrap()
			.load_plan(plan)
			.unwrap();
		crate::mcp::core::plan::set_current_task_start_index(7);

		let state = plan_state().expect("a plan");

		assert_eq!(state.title, "Ship the parser");
		assert_eq!(state.current, 1);
		assert_eq!(state.task_start_index, Some(7));
		assert!(state.touched_at > 0);
		let tasks: Vec<_> = state
			.tasks
			.iter()
			.map(|task| (task.title.as_str(), task.done_when.as_str(), task.done))
			.collect();
		assert_eq!(
			tasks,
			vec![
				("Read", "the parser is understood", true),
				("Fix", "tabs parse", false),
				// Without the prefix the description is the outcome as it stands.
				("Verify", "tests pass", false),
			]
		);
		assert!(state
			.tasks
			.iter()
			.all(|task| task.valid_if.is_none() && task.valid.is_none()));
	})
	.await;
}

#[tokio::test]
async fn a_task_whose_condition_broke_is_marked_invalid() {
	let session = unique_session("broken");
	let dir = tempfile::tempdir().unwrap();
	let present = dir.path().join("present.txt");
	std::fs::write(&present, "x").unwrap();
	let gone = dir.path().join("never-created.txt");
	crate::session::context::with_session_id(session.clone(), async {
		let plan = plan_of(
			&[
				("Now", "Done when: now", None),
				(
					"Holds",
					"Done when: holds",
					Some(format!("file_exists:{}", present.display())),
				),
				(
					"Broken",
					"Done when: broken",
					Some(format!("file_exists:{}", gone.display())),
				),
			],
			0,
		);
		crate::session::context::get_plan_storage(&session)
			.lock()
			.unwrap()
			.load_plan(plan)
			.unwrap();

		let state = plan_state().expect("a plan");

		assert_eq!(state.tasks[0].valid, None);
		assert_eq!(state.tasks[1].valid, Some(true));
		assert_eq!(state.tasks[2].valid, Some(false));
		assert_eq!(state.tasks[2].valid_if, Some(format!("file_exists:{}", gone.display())));
	})
	.await;
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[test]
fn a_shell_call_is_a_command_and_not_a_mutation() {
	register_tool_command_shape("payload_shell", true);
	register_tool_read_only_hint("payload_shell", Some(false));

	let outcome = outcome("payload_shell", json!({"command": "cargo test"}), Path::new("/work"));

	assert_eq!(outcome.command.as_deref(), Some("cargo test"));
	assert!(outcome.free_form);
	assert!(!outcome.mutation);
	assert!(outcome.paths.is_empty());
}

#[test]
fn a_file_edit_is_a_mutation_with_absolute_paths() {
	register_tool_command_shape("payload_edit", false);
	register_tool_read_only_hint("payload_edit", Some(false));

	let outcome = outcome(
		"payload_edit",
		json!({"command": "str_replace", "path": "src/a.rs"}),
		Path::new("/work"),
	);

	assert!(outcome.mutation);
	assert!(!outcome.free_form);
	assert_eq!(outcome.command, None);
	assert_eq!(outcome.paths, vec!["/work/src/a.rs".to_string()]);
}

#[test]
fn a_read_changes_nothing_and_keeps_an_absolute_path() {
	register_tool_read_only_hint("payload_read", Some(true));

	let outcome = outcome("payload_read", json!({"path": "/etc/hosts"}), Path::new("/work"));

	assert!(!outcome.mutation);
	assert!(!outcome.free_form);
	assert_eq!(outcome.command, None);
	assert_eq!(outcome.paths, vec!["/etc/hosts".to_string()]);
}

#[test]
fn an_outcome_carries_the_result_text_and_its_measures() {
	let call = call("payload_measure", json!({"path": "a"}));
	let result = McpToolResult::error(
		"payload_measure".to_string(),
		call.tool_id.clone(),
		"no such file é".to_string(),
	);

	let outcome = tool_outcome(&call, &result, Path::new("/work"), true, true, None);

	assert_eq!(outcome.id, "id-payload_measure");
	assert_eq!(outcome.tool, "payload_measure");
	assert_eq!(outcome.arguments, json!({"path": "a"}));
	assert!(outcome.error && outcome.denied && outcome.replaceable);
	assert_eq!(outcome.result.as_deref(), Some("no such file é"));
	assert_eq!(outcome.size_bytes, "no such file é".len());
	assert_eq!(outcome.size_tokens, crate::session::estimate_tokens("no such file é"));
	assert_eq!(outcome.result_hash, result_hash("no such file é"));
}

/// The outcome of a call whose result the agent was shown as `shown`, with what
/// the tool itself returned remembered as `returned`.
fn shown_outcome(name: &str, shown: McpToolResult, returned: Option<&Returned>) -> ToolOutcome {
	let call = call(name, json!({}));
	tool_outcome(&call, &shown, Path::new("/work"), false, false, returned)
}

fn returned(error: bool, text: &str) -> Returned {
	Returned {
		error,
		text: text.to_string(),
	}
}

#[test]
fn a_repeat_note_carries_what_the_tool_really_returned() {
	let real = McpToolResult::success(
		"payload_repeat".into(),
		"id-payload_repeat".into(),
		"real output".into(),
	);
	let remembered = returned_by_tool(&real, 8);
	assert_eq!(remembered, returned(false, "real out"));

	// The repeat note replaces the text and marks the result as an error.
	let note = McpToolResult::error(
		"payload_repeat".into(),
		"id-payload_repeat".into(),
		"Already seen".into(),
	);
	let outcome = shown_outcome("payload_repeat", note, Some(&remembered));

	assert!(outcome.error);
	assert_eq!(outcome.returned, Some(returned(false, "real out")));
}

#[test]
fn a_cut_that_keeps_the_start_carries_nothing_extra() {
	let shown = McpToolResult::success(
		"payload_cut".into(),
		"id-payload_cut".into(),
		"real output ... [cut]".into(),
	);
	let outcome = shown_outcome("payload_cut", shown, Some(&returned(false, "real out")));

	assert_eq!(outcome.returned, None);
}

#[test]
fn a_cut_that_does_not_keep_the_start_carries_what_the_tool_returned() {
	let shown = McpToolResult::success(
		"payload_tail".into(),
		"id-payload_tail".into(),
		"... [cut] end of the output".into(),
	);
	let outcome = shown_outcome("payload_tail", shown, Some(&returned(false, "real out")));

	assert_eq!(outcome.returned, Some(returned(false, "real out")));
}

#[test]
fn an_error_flag_that_changed_carries_what_the_tool_returned() {
	let shown = McpToolResult::error("payload_flag".into(), "id-payload_flag".into(), "real output".into());
	let outcome = shown_outcome("payload_flag", shown, Some(&returned(false, "real output")));

	assert_eq!(outcome.returned, Some(returned(false, "real output")));
}

#[test]
fn with_nothing_remembered_an_outcome_carries_no_returned_text() {
	let shown = McpToolResult::success("payload_none".into(), "id-payload_none".into(), "output".into());

	assert_eq!(shown_outcome("payload_none", shown, None).returned, None);
}

#[test]
fn a_call_before_it_ran_carries_what_the_caller_decided() {
	let call = call("payload_before", json!({"path": "a"}));

	let wire = tool_call(
		&call,
		Some("write".to_string()),
		Some("no".to_string()),
		vec!["r1".to_string()],
	);

	assert_eq!(wire.id, "id-payload_before");
	assert_eq!(wire.tool, "payload_before");
	assert_eq!(wire.capability.as_deref(), Some("write"));
	assert_eq!(wire.arguments, json!({"path": "a"}));
	assert_eq!(wire.denied.as_deref(), Some("no"));
	assert_eq!(wire.rules, vec!["r1".to_string()]);
}

#[test]
fn a_tool_function_becomes_a_tool_info_with_its_read_only_hint() {
	register_tool_read_only_hint("payload_info", Some(true));
	let function = McpFunction {
		name: "payload_info".to_string(),
		description: "Reads a thing".to_string(),
		parameters: json!({"type": "object"}),
		annotations: None,
	};

	let info = tool_info(&function);

	assert_eq!(info.name, "payload_info");
	assert_eq!(info.description, "Reads a thing");
	assert_eq!(info.parameters, json!({"type": "object"}));
	assert_eq!(info.read_only, Some(true));
}

// ---------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------

#[test]
fn a_result_hash_is_the_start_of_the_sha256_in_lowercase_hex() {
	// sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
	assert_eq!(result_hash("hello"), "2cf24dba5fb0a30e");
	assert_eq!(result_hash("hello").len(), RESULT_HASH_HEX_CHARS);
	assert_ne!(result_hash("hello"), result_hash("hello!"));
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_common_fields_are_read_from_the_session() {
	let chat_session = ChatSession::for_tests(vec![
		message("user", "fix it"),
		message("assistant", STATUS_LINE),
		message("user", "thanks"),
	]);

	let parts = ContextParts::of(&chat_session);

	assert_eq!(parts.session_id, "test");
	assert_eq!(parts.parent_session_id, None);
	assert_eq!(parts.role, "core");
	assert_eq!(parts.workdir, get_thread_working_directory());
	assert_eq!(parts.project, super::super::project::project_name(&parts.workdir));
	assert_eq!(
		parts.status.as_ref().map(|status| status.state.as_str()),
		Some("progressing")
	);
	assert_eq!(parts.plan, None);
	assert!(parts.active_skills.is_empty());
}

#[tokio::test]
async fn the_status_comes_from_the_latest_assistant_message() {
	let earlier =
		r#"<sup>{"state":"exploring","focus":"reading","next":"edit","carry":[],"plan":null,"memories":[]}</sup>"#;
	let chat_session = ChatSession::for_tests(vec![
		message("assistant", earlier),
		message("user", "go on"),
		message("assistant", "No status line here."),
	]);

	assert_eq!(ContextParts::of(&chat_session).status, None);
}

#[tokio::test]
async fn the_event_context_borrows_from_the_parts() {
	let chat_session = ChatSession::for_tests(vec![message("assistant", STATUS_LINE)]);
	let parts = ContextParts::of(&chat_session);
	let messages = chat_session.session.messages.clone();

	let context = parts.context(&messages, None);

	assert_eq!(context.session_id, "test");
	assert_eq!(context.role, "core");
	assert_eq!(context.messages.len(), 1);
	assert_eq!(context.status, parts.status);
	assert!(context.tools.is_none());
}
