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

use super::{result_status, trace};
use crate::session::Message;
use octolib::hindsight::trace::{event_text, Event, Op, Status};

fn message(role: &str, content: &str) -> Message {
	Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[test]
fn trace_opens_with_the_task_and_maps_every_role() {
	let mut assistant = message("assistant", "Reading the file first.");
	assistant.thinking = Some(serde_json::json!({"content": "plan: read then edit", "tokens": 5}));
	assistant.tool_calls = Some(serde_json::json!([
		{"id": "c1", "function": {"name": "view", "arguments": "{\"path\": \"src/a.rs\"}"}},
		{"id": "c2", "function": {"name": "shell", "arguments": {"command": "git status"}}},
		{"id": "c3", "function": {"name": "text_editor", "arguments": {"command": "str_replace", "path": "src/a.rs", "old_text": "a", "new_text": "b"}}}
	]));
	let messages = vec![
		message("system", "You are a developer."),
		message("user", "<system-note>\nprelude\n</system-note>"),
		message("user", "fix the failing test"),
		assistant,
		message("tool", "fn main() {}"),
		message("tool", "Error: no such file"),
		message(
			"user",
			"<pay-attention>\nverification found gaps\n</pay-attention>",
		),
		message("assistant", "Done."),
	];
	let events = trace(&messages);
	let kinds: Vec<&str> = events.iter().map(Event::kind).collect();
	assert_eq!(
		kinds,
		[
			"user_turn",
			"thinking",
			"assistant",
			"tool_call",
			"tool_call",
			"tool_call",
			"tool_result",
			"tool_result",
			"harness",
			"assistant"
		]
	);
	let Event::ToolCall { op, path, .. } = &events[3] else {
		panic!("view call")
	};
	assert_eq!(*op, Op::Read);
	assert_eq!(path.as_deref(), Some("src/a.rs"));
	let Event::ToolCall { op, .. } = &events[4] else {
		panic!("shell call")
	};
	assert_eq!(*op, Op::Git);
	let Event::ToolCall { op, args, .. } = &events[5] else {
		panic!("edit call")
	};
	assert_eq!(*op, Op::Edit);
	assert_eq!(args["mode"], serde_json::json!("str_replace"));
	assert!(args.get("command").is_none());
	assert!(event_text(&events[5], 1500).contains("\"new_text\": \"b\""));
	let Event::ToolResult { status, .. } = &events[7] else {
		panic!("error result")
	};
	assert_eq!(*status, Status::Error);
	assert!(event_text(&events[8], 1500).starts_with("HARNESS steer gate\n"));
}

#[test]
fn continuation_wrapper_becomes_the_task_after_compaction() {
	let wrapper = "<continuation>\nsummary\n<request>\nadd a test for clip\n</request>\n<task>\ncontinue\n</task>\n</continuation>";
	let events = trace(&[message("user", wrapper), message("assistant", "ok")]);
	let kinds: Vec<&str> = events.iter().map(Event::kind).collect();
	assert_eq!(kinds, ["user_turn", "harness", "assistant"]);
	assert_eq!(event_text(&events[0], 1500), "USER\nadd a test for clip");
}

#[test]
fn trace_without_a_user_turn_is_empty() {
	assert!(trace(&[message("system", "role"), message("assistant", "hi")]).is_empty());
}

#[test]
fn result_status_reads_the_text_head() {
	assert_eq!(result_status("ok"), Status::Ok);
	assert_eq!(result_status("  Error: boom"), Status::Error);
	assert_eq!(result_status("Blocked by guardrail"), Status::Denied);
	assert_eq!(
		result_status("[Request interrupted by user]"),
		Status::Interrupted
	);
}
