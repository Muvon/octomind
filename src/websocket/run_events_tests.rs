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
		summary: false,
	}
}

#[test]
fn live_snapshot_coalesces_chunks_without_losing_sequence_and_resume_keeps_sequence() {
	let request = request();
	let mut events = RunEvents::default();
	events
		.begin(&request.session_id, &request.run_id, "developer:cto")
		.unwrap();
	for content in ["one", "two"] {
		events
			.emit(
				&request.session_id,
				&request.run_id,
				RunEvent::Assistant {
					content: content.into(),
				},
			)
			.unwrap();
	}
	for content in ["think", "more"] {
		events
			.emit(
				&request.session_id,
				&request.run_id,
				RunEvent::Thinking {
					content: content.into(),
				},
			)
			.unwrap();
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
	events
		.emit(
			&request.session_id,
			&request.run_id,
			RunEvent::Assistant { content: "next".into() },
		)
		.unwrap();
	assert_eq!(events.snapshot(&request).unwrap().seq, snapshot.seq + 1);
	for status in [TapJobStatus::Done, TapJobStatus::Failed, TapJobStatus::Cancelled] {
		let before = events.last_seq(&request.run_id);
		events
			.emit(
				&request.session_id,
				&request.run_id,
				RunEvent::Status { status, error: None },
			)
			.unwrap();
		assert!(events.snapshot(&request).is_none(), "terminal buffer is freed");
		events
			.begin(&request.session_id, &request.run_id, "developer:cto")
			.unwrap();
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

fn usage(cost: f64, input_tokens: u64, output_tokens: u64) -> RunEvent {
	RunEvent::Usage {
		cost,
		input_tokens,
		output_tokens,
	}
}

#[test]
fn live_summary_returns_only_latest_usage_even_after_truncation() {
	let request = request();
	let summary = RunLogMessage {
		summary: true,
		..request()
	};
	let mut events = RunEvents::default();
	events
		.begin(&request.session_id, &request.run_id, "developer:cto")
		.unwrap();
	let reply = events.snapshot(&summary).unwrap();
	assert!(reply.events.is_empty(), "no usage reported yet");
	for event in [
		usage(0.1, 10, 2),
		RunEvent::Assistant { content: "a".into() },
		usage(0.2, 20, 4),
		RunEvent::Assistant {
			content: "b".repeat(MAX_RUN_CONTENT),
		},
	] {
		events.emit(&request.session_id, &request.run_id, event).unwrap();
	}
	let full = events.snapshot(&request).unwrap();
	assert!(full.truncated);
	assert!(!full.events.iter().any(|event| matches!(event, RunEvent::Usage { .. })));
	let reply = events.snapshot(&summary).unwrap();
	assert_eq!(
		(reply.status.as_str(), reply.source.as_str(), reply.seq),
		("running", "live", 5)
	);
	assert_eq!(reply.role.as_deref(), Some("developer:cto"));
	assert_eq!(reply.request_id.as_deref(), Some("replay"));
	assert!(!reply.truncated);
	assert_eq!(reply.events, vec![usage(0.2, 20, 4)]);
}

#[test]
fn run_log_summary_is_optional_on_the_wire() {
	let request: RunLogMessage =
		serde_json::from_value(json!({"session_id":"parent", "run_id":"tap-developer-cto-abcdef"})).unwrap();
	assert!(!request.summary);
	assert!(serde_json::to_value(&request).unwrap().get("summary").is_none());
	let request: RunLogMessage =
		serde_json::from_value(json!({"session_id":"parent", "run_id":"tap-developer-cto-abcdef", "summary":true}))
			.unwrap();
	assert!(request.summary);
}

/// Records totals the way a real run does: SUMMARY on save, then a later STATS checkpoint.
fn record_stats(path: &Path, session: &mut crate::session::Session) {
	session.info.total_cost = 0.5;
	session.info.input_tokens = 100;
	session.info.output_tokens = 20;
	session.save().unwrap();
	// STATS older than the last SUMMARY are ignored on load; a far-future one always applies.
	crate::session::append_to_session_file(
		&path.to_path_buf(),
		&json!({"type":"STATS", "timestamp": 4_000_000_000u64, "total_cost":0.75, "input_tokens":150, "output_tokens":30})
			.to_string(),
	)
	.unwrap();
}

#[test]
fn disk_replay_appends_recorded_session_totals_as_final_usage() {
	let dir = tempfile::tempdir().unwrap();
	let id = "tap-developer-cto-abcdef";
	let path = dir.path().join(format!("{id}.jsonl.zst"));
	let mut session = crate::session::Session::new(id.into(), "ollama:fake-model".into());
	session.session_file = Some(path.clone());
	session.save().unwrap();
	crate::session::append_to_session_file(
		&path,
		&serde_json::to_string(&crate::session::Message {
			role: "user".into(),
			content: "task".into(),
			..Default::default()
		})
		.unwrap(),
	)
	.unwrap();
	record_stats(&path, &mut session);
	assert_eq!(
		disk_events(dir.path(), id).unwrap(),
		vec![RunEvent::Prompt { content: "task".into() }, usage(0.75, 150, 30)]
	);
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
		// AGENTS.md as the child seeds it: system-managed, never the task.
		crate::session::Message {
			role: "user".into(),
			content: "<instructions>\n# Octomind Cloud machine\n</instructions>".into(),
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
	// The session never recorded stats (all-zero totals), so no usage event is claimed.
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

fn prepare_finished_replay_run(request: &RunLogMessage, dir: &Path) -> std::path::PathBuf {
	use crate::session::tap_runs::{TapJob, TapLiveState};
	use std::sync::{Arc, RwLock};
	tap_runs::init_for_session();
	tap_runs::enable_run_events(None).unwrap();
	let (cancel_tx, _) = tokio::sync::watch::channel(false);
	tap_runs::register_job(TapJob {
		id: request.run_id.clone(),
		role: "developer:cto".into(),
		workdir: dir.to_string_lossy().into_owned(),
		started_at: std::time::SystemTime::now(),
		status: Arc::new(RwLock::new(TapJobStatus::Done)),
		cancel_tx,
		live: Arc::new(RwLock::new(TapLiveState::default())),
	});
	let path = dir.join(format!("{}.jsonl.zst", request.run_id));
	let mut session = crate::session::Session::new(request.run_id.clone(), "ollama:fake-model".into());
	session.session_file = Some(path.clone());
	session.save().unwrap();
	crate::session::append_to_session_file(
		&path,
		&serde_json::to_string(&crate::session::Message {
			role: "user".into(),
			content: "old task".into(),
			..Default::default()
		})
		.unwrap(),
	)
	.unwrap();
	path
}

#[tokio::test]
async fn disk_replay_switches_to_live_when_resumed_during_read() {
	let request = RunLogMessage {
		session_id: "replay-resume-during-read".into(),
		..request()
	};
	let dir = tempfile::tempdir().unwrap();
	crate::session::context::with_session_id(request.session_id.clone(), async {
		let path = prepare_finished_replay_run(&request, dir.path());
		let captured = tap_runs::snapshot_run_log(&request).unwrap();
		assert_eq!(captured.source, "disk");
		let reply = replay_disk(&request, captured, || {
			let _claim = tap_runs::resume_run(&request.run_id)?;
			crate::session::append_to_session_file(
				&path,
				&serde_json::to_string(&crate::session::Message {
					role: "user".into(),
					content: "resumed task".into(),
					..Default::default()
				})?,
			)?;
			tap_runs::with_run_events(|events, parent| {
				events.emit(
					parent,
					&request.run_id,
					RunEvent::Prompt {
						content: "resumed task".into(),
					},
				)
			})?;
			disk_events(dir.path(), &request.run_id)
		})
		.unwrap();
		assert_eq!(reply.source, "live");
		assert_eq!(reply.status, "running");
		assert_eq!(reply.seq, 2);
		assert_eq!(
			reply.events,
			vec![
				RunEvent::Status {
					status: TapJobStatus::Running,
					error: None
				},
				RunEvent::Prompt {
					content: "resumed task".into()
				},
			]
		);
	})
	.await;
	tap_runs::clear_for_session(&request.session_id);
}

#[tokio::test]
async fn disk_replay_retries_when_resumed_turn_finishes_during_read() {
	let request = RunLogMessage {
		session_id: "replay-resume-finished-during-read".into(),
		..request()
	};
	let dir = tempfile::tempdir().unwrap();
	crate::session::context::with_session_id(request.session_id.clone(), async {
		let path = prepare_finished_replay_run(&request, dir.path());
		let captured = tap_runs::snapshot_run_log(&request).unwrap();
		let mut reads = 0;
		let reply = replay_disk(&request, captured, || {
			reads += 1;
			if reads == 1 {
				let claim = tap_runs::resume_run(&request.run_id)?;
				crate::session::append_to_session_file(
					&path,
					&serde_json::to_string(&crate::session::Message {
						role: "user".into(),
						content: "resumed task".into(),
						..Default::default()
					})?,
				)?;
				tap_runs::finish_run(&request.run_id, &claim.status, TapJobStatus::Done, None)?;
			}
			disk_events(dir.path(), &request.run_id)
		})
		.unwrap();
		assert_eq!(reads, 2);
		assert_eq!(reply.source, "disk");
		assert_eq!(reply.status, "done");
		assert_eq!(reply.seq, 2);
		assert_eq!(
			reply.events,
			vec![
				RunEvent::Prompt {
					content: "old task".into()
				},
				RunEvent::Prompt {
					content: "resumed task".into()
				},
			]
		);
	})
	.await;
	tap_runs::clear_for_session(&request.session_id);
}

#[tokio::test]
async fn disk_summary_returns_only_session_totals_with_status_intact() {
	let request = RunLogMessage {
		session_id: "replay-disk-summary".into(),
		summary: true,
		..request()
	};
	let dir = tempfile::tempdir().unwrap();
	crate::session::context::with_session_id(request.session_id.clone(), async {
		let path = prepare_finished_replay_run(&request, dir.path());
		let captured = tap_runs::snapshot_run_log(&request).unwrap();
		let empty = replay_disk(&request, captured.clone(), || disk_events(dir.path(), &request.run_id)).unwrap();
		assert!(empty.events.is_empty(), "no recorded stats, no usage");
		let mut session = crate::session::Session::new(request.run_id.clone(), "ollama:fake-model".into());
		session.session_file = Some(path.clone());
		record_stats(&path, &mut session);
		let reply = replay_disk(&request, captured, || disk_events(dir.path(), &request.run_id)).unwrap();
		assert_eq!(
			(reply.status.as_str(), reply.source.as_str(), reply.seq),
			("done", "disk", 0)
		);
		assert_eq!(reply.role.as_deref(), Some("developer:cto"));
		assert!(!reply.truncated);
		assert_eq!(reply.events, vec![usage(0.75, 150, 30)]);
		let full = replay_disk(
			&RunLogMessage {
				summary: false,
				..request.clone()
			},
			reply,
			|| disk_events(dir.path(), &request.run_id),
		)
		.unwrap();
		assert_eq!(
			full.events,
			vec![
				RunEvent::Prompt {
					content: "old task".into()
				},
				usage(0.75, 150, 30)
			]
		);
	})
	.await;
	tap_runs::clear_for_session(&request.session_id);
}

#[test]
fn run_events_reject_duplicate_begin_and_late_emission_without_panicking() {
	let request = request();
	let mut events = RunEvents::default();
	let text = RunEvent::Assistant { content: "late".into() };
	assert!(events.emit(&request.session_id, &request.run_id, text.clone()).is_err());
	events
		.begin(&request.session_id, &request.run_id, "developer:cto")
		.unwrap();
	assert!(events
		.begin(&request.session_id, &request.run_id, "developer:cto")
		.is_err());
	assert_eq!(events.snapshot(&request).unwrap().seq, 1);
	events
		.emit(
			&request.session_id,
			&request.run_id,
			RunEvent::Status {
				status: TapJobStatus::Done,
				error: None,
			},
		)
		.unwrap();
	assert!(events.emit(&request.session_id, &request.run_id, text).is_err());
	assert_eq!(events.last_seq(&request.run_id), 2);
}
