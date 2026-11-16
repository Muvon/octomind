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
use std::collections::HashSet;

fn sample_event() -> Event<PromptSubmit> {
	Event {
		session_id: "s1".to_string(),
		parent_session_id: None,
		role: "developer:general".to_string(),
		workdir: "/work".to_string(),
		project: "work".to_string(),
		git_remote: None,
		messages: vec![Message {
			role: "user".to_string(),
			content: "hi".to_string(),
			timestamp: 7,
			managed: false,
			id: None,
			name: None,
			tool_call_id: None,
			tool_calls: None,
			thinking: None,
		}],
		messages_reset: false,
		status: None,
		rule_matches: vec![RuleMatch {
			id: "r1".to_string(),
			outcome: RuleOutcome::Observed,
		}],
		plan: None,
		plan_error: None,
		tools: None,
		active_skills: Vec::new(),
		dropped_context: Vec::new(),
		files: Vec::new(),
		payload: PromptSubmit {
			prompt: "do it".to_string(),
			original: None,
			carried_summary: None,
			goal: None,
		},
	}
}

#[test]
fn event_flattens_its_payload_next_to_the_common_fields() {
	let value = serde_json::to_value(sample_event()).unwrap();
	assert_eq!(value["session_id"], "s1");
	assert_eq!(value["prompt"], "do it");
	assert_eq!(value["rule_matches"][0]["outcome"], "observed");
	assert!(value.get("payload").is_none());
	assert!(value.get("parent_session_id").is_none());
	assert!(value.get("plan").is_none());
	assert_eq!(value["project"], "work");
	for absent in ["plan_error", "tools", "active_skills", "dropped_context", "goal"] {
		assert!(value.get(absent).is_none(), "{absent}");
	}
}

#[test]
fn event_round_trips() {
	let value = serde_json::to_value(sample_event()).unwrap();
	let back: Event<PromptSubmit> = serde_json::from_value(value.clone()).unwrap();
	assert_eq!(back.payload.prompt, "do it");
	assert_eq!(back.messages.len(), 1);
	assert_eq!(back.messages[0].timestamp, 7);
	assert_eq!(serde_json::to_value(back).unwrap(), value);
}

/// An event with every common field set, so a payload field named like one of
/// them shows up as a failed round trip.
fn full_event<P>(payload: P) -> Event<P> {
	Event {
		parent_session_id: Some("parent".into()),
		git_remote: Some("git@example.com:x/y.git".into()),
		messages_reset: true,
		status: Some(Status {
			state: "done".into(),
			focus: Some("f".into()),
			next: Some("n".into()),
			carry: vec!["c".into()],
			fields: BTreeMap::from([("k".to_string(), json!(1))]),
		}),
		plan: Some(PlanState {
			title: "t".into(),
			tasks: vec![PlanTask {
				title: "a".into(),
				done_when: "b".into(),
				done: false,
				valid_if: Some("file_exists:x".into()),
				valid: Some(true),
			}],
			current: 0,
			touched_at: 5,
			task_start_index: Some(2),
		}),
		plan_error: Some("why".into()),
		tools: Some(vec![tool_info()]),
		active_skills: vec!["skill".into()],
		dropped_context: vec!["block".into()],
		files: vec![FileText {
			path: "docs/api.md".into(),
			hash: "ab12".into(),
			text: Some("# API".into()),
			skipped: None,
		}],
		payload,
		session_id: "s1".into(),
		role: "developer".into(),
		workdir: "/work".into(),
		project: "work".into(),
		messages: vec![Message {
			role: "assistant".into(),
			content: "hi".into(),
			timestamp: 7,
			managed: true,
			id: Some("m1".into()),
			name: Some("n".into()),
			tool_call_id: Some("c1".into()),
			tool_calls: Some(json!([])),
			thinking: Some("t".into()),
		}],
		rule_matches: vec![RuleMatch {
			id: "r".into(),
			outcome: RuleOutcome::Applied,
		}],
	}
}

fn tool_info() -> ToolInfo {
	ToolInfo {
		name: "shell".into(),
		server: "core".into(),
		description: "d".into(),
		parameters: json!({"type": "object"}),
		read_only: Some(true),
	}
}

fn background() -> Vec<BackgroundOutput> {
	vec![BackgroundOutput {
		call_id: "c".into(),
		label: "l".into(),
		output: "o".into(),
	}]
}

fn assert_round_trip<P>(payload: P)
where
	P: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
	let event = full_event(payload);
	let text = serde_json::to_string(&event).unwrap();
	let back: Event<P> = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"));
	assert_eq!(back, event, "{text}");
}

#[test]
fn every_payload_round_trips_inside_an_event_with_every_common_field_set() {
	assert_round_trip(SessionStart {
		source: "startup".into(),
		system_prompt: "s".into(),
		interactive: true,
		can_read_saved_output: true,
		inherited: Some(json!({"x": 1})),
		capabilities: vec!["cap".into()],
		context_files: vec!["/AGENTS.md".into()],
	});
	assert_round_trip(PromptSubmit {
		prompt: "p".into(),
		original: Some("o".into()),
		carried_summary: Some("c".into()),
		goal: Some("g".into()),
	});
	assert_round_trip(RequestBefore {
		phase: RequestPhase::FollowUp,
		turn: TurnKind::System,
		free_context_tokens: Some(10),
		pending_async: true,
	});
	assert_round_trip(ToolBefore {
		calls: vec![ToolCall {
			id: "c".into(),
			tool: "t".into(),
			server: "s".into(),
			capability: Some("cap".into()),
			arguments: json!({"a": 1}),
			denied: Some("no".into()),
			rules: vec!["r".into()],
		}],
	});
	assert_round_trip(ToolAfter {
		intent: Some("i".into()),
		calls: vec![ToolOutcome {
			id: "c".into(),
			tool: "t".into(),
			server: "s".into(),
			arguments: json!({}),
			error: true,
			denied: true,
			result: Some("r".into()),
			result_hash: "h".into(),
			size_tokens: 3,
			size_bytes: 12,
			mutation: true,
			command: Some("ls".into()),
			free_form: true,
			paths: vec!["/p".into()],
			replaceable: true,
		}],
		tree: Some(TreeChange {
			before: "a".into(),
			after: "b".into(),
		}),
		subagents: Some(SubagentRuns {
			finished: 2,
			verified: 1,
		}),
		background: background(),
		stopping: Some("loop".into()),
		context_files: vec!["/AGENTS.md".into()],
	});
	assert_round_trip(TurnEnd {
		turn: TurnKind::User,
		answers: vec!["a".into()],
		pending_async: true,
		changes: vec![FileChange {
			path: "/p".into(),
			exists: true,
			diff: Some("d".into()),
			head: Some("h".into()),
			truncated: true,
		}],
		tree_status: vec![" M x".into()],
		tree: Some("fp".into()),
		requests: 4,
		stopped: Some("loop".into()),
		background: background(),
	});
	assert_round_trip(CompactBefore { forced: true });
	assert_round_trip(CompactAfter {
		carried_summary: Some("s".into()),
		goal: Some("g".into()),
	});
	assert_round_trip(SubagentStart {
		child_session_id: Some("child".into()),
		prompt: "p".into(),
		system_prompt: Some("s".into()),
	});
	assert_round_trip(SubagentEnd {
		child_session_id: Some("child".into()),
	});
	assert_round_trip(Command {
		name: "learn".into(),
		arguments: "now".into(),
	});
	assert_round_trip(StatusRequest {});
	assert_round_trip(SessionEnd { reason: "exit".into() });
}

#[test]
fn session_start_carries_the_tool_list_once_in_the_event() {
	let event = full_event(SessionStart {
		source: "startup".into(),
		system_prompt: "s".into(),
		interactive: false,
		can_read_saved_output: false,
		inherited: None,
		capabilities: Vec::new(),
		context_files: Vec::new(),
	});
	let text = serde_json::to_string(&event).unwrap();
	assert_eq!(text.matches("\"tools\":").count(), 1, "{text}");
}

#[test]
fn event_accepts_a_minimal_object_from_a_plugin_author() {
	let back: Event<PromptSubmit> = serde_json::from_value(json!({
		"session_id": "s", "role": "r", "workdir": "/w", "project": "w", "prompt": "p"
	}))
	.unwrap();
	assert!(back.messages.is_empty());
	assert!(!back.messages_reset);
	assert!(back.rule_matches.is_empty());
}

#[test]
fn an_empty_answer_is_an_empty_object() {
	assert_eq!(serde_json::to_string(&Answer::default()).unwrap(), "{}");
	let back: Answer = serde_json::from_str("{}").unwrap();
	assert_eq!(back, Answer::default());
}

#[test]
fn continue_is_the_wire_name_of_continue_with() {
	let answer = Answer {
		continue_with: Some("go on".to_string()),
		..Answer::default()
	};
	assert_eq!(serde_json::to_value(&answer).unwrap(), json!({"continue": "go on"}));
	let back: Answer = serde_json::from_value(json!({"continue": "again"})).unwrap();
	assert_eq!(back.continue_with.as_deref(), Some("again"));
}

#[test]
fn every_answer_field_is_named_like_its_effect() {
	let full = Answer {
		context: vec![Block {
			name: "a".into(),
			text: None,
			keep: false,
			items: Vec::new(),
		}],
		pin: vec![Block {
			name: "a".into(),
			text: None,
			keep: false,
			items: Vec::new(),
		}],
		usage: vec![Usage::default()],
		report: Some(Report::default()),
		notice: vec!["n".into()],
		register: Some(Register::default()),
		history: true,
		read: vec!["docs/*.md".into()],
		plan: Some(PlanUpdate::Finish { summary: "s".into() }),
		replace_prompt: Some("p".into()),
		block: Some("b".into()),
		deny: vec![Deny {
			call_id: "c".into(),
			reason: "r".into(),
		}],
		lift: vec![Lift {
			call_id: "c".into(),
			rules: vec![],
		}],
		replace: vec![Replace {
			call_id: "c".into(),
			text: "t".into(),
			keep_original: false,
		}],
		note: vec!["n".into()],
		stop: Some("s".into()),
		continue_with: Some("c".into()),
		inherit: Some(json!(1)),
		output: Some(CommandOutput::default()),
		rows: vec![Row {
			label: "l".into(),
			value: "v".into(),
		}],
		verified: Some(true),
	};
	let value = serde_json::to_value(full).unwrap();
	let fields: HashSet<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
	let effects = [
		effect::CONTEXT,
		effect::PIN,
		effect::USAGE,
		effect::REPORT,
		effect::NOTICE,
		effect::REGISTER,
		effect::HISTORY,
		effect::READ,
		effect::PLAN,
		effect::REPLACE_PROMPT,
		effect::BLOCK,
		effect::DENY,
		effect::LIFT,
		effect::REPLACE,
		effect::NOTE,
		effect::STOP,
		effect::CONTINUE,
		effect::INHERIT,
		effect::OUTPUT,
		effect::ROWS,
		effect::VERIFIED,
	];
	assert_eq!(fields, effects.into_iter().collect::<HashSet<_>>());
}

#[test]
fn plan_update_is_tagged_by_action() {
	let start = PlanUpdate::Start {
		title: "t".into(),
		tasks: vec![PlanTaskSpec {
			title: "a".into(),
			done_when: "b".into(),
		}],
	};
	let value = serde_json::to_value(&start).unwrap();
	assert_eq!(value["action"], "start");
	assert_eq!(value["tasks"][0]["done_when"], "b");
	assert_eq!(serde_json::from_value::<PlanUpdate>(value).unwrap(), start);

	let advance: PlanUpdate = serde_json::from_value(json!({"action": "advance", "summary": "done"})).unwrap();
	assert_eq!(advance, PlanUpdate::Advance { summary: "done".into() });
	assert!(serde_json::from_value::<PlanUpdate>(json!({"action": "explode"})).is_err());
}

#[test]
fn notifications_are_events_and_every_event_is_listed_once() {
	let all: HashSet<&str> = event::ALL.iter().copied().collect();
	assert_eq!(all.len(), event::ALL.len());
	for name in event::NOTIFICATIONS {
		assert!(all.contains(name), "{name}");
	}
	assert_eq!(
		event::NOTIFICATIONS,
		[event::COMPACT_AFTER, event::SUBAGENT_END, event::SESSION_END]
	);
	assert_eq!(event::ALL.first(), Some(&event::SESSION_START));
	assert_eq!(event::ALL.last(), Some(&event::SESSION_END));
}

#[test]
fn rpc_request_without_an_id_is_a_notification() {
	let notification = serde_json::to_value(RpcRequest::new(None, "session.end", json!({}))).unwrap();
	assert!(notification.get("id").is_none());
	assert_eq!(notification["jsonrpc"], "2.0");
	let request = serde_json::to_value(RpcRequest::new(Some(3), "initialize", json!({}))).unwrap();
	assert_eq!(request["id"], 3);
}

#[test]
fn cancel_names_the_request_it_cancels() {
	assert_eq!(METHOD_CANCEL, "cancel");
	assert_eq!(serde_json::to_value(CancelParams { id: 9 }).unwrap(), json!({"id": 9}));
}

#[test]
fn blocks_default_to_plain_text_and_the_handshake_may_declare_timeouts() {
	let block: Block = serde_json::from_value(json!({"name": "n", "text": "t"})).unwrap();
	assert!(!block.keep && block.items.is_empty());
	let kept: Block = serde_json::from_value(json!({"name": "n", "text": "t", "keep": true, "items": ["a"]})).unwrap();
	assert!(kept.keep);
	assert_eq!(kept.items, vec!["a".to_string()]);

	let result: InitializeResult = serde_json::from_value(json!({
		"protocol": 1,
		"plugin": {"name": "p", "version": "1"},
		"subscribe": [],
		"timeouts": {"turn.end": 120},
		"status_fields": [{"name": "f", "instruction": "i", "default": "x"}]
	}))
	.unwrap();
	assert_eq!(result.timeouts["turn.end"], 120);
	assert_eq!(result.status_fields[0].default, json!("x"));
	assert!(result.status_fields[0].example.is_none());
}

#[test]
fn rpc_response_carries_a_result_or_an_error() {
	let ok: RpcResponse = serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap();
	assert!(ok.result.is_some() && ok.error.is_none());
	let failed: RpcResponse =
		serde_json::from_str(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"no"}}"#).unwrap();
	assert_eq!(failed.error.unwrap().code, -32601);
}
