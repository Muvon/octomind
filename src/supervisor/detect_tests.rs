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

//! Complementary unit tests for the detectors module: token-parsing edge
//! cases, the steer decision table, and per-round detector state transitions
//! beyond the inline module tests.

use super::*;
use serde_json::json;

#[test]
fn from_token_accepts_aliases_case_and_whitespace() {
	assert_eq!(
		SelfReport::from_token("exploring"),
		Some(SelfReport::Exploring)
	);
	assert_eq!(
		SelfReport::from_token("PROGRESSING"),
		Some(SelfReport::Progressing)
	);
	assert_eq!(SelfReport::from_token("  Done  "), Some(SelfReport::Done));
	assert_eq!(
		SelfReport::from_token("need_input"),
		Some(SelfReport::NeedInput)
	);
	assert_eq!(
		SelfReport::from_token("need-input"),
		Some(SelfReport::NeedInput)
	);
	assert_eq!(
		SelfReport::from_token("needinput"),
		Some(SelfReport::NeedInput)
	);
	assert_eq!(SelfReport::from_token("blocked"), Some(SelfReport::Blocked));
	for unknown in ["finished", "", "state", "done now"] {
		assert_eq!(SelfReport::from_token(unknown), None);
	}
}

#[test]
fn as_str_roundtrips_through_from_token() {
	for state in [
		SelfReport::Exploring,
		SelfReport::Progressing,
		SelfReport::Blocked,
		SelfReport::NeedInput,
		SelfReport::Done,
	] {
		assert_eq!(SelfReport::from_token(state.as_str()), Some(state));
	}
}

#[test]
fn handoff_missing_or_unclosed_token_is_none() {
	assert!(parse_self_report_handoff("").is_none());
	assert!(parse_self_report_handoff("no tokens here").is_none());
	assert!(parse_self_report_handoff("<sup>done").is_none());
	assert!(parse_self_report_handoff("done</sup>").is_none());
}

#[test]
fn handoff_uses_the_last_sup_token() {
	let text = concat!(
		r#"a <sup>{"state":"exploring","focus":"f","next":null,"carry":[]}</sup> b "#,
		r#"<sup>{"state":"done","focus":"g","next":null,"carry":[]}</sup>"#
	);
	let parsed = parse_self_report_handoff(text).expect("last token wins");
	assert_eq!(parsed.state, SelfReport::Done);
	assert_eq!(parsed.handoff.focus, "g");
}

#[test]
fn handoff_rejects_unknown_state_and_unknown_fields() {
	let unknown_state = r#"<sup>{"state":"finished","focus":"f","next":null,"carry":[]}</sup>"#;
	assert!(parse_self_report_handoff(unknown_state).is_none());
	let extra_field =
		r#"<sup>{"state":"done","focus":"f","next":null,"carry":[],"surprise":1}</sup>"#;
	assert!(parse_self_report_handoff(extra_field).is_none());
}

#[test]
fn handoff_trims_focus_and_drops_empty_carry_entries() {
	let text = r#"<sup>{"state":"blocked","focus":"  waiting on creds  ","next":" retry ","carry":["","   ","keep me "]}</sup>"#;
	let parsed = parse_self_report_handoff(text).expect("parses");
	assert_eq!(parsed.handoff.focus, "waiting on creds");
	assert_eq!(parsed.handoff.next, "retry");
	assert_eq!(parsed.handoff.carry, vec!["keep me"]);
}

#[test]
fn handoff_parses_phase_complete_and_reassess_plan_signals() {
	for (wire, expected) in [
		(
			"phase_complete",
			crate::supervisor::plan::PlanSignal::PhaseComplete,
		),
		("reassess", crate::supervisor::plan::PlanSignal::Reassess),
	] {
		let text = format!(
			r#"<sup>{{"state":"progressing","focus":"f","next":"n","carry":[],"plan":"{wire}"}}</sup>"#
		);
		let parsed = parse_self_report_handoff(&text).expect("parses");
		assert_eq!(parsed.plan, Some(expected));
	}
}

#[test]
fn legacy_parse_tolerates_whitespace_and_pipe_separator() {
	let (state, reason) =
		parse_self_report("<sup> blocked | waiting on creds </sup>").expect("parses");
	assert_eq!(state, SelfReport::Blocked);
	assert_eq!(reason.as_deref(), Some("waiting on creds"));
}

#[test]
fn strip_removes_every_report_token_and_keeps_surrounding_text() {
	assert_eq!(
		strip_self_report("a<sup>done</sup>b<sup>exploring · why</sup>c"),
		"abc"
	);
	// Interior whitespace survives; only the tail is trimmed.
	assert_eq!(strip_self_report("a <sup>done</sup>\n\nb"), "a \n\nb");
}

#[test]
fn strip_keeps_json_without_state_key_unclosed_and_plain_text() {
	// Shape-matched on the `state` key: unrelated JSON is legitimate markup.
	let markup = r#"x <sup>{"foo":1}</sup> y"#;
	assert_eq!(strip_self_report(markup), markup);
	assert!(parse_self_report_handoff(r#"<sup>{"foo":1}</sup>"#).is_none());
	assert_eq!(strip_self_report("a <sup>done"), "a <sup>done");
	assert_eq!(strip_self_report(""), "");
	assert_eq!(strip_self_report("plain answer"), "plain answer");
}

#[test]
fn should_steer_decision_table() {
	for report in [
		None,
		Some(SelfReport::Exploring),
		Some(SelfReport::Progressing),
		Some(SelfReport::Blocked),
		Some(SelfReport::NeedInput),
		Some(SelfReport::Done),
	] {
		assert!(!should_steer(DetectorSignal::None, report));
	}
	// `done` always defers to the verify-gate.
	assert!(!should_steer(DetectorSignal::Loop, Some(SelfReport::Done)));
	assert!(!should_steer(
		DetectorSignal::Recovery,
		Some(SelfReport::Done)
	));
	// No-progress is legitimate only while exploring.
	assert!(!should_steer(
		DetectorSignal::NoProgress,
		Some(SelfReport::Exploring)
	));
	assert!(should_steer(
		DetectorSignal::NoProgress,
		Some(SelfReport::Blocked)
	));
	assert!(should_steer(
		DetectorSignal::NoProgress,
		Some(SelfReport::NeedInput)
	));
	assert!(should_steer(
		DetectorSignal::Loop,
		Some(SelfReport::Exploring)
	));
	assert!(should_steer(
		DetectorSignal::Recovery,
		Some(SelfReport::Exploring)
	));
	assert!(should_steer(DetectorSignal::Loop, None));
	assert!(should_steer(DetectorSignal::Recovery, None));
}

#[test]
fn signal_description_covers_every_fired_signal() {
	assert_eq!(signal_description(DetectorSignal::None), "");
	let descriptions = [
		signal_description(DetectorSignal::Loop),
		signal_description(DetectorSignal::NoProgress),
		signal_description(DetectorSignal::Recovery),
	];
	for desc in descriptions {
		assert!(!desc.is_empty());
	}
	assert_ne!(descriptions[0], descriptions[1]);
	assert_ne!(descriptions[1], descriptions[2]);
}

#[test]
fn steer_note_ladder_rotates_and_conflict_clamps() {
	assert_eq!(steer_note(DetectorSignal::None, None, 0), "");
	let rungs = [
		steer_note(DetectorSignal::Loop, None, 0),
		steer_note(DetectorSignal::Loop, None, 1),
		steer_note(DetectorSignal::Loop, None, 2),
	];
	assert_ne!(rungs[0], rungs[1]);
	assert_ne!(rungs[1], rungs[2]);
	// Persistent variants rotate by attempt but repeat on the mod-3 cycle.
	assert_ne!(
		steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT),
		steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT + 1)
	);
	assert_eq!(
		steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT),
		steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT + 3)
	);
	// Conflict framing has its own ladder, clamped at its last variant.
	let conflict = [
		steer_note(DetectorSignal::NoProgress, Some(SelfReport::Progressing), 0),
		steer_note(DetectorSignal::NoProgress, Some(SelfReport::Progressing), 1),
		steer_note(DetectorSignal::NoProgress, Some(SelfReport::Progressing), 2),
	];
	assert_ne!(conflict[0], conflict[1]);
	assert_ne!(conflict[1], conflict[2]);
}

#[test]
fn note_call_novelty_rules() {
	let mut d = Detectors::default();
	let (_, first) = d.note_call("t", "r", false, false);
	assert!(first, "first sight of a result is novel");
	let (_, repeat) = d.note_call("t", "r", false, false);
	assert!(!repeat, "an already-seen result is not novel");
	let (_, errored) = d.note_call("t2", "fresh", true, false);
	assert!(!errored, "errors carry no new information");
	let (_, mutating) = d.note_call("t", "r", true, true);
	assert!(mutating, "a mutation always advances state");
}

#[test]
fn parallel_batch_is_one_round_for_loop_detection() {
	let mut d = Detectors::default();
	let h = d.note_call("grep", "same", false, false).0;
	// Three identical calls inside ONE round are a single window entry.
	assert_eq!(
		d.record_round_signals(&[h, h, h], false, 3, 9),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_signals(&[h, h, h], false, 3, 9),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_signals(&[h, h, h], false, 3, 9),
		DetectorSignal::Loop
	);
}

#[test]
fn loop_window_resets_on_a_new_result() {
	let mut d = Detectors::default();
	let a = d.note_call("grep", "a", false, false).0;
	let b = d.note_call("grep", "b", false, false).0;
	d.record_round_signals(&[a], true, 3, 9);
	d.record_round_signals(&[a], false, 3, 9);
	assert_eq!(
		d.record_round_signals(&[b], true, 3, 9),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_signals(&[a], false, 3, 9),
		DetectorSignal::None
	);
}

#[test]
fn zero_thresholds_disable_their_signals() {
	let mut d = Detectors::default();
	// loop_threshold 0: identical novel rounds never fire Loop.
	for i in 0..4 {
		let h = d.note_call("t", &format!("r{i}"), false, false).0;
		assert_eq!(
			d.record_round_signals(&[h], true, 0, 9),
			DetectorSignal::None
		);
	}
	// no_progress_window 0: repeated stale rounds never fire NoProgress.
	let mut stale = Detectors::default();
	let h = stale.note_call("t", "r", false, false).0;
	for _ in 0..6 {
		assert_eq!(
			stale.record_round_signals(&[h], false, 9, 0),
			DetectorSignal::None
		);
	}
}

#[test]
fn mutated_paths_dedupe_and_cap() {
	let mut d = Detectors::default();
	d.note_mutated_paths(&json!({"path": "zz_detect_tests/a.md"}));
	d.note_mutated_paths(&json!({"path": "zz_detect_tests/a.md"}));
	assert_eq!(d.mutated_paths.len(), 1, "duplicate paths collapse");
	for i in 0..40 {
		d.note_mutated_paths(&json!({"path": format!("zz_detect_tests/{i}.md")}));
	}
	assert_eq!(d.mutated_paths.len(), 32, "capped at MUTATED_PATHS_CAP");
	assert!(
		!d.mutated_paths
			.contains(&"zz_detect_tests/a.md".to_string()),
		"oldest evicted"
	);
}

#[test]
fn verifier_key_is_shape_gated_and_stable() {
	let params = json!({"command": "cargo test -p octomind"});
	let key = verifier_key("detect_tests_runner", &params).expect("command-shaped check");
	assert_eq!(Some(key), verifier_key("detect_tests_runner", &params));
	assert_ne!(
		key,
		verifier_key(
			"detect_tests_runner",
			&json!({"command": "cargo test -p other"})
		)
		.expect("different command")
	);
	assert_eq!(verifier_key("view", &json!({"path": "a.rs"})), None);
	assert_eq!(
		verifier_key("detect_tests_runner", &json!({"command": "deploy it"})),
		None,
		"mutation intent is never a verifier"
	);
}

#[test]
fn mutation_intent_normalizes_camel_case_and_separators() {
	assert!(
		is_mutation_call("sendEmail", &json!({})),
		"camelCase splits into intent words"
	);
	assert!(is_mutation_call("createFile", &json!({})));
	assert!(is_mutation_call(
		"runner",
		&json!({"command": "git-push release"})
	));
	assert!(!is_mutation_call(
		"runner",
		&json!({"command": "cat notes.txt"})
	));
	assert!(!is_mutation_call("statusLookup", &json!({})));
}

#[test]
fn read_only_hint_none_falls_back_to_call_intent() {
	register_tool_read_only_hint("detectTestsUntold", None);
	assert!(!is_mutation_call("detectTestsUntold", &json!({})));
}

#[test]
fn needs_verification_stands_down_when_tree_reverts() {
	let mut d = Detectors::default();
	d.note_round_verification(Some(10), Some(11), false, false, true, false, true);
	assert!(d.needs_verification(Some(11)));
	assert!(
		!d.needs_verification(Some(10)),
		"back at the verified baseline"
	);
}

#[test]
fn cleared_by_readback_only_tracks_evidence_kind() {
	let mut d = Detectors::default();
	assert!(!d.cleared_by_readback_only());
	// Mutation, then a command-shaped check: clearance is not read-back-only.
	d.note_round_verification(Some(10), Some(11), false, false, true, false, true);
	d.note_round_verification(Some(11), Some(11), true, false, false, false, true);
	assert!(!d.needs_verification(Some(11)));
	assert!(!d.cleared_by_readback_only());
	// A fresh mutation cleared only by re-reading the artifact is read-back-only.
	d.note_mutated_paths(&json!({"path": "zz_detect_tests/doc.md"}));
	d.note_round_verification(Some(11), Some(12), false, false, true, false, true);
	let readback = d.is_readback_call(&json!({"path": "zz_detect_tests/doc.md"}), false, false);
	assert!(readback);
	d.note_round_verification(Some(12), Some(12), false, readback, false, false, false);
	assert!(!d.needs_verification(Some(12)));
	assert!(d.cleared_by_readback_only());
}

#[test]
fn recovery_zero_threshold_and_empty_outcomes_are_inert() {
	let mut d = Detectors::default();
	assert_eq!(
		d.record_round_verifier_outcomes(&[], 3),
		DetectorSignal::None
	);
	for _ in 0..5 {
		assert_eq!(
			d.record_round_verifier_outcomes(&[(9, false)], 0),
			DetectorSignal::None
		);
	}
}

#[test]
fn recovery_emission_resets_the_counter_not_the_ledger() {
	let mut d = Detectors::default();
	assert_eq!(
		d.record_round_verifier_outcomes(&[(1, false)], 2),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_verifier_outcomes(&[(1, false)], 2),
		DetectorSignal::Recovery
	);
	// Counter restarted: one more failing round is below the threshold again…
	assert_eq!(
		d.record_round_verifier_outcomes(&[(1, false)], 2),
		DetectorSignal::None
	);
	// …but the debt is still recorded: an unrelated success cannot discharge it.
	assert_eq!(
		d.record_round_verifier_outcomes(&[(2, true)], 2),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_verifier_outcomes(&[(1, false)], 2),
		DetectorSignal::Recovery
	);
}

#[test]
fn call_set_hash_is_sensitive_to_tool_and_params() {
	use crate::mcp::McpToolCall;
	let mk = |name: &str, p: serde_json::Value| McpToolCall {
		tool_name: name.into(),
		parameters: p,
		tool_id: "per-call-unique".into(),
	};
	assert_eq!(call_set_hash(&[]), call_set_hash(&[]));
	let read = mk("read", json!({"path": "x"}));
	assert_eq!(call_set_hash(&[read.clone()]), call_set_hash(&[read]));
	assert_ne!(
		call_set_hash(&[mk("read", json!({"path": "x"}))]),
		call_set_hash(&[mk("view", json!({"path": "x"}))])
	);
}
