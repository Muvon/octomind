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
use serde_json::json;

fn request() -> RunLogMessage {
	RunLogMessage {
		session_id: "parent".into(),
		run_id: "tap-developer-cto-abcdef".into(),
		request_id: Some("replay".into()),
	}
}

#[test]
fn live_snapshot_coalesces_chunks_without_losing_sequence_and_resume_keeps_sequence() {
	let request = request();
	let mut events = RunEvents::default();
	events.begin(&request.session_id, &request.run_id, "developer:cto");
	for content in ["one", "two"] {
		events.emit(
			&request.session_id,
			&request.run_id,
			RunEvent::Assistant {
				content: content.into(),
			},
		);
	}
	for content in ["think", "more"] {
		events.emit(
			&request.session_id,
			&request.run_id,
			RunEvent::Thinking {
				content: content.into(),
			},
		);
	}
	let snapshot = events.snapshot(&request).unwrap();
	assert_eq!(snapshot.seq, 5);
	assert_eq!(snapshot.events.len(), 3);
	assert_eq!(
		snapshot.events[1],
		RunEvent::Assistant {
			content: "onetwo".into()
		}
	);
	assert_eq!(
		snapshot.events[2],
		RunEvent::Thinking {
			content: "thinkmore".into()
		}
	);
	events.emit(
		&request.session_id,
		&request.run_id,
		RunEvent::Assistant { content: "next".into() },
	);
	assert_eq!(events.snapshot(&request).unwrap().seq, snapshot.seq + 1);
	for status in [TapJobStatus::Done, TapJobStatus::Failed, TapJobStatus::Cancelled] {
		let before = events.last_seq(&request.run_id);
		events.emit(
			&request.session_id,
			&request.run_id,
			RunEvent::Status { status, error: None },
		);
		assert!(events.snapshot(&request).is_none(), "terminal buffer is freed");
		events.begin(&request.session_id, &request.run_id, "developer:cto");
		assert_eq!(events.snapshot(&request).unwrap().seq, before + 2);
		assert_eq!(events.snapshot(&request).unwrap().events.len(), 1);
	}
}

#[test]
fn buffer_caps_content_and_drops_oldest_even_for_one_oversized_chunk() {
	let mut buffer = EventBuffer::default();
	buffer.push(RunEvent::Prompt { content: "old".into() });
	buffer.push(RunEvent::Assistant {
		content: "a".repeat(MAX_RUN_CONTENT),
	});
	assert!(buffer.truncated);
	assert_eq!(buffer.bytes, MAX_RUN_CONTENT);
	assert_eq!(buffer.events.len(), 1);
	buffer.push(RunEvent::Assistant { content: "b".into() });
	assert_eq!(buffer.bytes, 0);
	assert!(buffer.events.is_empty());
	buffer.push(RunEvent::Thinking {
		content: "latest".into(),
	});
	assert_eq!(buffer.bytes, 6);
}

#[test]
fn disk_replay_uses_real_session_writer_and_preserves_tool_pairing() {
	let dir = tempfile::tempdir().unwrap();
	let id = "tap-developer-cto-abcdef";
	let path = dir.path().join(format!("{id}.jsonl.zst"));
	let mut session = crate::session::Session::new(id.into(), "ollama:fake-model".into());
	session.session_file = Some(path.clone());
	session.save().unwrap();
	let messages = [
		crate::session::Message {
			role: "system".into(),
			content: "skip".into(),
			..Default::default()
		},
		crate::session::Message {
			role: "user".into(),
			content: "task".into(),
			..Default::default()
		},
		crate::session::Message {
			role: "assistant".into(),
			content: "working".into(),
			thinking: Some(json!({"content": "reason"})),
			// What message_handler actually persists: octolib's unified GenericToolCall
			// JSON. The OpenAI `function` shape here once hid a replay that failed on
			// every real log (dev, 2026-10-20).
			tool_calls: Some(json!([{"id": "call-1", "name": "read", "arguments": {"path": "file.rs"}}])),
			..Default::default()
		},
		crate::session::Message {
			role: "tool".into(),
			content: "file content".into(),
			tool_call_id: Some("call-1".into()),
			..Default::default()
		},
	];
	for message in messages {
		crate::session::append_to_session_file(&path, &serde_json::to_string(&message).unwrap()).unwrap();
	}
	let events = disk_events(dir.path(), id).unwrap();
	assert_eq!(
		serde_json::to_value(events).unwrap(),
		json!([
			{"kind":"prompt", "content":"task"},
			{"kind":"thinking", "content":"reason"},
			{"kind":"assistant", "content":"working"},
			{"kind":"tool_use", "tool":"read", "tool_id":"call-1", "params":{"path":"file.rs"}},
			{"kind":"tool_result", "tool_id":"call-1", "content":"file content"}
		])
	);
	assert_eq!(
		disk_events(dir.path(), "tap-missing-000000").unwrap_err().to_string(),
		"run not found: tap-missing-000000"
	);
}

#[test]
fn acp_events_preserve_terminal_results_and_usage() {
	for (status, success) in [("completed", true), ("failed", false)] {
		let events = acp_events(&json!({"params":{"update":{"sessionUpdate":"tool_call_update", "toolCallId":"call", "status":status, "rawOutput":{"ok":success}}}})).unwrap();
		assert_eq!(
			events,
			vec![RunEvent::ToolResult {
				tool_id: "call".into(),
				content: json!({"ok":success}).to_string(),
				success: Some(success)
			}]
		);
	}
	assert!(acp_events(
		&json!({"params":{"update":{"sessionUpdate":"tool_call_update", "toolCallId":"call", "status":"in_progress"}}})
	)
	.unwrap()
	.is_empty());
	let events = acp_events(&json!({"params":{"_meta":{"octomind.usage":{"session_cost":0.12,"input_tokens":12,"output_tokens":3}}, "update":{"sessionUpdate":"session_info_update"}}})).unwrap();
	assert_eq!(
		events,
		vec![RunEvent::Usage {
			cost: 0.12,
			input_tokens: 12,
			output_tokens: 3
		}]
	);
	assert!(acp_events(&json!({"params":{"_meta":{"octomind.usage":{}}}})).is_err());
}

#[test]
fn run_id_validation_matches_generator_and_rejects_paths_and_package_names() {
	assert!(tap_runs::validate_run_id(&tap_runs::generate_id("developer:cto")).is_ok());
	for id in [
		"",
		"owner/repo",
		"../tap-x-abcdef",
		"tap-x-abcdef/",
		"tap-x-abcdef.jsonl.zst",
		"tap-x-abcde",
		"tap-x-zzzzzz",
		"tap--abcdef",
	] {
		assert!(tap_runs::validate_run_id(id).is_err(), "{id}");
	}
}
