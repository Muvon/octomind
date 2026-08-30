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

impl Detectors {
	/// Test shim: run ONE call through the real two-phase path as a single-call
	/// round (note_call → record_round_signals) and return the round signal. Lets
	/// the existing per-call tests exercise the new per-round code unchanged.
	#[allow(clippy::too_many_arguments)]
	fn record_action(
		&mut self,
		tool: &str,
		result: &str,
		is_error: bool,
		is_mutation: bool,
		loop_threshold: usize,
		no_progress_window: usize,
	) -> DetectorSignal {
		let (rhash, novel) = self.note_call(tool, result, is_error, is_mutation);
		self.record_round_signals(&[rhash], novel, loop_threshold, no_progress_window)
	}
}

#[test]
fn parses_state_only() {
	assert_eq!(
		parse_self_report("work\n<sup>done</sup>"),
		Some((SelfReport::Done, None))
	);
}

#[test]
fn parses_state_with_reason() {
	let r = parse_self_report("x <sup>progressing · editing api</sup> y");
	assert_eq!(
		r,
		Some((SelfReport::Progressing, Some("editing api".into())))
	);
}

#[test]
fn parses_structured_handoff_and_strips_it() {
	let text = r#"answer
<sup>{"state":"progressing","focus":"inspect the active state","next":"continue from the last verified checkpoint","carry":["credential source is configured externally","retain opaque-run-ref"]}</sup>"#;
	let parsed = parse_self_report_handoff(text).expect("structured report");
	assert_eq!(parsed.state, SelfReport::Progressing);
	assert_eq!(parsed.handoff.focus, "inspect the active state");
	assert_eq!(
		parsed.handoff.next,
		"continue from the last verified checkpoint"
	);
	assert_eq!(parsed.handoff.carry.len(), 2);
	assert_eq!(parsed.plan, None);
	assert!(parsed.used_memories.is_empty());
	assert_eq!(strip_self_report(text), "answer");
}

#[test]
fn parses_materially_used_memory_ids() {
	let text = r#"<sup>{"state":"progressing","focus":"applying a recalled constraint","next":"continue","carry":[],"plan":null,"memories":[" M2 ","M4",""]}</sup>"#;
	let parsed = parse_self_report_handoff(text).expect("structured report");
	assert_eq!(parsed.used_memories, vec!["M2", "M4"]);
}

#[test]
fn parses_materially_used_evolved_behavior_ids_separately() {
	let parsed = parse_self_report_handoff(
			r#"answer
<sup>{"state":"progressing","focus":"used evolved skill","next":"continue","carry":[],"plan":null,"memories":["M2"],"behaviors":["evo-rust-123"]}</sup>"#,
		)
		.unwrap();
	assert_eq!(parsed.used_memories, vec!["M2"]);
	assert_eq!(parsed.used_behaviors, vec!["evo-rust-123"]);
}

#[test]
fn parses_external_plan_signal_without_plan_content() {
	let text = r#"<sup>{"state":"progressing","focus":"surveying sources","next":"compare findings","carry":[],"plan":"request"}</sup>"#;
	let parsed = parse_self_report_handoff(text).expect("structured report");
	assert_eq!(
		parsed.plan,
		Some(crate::supervisor::plan::PlanSignal::Request)
	);
}

#[test]
fn malformed_structured_handoff_is_not_accepted_as_status() {
	// Rejected as a status (no `carry`) — but still never shown to the user.
	let malformed = r#"<sup>{"state":"progressing","focus":"x"}</sup>"#;
	assert!(parse_self_report_handoff(malformed).is_none());
	assert_eq!(strip_self_report(malformed), "");
	// Truncated mid-token: not parseable at all, still hidden.
	assert_eq!(strip_self_report(r#"a <sup>{"state":"do</sup>"#), "a");
}

#[test]
fn done_report_with_null_next_parses_and_is_hidden() {
	let text = r#"answer
<sup>{"state":"done","focus":"briefed the staged changes","next":null,"carry":["one file left untracked"],"plan":null}</sup>"#;
	let parsed = parse_self_report_handoff(text).expect("null next is a valid done report");
	assert_eq!(parsed.state, SelfReport::Done);
	assert_eq!(parsed.handoff.next, "");
	assert_eq!(strip_self_report(text), "answer");
}

#[test]
fn need_input_variants() {
	assert_eq!(
		parse_self_report("<sup>need_input</sup>").map(|(s, _)| s),
		Some(SelfReport::NeedInput)
	);
}

#[test]
fn strips_token_and_trailing_blank() {
	assert_eq!(strip_self_report("answer\n\n<sup>done</sup>"), "answer");
}

#[test]
fn strips_and_parses_real_multiword_reason() {
	let s = "answer\n<sup>need_input · Phase 1 complete, awaiting user direction</sup>";
	assert_eq!(strip_self_report(s), "answer");
	let (st, reason) = parse_self_report(s).unwrap();
	assert_eq!(st, SelfReport::NeedInput);
	assert_eq!(
		reason.as_deref(),
		Some("Phase 1 complete, awaiting user direction")
	);
}

#[test]
fn handles_non_dot_separators() {
	assert_eq!(strip_self_report("x <sup>done: all good</sup>"), "x");
	assert_eq!(
		strip_self_report("x <sup>blocked - cannot proceed</sup>"),
		"x"
	);
	assert_eq!(
		parse_self_report("<sup>done: all good</sup>").map(|(s, _)| s),
		Some(SelfReport::Done)
	);
}

#[test]
fn keeps_legitimate_superscript() {
	// `<sup>2</sup>` (x squared) is not a state token — keep it verbatim.
	assert_eq!(strip_self_report("x<sup>2</sup> + 1"), "x<sup>2</sup> + 1");
	// A short non-state superscript with no separator stays too.
	assert_eq!(
		strip_self_report("the 5<sup>th</sup>"),
		"the 5<sup>th</sup>"
	);
}

#[test]
fn strips_echoed_state_placeholder() {
	// The reported leak: a model copies the literal `STATE` placeholder.
	// It must never reach the screen, and we recover the intended state.
	assert_eq!(
		strip_self_report("answer\n<sup>STATE · done</sup>"),
		"answer"
	);
	assert_eq!(
		parse_self_report("answer\n<sup>STATE · done</sup>"),
		Some((SelfReport::Done, None))
	);
	// Bare echoed placeholder (no reason) is stripped as well.
	assert_eq!(strip_self_report("answer <sup>STATE</sup>"), "answer");
}

#[test]
fn strips_report_with_unknown_lead_but_separator() {
	// Even a malformed state word can't leak once the `·` separator is present.
	assert_eq!(
		strip_self_report("ok\n<sup>finished · all good</sup>"),
		"ok"
	);
}

#[test]
fn loop_fires_on_repeated_result() {
	let mut d = Detectors::default();
	assert_eq!(
		d.record_action("grep", "same", false, false, 3, 9),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_action("grep", "same", false, false, 3, 9),
		DetectorSignal::None
	);
	// Third identical RESULT → loop.
	assert_eq!(
		d.record_action("grep", "same", false, false, 3, 9),
		DetectorSignal::Loop
	);
}

#[test]
fn no_progress_fires_on_zero_novelty_window() {
	let mut d = Detectors::default();
	d.record_action("a", "r", false, false, 9, 3); // first "r" → novel
	d.record_action("a", "r", false, false, 9, 3); // seen → not novel
	d.record_action("a", "r", false, false, 9, 3); // not novel
	assert_eq!(
		d.record_action("a", "r", false, false, 9, 3),
		DetectorSignal::NoProgress
	);
}

#[test]
fn mutation_counts_as_progress() {
	let mut d = Detectors::default();
	d.record_action("read", "same", false, false, 9, 2);
	d.record_action("read", "same", false, false, 9, 2);
	// An edit always advances state → breaks the stall.
	assert_eq!(
		d.record_action("edit", "ok", false, true, 9, 2),
		DetectorSignal::None
	);
}

#[test]
fn verification_shape_fallback_without_fingerprints() {
	let mut d = Detectors::default();
	assert!(!d.needs_verification(None));
	// Mutation-shaped round, no verifier → unverified.
	d.note_round_verification(None, None, false, false, true, false, true);
	assert!(d.needs_verification(None));
	// A read-only round changes nothing — looking is not verifying.
	d.note_round_verification(None, None, false, false, false, false, false);
	assert!(d.needs_verification(None));
	// A round where the verifier ran alongside a mutation proves nothing.
	d.note_round_verification(None, None, true, false, true, false, true);
	assert!(d.needs_verification(None));
	// A clean verifier round clears it.
	d.note_round_verification(None, None, true, false, false, false, true);
	assert!(!d.needs_verification(None));
}

#[test]
fn verification_tracks_tree_fingerprint() {
	let mut d = Detectors::default();
	// Round 1 seeds the baseline (10 = task-start tree); the round's edit
	// moved the tree to 11 → unverified.
	d.note_round_verification(Some(10), Some(11), false, false, true, false, true);
	assert!(d.needs_verification(Some(11)));
	// Verifier ran but the same round dirtied the tree (11→12): ambiguous
	// state, proves nothing.
	d.note_round_verification(Some(11), Some(12), true, false, true, false, true);
	assert!(d.needs_verification(Some(12)));
	// Clean verifier on an unchanged tree → verified at 12.
	d.note_round_verification(Some(12), Some(12), true, false, false, false, true);
	assert!(!d.needs_verification(Some(12)));
	// Drift with NO agent round in between is external (the user editing
	// their own tree): the agent changed nothing since its clean
	// verification, so there is nothing for it to verify. Agent-made edits
	// through ANY tool (`shell sed -i` included) are still caught — they
	// move the fingerprint ACROSS their own round, as above.
	assert!(!d.needs_verification(Some(13)));
}

#[test]
fn external_drift_never_arms_verification() {
	let mut d = Detectors::default();
	// Read-only rounds over a tree that drifts externally mid-session — the
	// observe-only job shape (review/brief/audit): the deliverable is a
	// report, and a done-claim needs no check run.
	d.note_round_verification(Some(10), Some(10), false, false, false, false, false);
	assert!(!d.needs_verification(Some(11)));
	// Drift DURING a pure-read round (a concurrent editor, a dev server, a
	// generated artifact moving the tree while the agent only views and
	// searches): no call could have written, so the movement is external
	// and must not arm — this is what falsely gated read-only jobs.
	d.note_round_verification(Some(11), Some(12), false, false, false, false, false);
	assert!(!d.needs_verification(Some(12)));
	// A write-capable round that moved the tree arms it, even when no call
	// was mutation-shaped (an edit hidden inside a shell command).
	d.note_round_verification(Some(12), Some(13), false, false, false, false, true);
	assert!(d.needs_verification(Some(13)));
}

#[test]
fn delegated_verification_clears_a_round_that_changed_the_tree() {
	let mut d = Detectors::default();
	// An orchestrator's `tap run`: the specialist edited AND checked inside
	// this one parent round, so the tree moved and no parent call could ever
	// be verifier-shaped. Without the child's verdict this latches dirty
	// forever and every `done` re-triggers the mutation pre-gate. A round
	// with delegated runs is always write-capable — the child can write
	// through any tool of its own.
	d.note_round_verification(Some(10), Some(11), false, false, false, false, true);
	assert!(d.needs_verification(Some(11)));
	// Same round shape, child reported verified → accepted, and the
	// post-round tree becomes the new baseline.
	let mut d = Detectors::default();
	d.note_round_verification(Some(10), Some(11), false, false, false, true, true);
	assert!(!d.needs_verification(Some(11)));
}

#[test]
fn delegated_verification_does_not_cover_the_parents_own_edit() {
	let mut d = Detectors::default();
	// Parallel round: a verified subagent alongside the parent's own
	// mutation-shaped call. The child never checked the parent's edit, so
	// its verdict must not clear the round.
	d.note_round_verification(Some(10), Some(11), false, false, true, true, true);
	assert!(d.needs_verification(Some(11)));
}

#[test]
fn readback_of_mutated_path_verifies_artifact_work() {
	use serde_json::json;
	let mut d = Detectors::default();
	// Round 1: agent edits a doc — mutation round, tree moves, dirty.
	d.note_mutated_paths(&json!({"path": "blog/post/index.md"}));
	d.note_round_verification(Some(10), Some(11), false, false, true, false, true);
	assert!(d.needs_verification(Some(11)));
	// Round 2: agent re-reads the exact artifact it changed — that IS the
	// verification for work with no command to run.
	let readback = d.is_readback_call(
		&json!({"path": "blog/post/index.md", "start": 85}),
		false,
		false,
	);
	assert!(readback);
	d.note_round_verification(Some(11), Some(11), false, readback, false, false, false);
	assert!(!d.needs_verification(Some(11)));
}

#[test]
fn readback_requires_matching_path_success_and_no_mutation() {
	use serde_json::json;
	let mut d = Detectors::default();
	d.note_mutated_paths(&json!({"path": "a.md"}));
	// Different artifact → not a read-back.
	assert!(!d.is_readback_call(&json!({"path": "b.md"}), false, false));
	// Mutation call re-touching the path is more editing, not verification.
	assert!(!d.is_readback_call(&json!({"path": "a.md"}), true, false));
	// Failed read proves nothing.
	assert!(!d.is_readback_call(&json!({"path": "a.md"}), false, true));
	// No mutated paths recorded → nothing to read back.
	let fresh = Detectors::default();
	assert!(!fresh.is_readback_call(&json!({"path": "a.md"}), false, false));
}

#[test]
fn param_paths_collects_pathish_keys_only() {
	use serde_json::json;
	let p = json!({
		"path": "a.md",
		"from_path": "b.rs",
		"files": ["c.py", ""],
		"command": "rm -rf /",
		"content": "path-like text ignored"
	});
	let mut got = param_paths(&p);
	got.sort();
	assert_eq!(got, vec!["a.md", "b.rs", "c.py"]);
	assert!(param_paths(&json!({"command": "x"})).is_empty());
}

#[test]
fn verifier_shape_requires_command_string_param() {
	use serde_json::json;
	// Command-string param → candidate (tool_map is empty in unit tests, so
	// the control-plane exclusion is exercised in integration, not here).
	assert!(is_verifier_shaped(
		"shell",
		&json!({"command": "cargo test"})
	));
	assert!(!is_verifier_shaped("view", &json!({"path": "a.rs"})));
	assert!(!is_verifier_shaped("shell", &json!({"command": 42})));
	assert!(!is_verifier_shaped("shell", &json!({})));
	assert!(!is_verifier_shaped("shell", &json!({"command": ""})));
}

#[test]
fn verifier_shape_is_domain_agnostic() {
	use serde_json::json;
	// Any non-mutation command execution is a verifier candidate: the
	// framework does not hard-code program or script names. Whether a
	// candidate actually verifies is decided observationally (tree unchanged).
	assert!(is_verifier_shaped(
		"shell",
		&json!({"command": "bash scripts/lint-capabilities.sh \"$PWD/capabilities/\""})
	));
	assert!(is_verifier_shaped(
		"shell",
		&json!({"command": "cd /proj && sh scripts/test.sh"})
	));
	assert!(!is_verifier_shaped(
		"shell",
		&json!({"command": "bash scripts/deploy.sh"})
	));
	assert!(is_verifier_shaped(
		"shell",
		&json!({"command": "python check_booking.py --ref ABC123"})
	));
	assert!(!is_verifier_shaped(
		"text_editor",
		&json!({"command": "str_replace"})
	));
}

#[test]
fn mutation_classification_uses_call_intent_and_mcp_hint() {
	use serde_json::json;
	assert!(is_mutation_call(
		"text_editor",
		&json!({"command":"str_replace"})
	));
	assert!(is_mutation_call(
		"generic_runner",
		&json!({"command":"deploy release"})
	));
	assert!(!is_mutation_call(
		"generic_runner",
		&json!({"command":"check booking status"})
	));
	register_tool_read_only_hint("remotePublisherForTest", Some(false));
	register_tool_read_only_hint("remoteLookupForTest", Some(true));
	assert!(is_mutation_call("remotePublisherForTest", &json!({})));
	assert!(!is_mutation_call("remoteLookupForTest", &json!({})));
}

#[test]
fn reset_streak_clears_previous_task_verification_latch() {
	let mut d = Detectors::default();
	d.note_round_verification(None, None, false, false, true, false, true);
	assert!(d.needs_verification(None));
	// A new genuine task must not inherit an earlier task's mutation.
	d.reset_streak();
	assert!(!d.needs_verification(None));
}

#[test]
fn steer_defers_to_gate_on_done() {
	assert!(!should_steer(
		DetectorSignal::NoProgress,
		Some(SelfReport::Done)
	));
	assert!(!should_steer(DetectorSignal::Loop, Some(SelfReport::Done)));
}

#[test]
fn steer_waits_while_exploring_but_fires_on_loop() {
	assert!(!should_steer(
		DetectorSignal::NoProgress,
		Some(SelfReport::Exploring)
	));
	assert!(should_steer(
		DetectorSignal::Loop,
		Some(SelfReport::Exploring)
	));
	assert!(should_steer(
		DetectorSignal::NoProgress,
		Some(SelfReport::Progressing)
	));
}

#[test]
fn conflict_framing_when_progressing_but_no_progress() {
	// No-progress signal while the agent insists it is progressing → conflict text.
	let conflict = steer_note(DetectorSignal::NoProgress, Some(SelfReport::Progressing), 0);
	assert!(conflict.contains("disagree"));
	// Without the progressing claim it stays the generic no-progress note.
	let generic = steer_note(DetectorSignal::NoProgress, None, 0);
	assert!(!generic.contains("disagree"));
}

#[test]
fn failed_verifier_recovery_survives_unrelated_successes() {
	let mut d = Detectors::default();
	let failing_check = 11;
	let unrelated_check = 22;
	assert_eq!(
		d.record_round_verifier_outcomes(&[(failing_check, false)], 3),
		DetectorSignal::None
	);
	// A different successful command does not prove the failed behavior.
	assert_eq!(
		d.record_round_verifier_outcomes(&[(unrelated_check, true)], 3),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_verifier_outcomes(&[(failing_check, false)], 3),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_verifier_outcomes(&[(failing_check, false)], 3),
		DetectorSignal::Recovery
	);
}

#[test]
fn same_verifier_success_discharges_recovery() {
	let mut d = Detectors::default();
	let check = 11;
	assert_eq!(
		d.record_round_verifier_outcomes(&[(check, false)], 2),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_verifier_outcomes(&[(check, true)], 2),
		DetectorSignal::None
	);
	// The old failed episode is gone, so one new failure is below threshold.
	assert_eq!(
		d.record_round_verifier_outcomes(&[(check, false)], 2),
		DetectorSignal::None
	);
}

#[test]
fn conflicting_parallel_verifier_outcomes_do_not_clear_failure() {
	let mut d = Detectors::default();
	let check = 11;
	assert_eq!(
		d.record_round_verifier_outcomes(&[(check, false)], 2),
		DetectorSignal::None
	);
	assert_eq!(
		d.record_round_verifier_outcomes(&[(check, true), (check, false)], 2),
		DetectorSignal::Recovery
	);
}

#[test]
fn user_turn_reset_clears_failed_verifier_recovery() {
	let mut d = Detectors::default();
	assert_eq!(
		d.record_round_verifier_outcomes(&[(11, false)], 2),
		DetectorSignal::None
	);
	d.reset_streak();
	assert_eq!(
		d.record_round_verifier_outcomes(&[(11, false)], 2),
		DetectorSignal::None
	);
}

#[test]
fn call_set_hash_ignores_order_and_id_but_tracks_params() {
	use crate::mcp::McpToolCall;
	let mk = |name: &str, p: serde_json::Value| McpToolCall {
		tool_name: name.into(),
		parameters: p,
		tool_id: "per-call-unique".into(),
	};
	let read = mk("read", serde_json::json!({"path": "x"}));
	let grep = mk("grep", serde_json::json!({"q": "y"}));
	// Same calls, any order, any tool_id → equal hash (re-issuing them = ignoring).
	assert_eq!(
		call_set_hash(&[read.clone(), grep.clone()]),
		call_set_hash(&[
			mk("grep", serde_json::json!({"q": "y"})),
			mk("read", serde_json::json!({"path": "x"})),
		])
	);
	// A changed parameter → different hash (the model trying a different call).
	assert_ne!(
		call_set_hash(&[read]),
		call_set_hash(&[mk("read", serde_json::json!({"path": "z"}))])
	);
}

#[test]
fn persistent_frame_clamps_stuck_signals_past_the_ladder() {
	// A stuck signal re-firing past the 0→1→2 ladder holds the firmest frame: every
	// persistent variant carries the same firm ask (a different path, or `blocked`).
	assert!(steer_note(DetectorSignal::Loop, None, 5).contains("blocked"));
	// …but the phrasing ROTATES each re-emit so the repeated nudge does not habituate
	// (polymorphic warnings resist habituation — Anderson 2015 / Ancker 2017).
	assert_ne!(
		steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT),
		steer_note(DetectorSignal::Loop, None, PERSISTENT_ATTEMPT + 1)
	);
}
