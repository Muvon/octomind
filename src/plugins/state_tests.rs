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
use crate::plugins::protocol::{Deny, Row};
use crate::plugins::test_support::StubPlugin;

/// `judge` and `session.end` are answered, but run outside the conversation.
const OUTSIDE_CONVERSATION: [&str; 2] = [event::JUDGE, event::SESSION_END];

#[test]
fn every_answered_event_of_the_conversation_allows_the_common_effects() {
	let answered = event::ALL
		.iter()
		.filter(|name| !event::NOTIFICATIONS.contains(name) && !OUTSIDE_CONVERSATION.contains(name));
	for name in answered {
		let allowed = allowed_effects(name);
		for effect in [
			effect::CONTEXT,
			effect::PIN,
			effect::USAGE,
			effect::NOTICE,
			effect::REGISTER,
			effect::HISTORY,
		] {
			assert!(allowed.contains(&effect), "{name} must allow {effect}");
		}
	}
}

#[test]
fn a_judgement_and_the_end_of_a_session_cannot_change_the_conversation() {
	assert_eq!(
		allowed_effects(event::SESSION_END),
		[effect::USAGE, effect::REPORT, effect::NOTICE]
	);
	assert_eq!(
		allowed_effects(event::JUDGE),
		[effect::USAGE, effect::REPORT, effect::NOTICE, effect::JUDGEMENTS]
	);
}

#[test]
fn command_and_status_allow_no_note_and_no_plan() {
	for name in [event::COMMAND, event::STATUS] {
		let allowed = allowed_effects(name);
		assert!(!allowed.contains(&effect::NOTE), "{name}");
		assert!(!allowed.contains(&effect::PLAN), "{name}");
	}
	for name in [event::SESSION_START, event::TOOL_AFTER, event::TURN_END] {
		let allowed = allowed_effects(name);
		assert!(
			allowed.contains(&effect::NOTE) && allowed.contains(&effect::PLAN),
			"{name}"
		);
	}
}

#[test]
fn each_event_allows_its_own_extra_effects_only() {
	let extra = [
		(event::PROMPT_SUBMIT, vec![effect::REPLACE_PROMPT, effect::BLOCK]),
		(event::TOOL_BEFORE, vec![effect::DENY, effect::LIFT]),
		(
			event::TOOL_AFTER,
			vec![effect::REPLACE, effect::STOP, effect::RESULT_NOTE],
		),
		(event::TURN_END, vec![effect::CONTINUE, effect::VERIFIED]),
		(event::SUBAGENT_START, vec![effect::INHERIT]),
		(event::COMMAND, vec![effect::OUTPUT]),
		(event::STATUS, vec![effect::ROWS]),
		(event::JUDGE, vec![effect::JUDGEMENTS]),
	];
	let every_extra: Vec<&str> = extra.iter().flat_map(|(_, effects)| effects.clone()).collect();
	for (name, own) in &extra {
		let allowed = allowed_effects(name);
		for effect in &every_extra {
			assert_eq!(allowed.contains(effect), own.contains(effect), "{name} / {effect}");
		}
	}
	for name in [event::SESSION_START, event::REQUEST_BEFORE, event::COMPACT_BEFORE] {
		assert!(
			every_extra.iter().all(|effect| !allowed_effects(name).contains(effect)),
			"{name}"
		);
	}
}

#[test]
fn notifications_allow_nothing_and_the_table_lists_every_event() {
	let table = events_table();
	assert_eq!(table.len(), event::ALL.len());
	for name in event::NOTIFICATIONS {
		assert!(table[name].is_empty(), "{name}");
	}
	assert!(table[event::TOOL_BEFORE].contains(&effect::DENY.to_string()));
}

#[test]
fn sanitize_drops_effects_the_event_does_not_allow() {
	let mut answer = Answer {
		notice: vec!["kept".into()],
		block: Some("not here".into()),
		deny: vec![Deny {
			call_id: "c".into(),
			reason: "r".into(),
		}],
		rows: vec![Row {
			label: "l".into(),
			value: "v".into(),
		}],
		..Answer::default()
	};
	sanitize("p", event::REQUEST_BEFORE, &mut answer);
	assert_eq!(answer.notice, vec!["kept".to_string()]);
	assert!(answer.block.is_none() && answer.deny.is_empty() && answer.rows.is_empty());
}

#[test]
fn sanitize_keeps_what_the_event_allows() {
	let mut answer = Answer {
		block: Some("stop".into()),
		replace_prompt: Some("new".into()),
		stop: Some("not here".into()),
		..Answer::default()
	};
	sanitize("p", event::PROMPT_SUBMIT, &mut answer);
	assert_eq!(answer.block.as_deref(), Some("stop"));
	assert_eq!(answer.replace_prompt.as_deref(), Some("new"));
	assert!(answer.stop.is_none());
}

#[test]
fn sanitize_clears_everything_from_a_notification() {
	let mut answer = Answer {
		notice: vec!["x".into()],
		..Answer::default()
	};
	sanitize("p", event::COMPACT_AFTER, &mut answer);
	assert_eq!(answer, Answer::default());
}

#[test]
fn rule_ids_read_every_rule_kind() {
	let rules = "[[guard]]\nid = \"g1\"\nmatch = \"x\"\n\n[[hook]]\nid = \"h1\"\n\n[[pipe]]\nname = \"no id\"\n";
	let ids = rule_ids(rules);
	assert_eq!(ids, HashSet::from(["g1".to_string(), "h1".to_string()]));
}

#[test]
fn rule_ids_of_broken_text_are_empty() {
	assert!(rule_ids("[[guard").is_empty());
	assert!(rule_ids("").is_empty());
}

#[test]
fn wire_message_marks_system_managed_user_text() {
	let managed = crate::session::Message {
		role: "user".into(),
		content: "<system-note>\nfeedback\n</system-note>".into(),
		..Default::default()
	};
	let typed = crate::session::Message {
		role: "user".into(),
		content: "please fix".into(),
		..Default::default()
	};
	let assistant = crate::session::Message {
		role: "assistant".into(),
		content: "<system-note>quoted</system-note>".into(),
		..Default::default()
	};
	assert!(wire_message(&managed, None).managed);
	assert!(!wire_message(&typed, None).managed);
	assert!(!wire_message(&assistant, None).managed);
}

#[test]
fn wire_message_cuts_tool_results_by_characters() {
	let tool = crate::session::Message {
		role: "tool".into(),
		content: "ééééé".into(),
		tool_call_id: Some("c1".into()),
		..Default::default()
	};
	assert_eq!(wire_message(&tool, Some(3)).content, "ééé");
	assert_eq!(wire_message(&tool, Some(50)).content, "ééééé");
	assert_eq!(wire_message(&tool, None).content, "ééééé");
	let user = crate::session::Message {
		role: "user".into(),
		content: "ééééé".into(),
		..Default::default()
	};
	assert_eq!(wire_message(&user, Some(3)).content, "ééééé");
	assert_eq!(wire_message(&tool, Some(3)).tool_call_id.as_deref(), Some("c1"));
}

#[test]
fn wire_message_reads_the_reasoning_text() {
	let message = crate::session::Message {
		role: "assistant".into(),
		content: "answer".into(),
		thinking: Some(serde_json::json!({"content": " thought ", "tokens": 4})),
		..Default::default()
	};
	assert_eq!(wire_message(&message, None).thinking.as_deref(), Some("thought"));
}

#[test]
fn set_block_keeps_the_order_the_plugin_first_set_them_in() {
	let block = |name: &str, text: Option<&str>| Block {
		name: name.into(),
		text: text.map(str::to_string),
		items: Vec::new(),
	};
	let mut blocks = Vec::new();
	for (name, text) in [("zeta", "1"), ("alpha", "2"), ("mid", "3")] {
		set_block(&mut blocks, block(name, Some(text)));
	}
	// Setting a name again replaces it in place; clearing removes it.
	set_block(
		&mut blocks,
		Block {
			items: vec!["i".into()],
			..block("alpha", Some("two"))
		},
	);
	set_block(&mut blocks, block("mid", None));
	set_block(&mut blocks, block("never-set", None));
	let names: Vec<&str> = blocks.iter().map(|held| held.name.as_str()).collect();
	assert_eq!(names, vec!["zeta", "alpha"]);
	assert_eq!(blocks[1].text, "two");
	assert_eq!(blocks[1].items, vec!["i".to_string()]);
}

#[test]
fn an_event_gets_the_declared_time_capped_by_the_users_limit() {
	let secs = Duration::from_secs;
	assert_eq!(event_limit(None, None), secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS));
	assert_eq!(event_limit(None, Some(120)), secs(120));
	assert_eq!(event_limit(Some(10), Some(120)), secs(10));
	assert_eq!(event_limit(Some(300), Some(120)), secs(120));
	// The user's limit also caps the default for an event the plugin did not list.
	assert_eq!(event_limit(Some(5), None), secs(5));
	assert_eq!(
		event_limit(None, Some(MAX_PLUGIN_TIMEOUT_SECONDS + 1)),
		secs(MAX_PLUGIN_TIMEOUT_SECONDS)
	);
	assert_eq!(event_limit(None, Some(0)), secs(1));
}

#[test]
fn the_tool_list_hash_changes_with_the_list() {
	let tool = |name: &str| protocol::ToolInfo {
		name: name.into(),
		server: "core".into(),
		description: String::new(),
		parameters: Value::Null,
		read_only: None,
	};
	let one = [tool("a")];
	assert_eq!(tools_hash(&one), tools_hash(&[tool("a")]));
	assert_ne!(tools_hash(&one), tools_hash(&[tool("a"), tool("b")]));
}

fn slot_declaring(timeouts: &[(&str, u64)]) -> PluginSlot {
	let config: PluginConfig = toml::from_str("name = \"p\"\ntype = \"stdio\"\ncommand = \"p\"\n").unwrap();
	let result = protocol::InitializeResult {
		protocol: protocol::PROTOCOL_VERSION,
		plugin: protocol::ProgramInfo {
			name: "p".into(),
			version: "0".into(),
		},
		subscribe: Vec::new(),
		options: PluginOptions::default(),
		status_fields: Vec::new(),
		commands: Vec::new(),
		timeouts: timeouts
			.iter()
			.map(|(name, seconds)| (name.to_string(), *seconds))
			.collect(),
	};
	let transport = Arc::new(StubPlugin::new(&[]));
	PluginSlot::new(
		&config,
		transport,
		HashSet::from([event::TOOL_AFTER.to_string()]),
		&result,
	)
}

#[test]
fn session_end_waits_five_seconds_at_most_whatever_the_plugin_declares() {
	let secs = Duration::from_secs;
	let long = slot_declaring(&[(event::SESSION_END, 3600)]);
	assert_eq!(
		long.wait_limit(event::SESSION_END),
		WaitLimit {
			silence: SESSION_END_WAIT,
			total: SESSION_END_WAIT
		}
	);
	let short = slot_declaring(&[(event::SESSION_END, 2)]);
	assert_eq!(
		short.wait_limit(event::SESSION_END),
		WaitLimit {
			silence: secs(2),
			total: SESSION_END_WAIT
		}
	);
}

#[test]
fn another_event_waits_its_declared_silence_but_never_past_the_hosts_maximum() {
	let slot = slot_declaring(&[(event::TOOL_AFTER, 120)]);
	assert_eq!(
		slot.wait_limit(event::TOOL_AFTER),
		WaitLimit {
			silence: Duration::from_secs(120),
			total: Duration::from_secs(MAX_PLUGIN_TIMEOUT_SECONDS)
		}
	);
}

#[test]
fn a_paused_slot_takes_nothing_until_the_pause_is_over_and_an_answer_ends_it() {
	let mut slot = slot_declaring(&[]);
	assert!(slot.takes(event::TOOL_AFTER));
	assert!(!slot.takes(event::TURN_END), "not subscribed");

	slot.paused_until = Some(Instant::now() + Duration::from_secs(60));
	assert!(!slot.takes(event::TOOL_AFTER));

	slot.paused_until = Some(Instant::now() - Duration::from_secs(1));
	assert!(slot.takes(event::TOOL_AFTER), "the next event is the trial");
	slot.failures = MAX_CONSECUTIVE_FAILURES;
	slot.pause = LONGEST_PAUSE;

	assert!(slot.recover());
	assert!(slot.paused_until.is_none() && slot.failures == 0);
	assert_eq!(slot.pause, FIRST_PAUSE);
	assert!(!slot.recover(), "it was not paused any more");
}

#[test]
fn overlapping_waits_count_from_the_first_start_to_the_last_end() {
	let start = Instant::now();
	let at = |milliseconds| start + Duration::from_millis(milliseconds);
	let mut clock = WaitClock::default();

	clock.enter(at(0));
	clock.enter(at(50));
	clock.leave(at(100));
	clock.leave(at(150));
	assert_eq!(
		clock.take_ms(at(200)),
		150,
		"idle time after the last end is not counted"
	);

	clock.enter(at(300));
	clock.leave(at(340));
	assert_eq!(clock.take_ms(at(400)), 40);
	assert_eq!(clock.take_ms(at(500)), 0);
}

#[test]
fn taking_the_time_while_an_event_is_awaited_counts_what_has_passed_and_goes_on() {
	let start = Instant::now();
	let at = |milliseconds| start + Duration::from_millis(milliseconds);
	let mut clock = WaitClock::default();

	clock.enter(at(0));
	assert_eq!(clock.take_ms(at(70)), 70);
	clock.leave(at(100));

	assert_eq!(clock.take_ms(at(200)), 30);
}
