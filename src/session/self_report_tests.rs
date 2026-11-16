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

/// The status-line instruction of earlier octomind versions, which carried the
/// `plan` and `memories` fields in its base text. The composed instruction must
/// still equal it once those two fields are registered, and a prompt built
/// without them must not contain it.
pub const SELF_REPORT_INSTRUCTION: &str = r#"Finish every response with one compact JSON status line — the last line, nothing after it:
	`<sup>{"state":"STATE","focus":"current subgoal and why","next":"next action","carry":["minimum fact or opaque reference needed after context loss"],"plan":null,"memories":[]}</sup>`
Use valid single-line JSON with exactly those fields. `carry` and `memories` may be empty and `next` is `null` when nothing remains to do. Put an active-memory ID such as `M2` in `memories` only when that entry materially affected this response or its chosen action. Never list entries merely because they were shown. Keep only information genuinely needed to resume. Never copy credentials or secret values into the report — retain only an opaque pointer, name, or location used to obtain them. Avoid generic text such as "working" or "continuing". STATE must be exactly one of:
- `exploring` — still gathering context, reading code
- `progressing` — actively making changes, or a background job you started is running: never poll it (`monitor list`, `ps`, `tail`, sleep, re-run, a variant, viewing its resource) and keep working on anything that does not need it (read or review code, write tests, prepare the next edit); only when nothing like that is left, reply with a short status and no tool call: its result arrives as your next user message
- `blocked` — stuck, cannot proceed
- `need_input` — asking the user a question and waiting on them
- `done` — the user's task is fully complete

`plan` is normally `null`. Set it to `"request"` once, alongside real work, only when the task clearly needs 3+ dependent outcomes or durable tracking. With an injected plan, use `"phase_complete"` alongside the next work batch only after the current outcome is evidenced, or `"reassess"` when evidence invalidates the remaining route. The external manager owns the plan; never emit a response only for planning: a plan request or update always rides on real work.
Example: `<sup>{"state":"progressing","focus":"fixing tab handling in the fence parser","next":"run the parser tests for the edited path","carry":["regression input is tests/fixtures/tabs.md"],"plan":null,"memories":["M2"]}</sup>`
This line is read by the system and hidden from the user. Emit exactly one."#;

#[test]
fn from_token_accepts_aliases_case_and_whitespace() {
	assert_eq!(SelfReport::from_token("exploring"), Some(SelfReport::Exploring));
	assert_eq!(SelfReport::from_token("PROGRESSING"), Some(SelfReport::Progressing));
	assert_eq!(SelfReport::from_token("  Done  "), Some(SelfReport::Done));
	assert_eq!(SelfReport::from_token("need_input"), Some(SelfReport::NeedInput));
	assert_eq!(SelfReport::from_token("need-input"), Some(SelfReport::NeedInput));
	assert_eq!(SelfReport::from_token("needinput"), Some(SelfReport::NeedInput));
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
	let extra_field = r#"<sup>{"state":"done","focus":"f","next":null,"carry":[],"surprise":1}</sup>"#;
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
fn handoff_accepts_and_ignores_phase_complete_and_reassess_plan_signals() {
	for wire in ["phase_complete", "reassess"] {
		let text = format!(r#"<sup>{{"state":"progressing","focus":"f","next":"n","carry":[],"plan":"{wire}"}}</sup>"#);
		assert!(parse_self_report_handoff(&text).is_some(), "{wire}");
	}
}

#[test]
fn legacy_parse_tolerates_whitespace_and_pipe_separator() {
	let (state, reason) = parse_self_report("<sup> blocked | waiting on creds </sup>").expect("parses");
	assert_eq!(state, SelfReport::Blocked);
	assert_eq!(reason.as_deref(), Some("waiting on creds"));
}

#[test]
fn strip_removes_every_report_token_and_keeps_surrounding_text() {
	assert_eq!(strip_self_report("a<sup>done</sup>b<sup>exploring · why</sup>c"), "abc");
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
fn handoff_and_display_share_report_boundaries_with_malformed_framing() {
	let body =
		r#"{"state":"progressing","focus":"working","next":"check result","carry":[],"plan":null,"memories":[]}"#;
	for (open, close) in [
		("<sup>", "</sup>"),
		("<sub>", "</sup>"),
		("<sup>", "</sub>"),
		("<sub>", "</sub>"),
		("<sup>", ""),
		("<sub>", ""),
		("", ""),
	] {
		let text = format!("Answer.\n{open}{body}{close}");
		let report = parse_self_report_handoff(&text).expect("report body is valid");
		assert_eq!(report.state, SelfReport::Progressing);
		assert_eq!(report.handoff.focus, "working");
		assert_eq!(strip_self_report(&text), "Answer.");
	}
}

#[test]
fn truncated_or_invalid_reports_are_hidden_without_accepting_metadata() {
	for body in [
		r#"{"state":"progressing","focus":"private detail""#,
		r#"{"state":"unknown","focus":"private detail","next":null,"carry":[]}"#,
		r#"{"state":"done","focus":"private detail","next":null,"carry":[],"unexpected":true}"#,
	] {
		for (open, close) in [("<sup>", ""), ("<sub>", "</sup>")] {
			let text = format!("Answer.\n{open}{body}{close}");
			assert!(parse_self_report_handoff(&text).is_none());
			assert_eq!(strip_self_report(&text), "Answer.");
		}
	}
}

#[test]
fn report_tag_mentions_do_not_swallow_prose_before_mismatched_report() {
	let text = "Mention `<sup>` and `<sub>` in prose.\n<sub>{\"state\":\"done\",\"focus\":\"finished\",\"next\":null,\"carry\":[]}</sup>";
	assert_eq!(strip_self_report(text), "Mention `<sup>` and `<sub>` in prose.");
	assert_eq!(parse_self_report_handoff(text).expect("report").state, SelfReport::Done);
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
	assert_eq!(r, Some((SelfReport::Progressing, Some("editing api".into()))));
}

#[test]
fn parses_structured_handoff_and_strips_it() {
	let text = r#"answer
<sup>{"state":"progressing","focus":"inspect the active state","next":"continue from the last verified checkpoint","carry":["credential source is configured externally","retain opaque-run-ref"]}</sup>"#;
	let parsed = parse_self_report_handoff(text).expect("structured report");
	assert_eq!(parsed.state, SelfReport::Progressing);
	assert_eq!(parsed.handoff.focus, "inspect the active state");
	assert_eq!(parsed.handoff.next, "continue from the last verified checkpoint");
	assert_eq!(parsed.handoff.carry.len(), 2);
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
fn accepts_and_ignores_legacy_behavior_ids() {
	let parsed = parse_self_report_handoff(
			r#"answer
<sup>{"state":"progressing","focus":"used evolved skill","next":"continue","carry":[],"plan":null,"memories":["M2"],"behaviors":["evo-rust-123"]}</sup>"#,
		)
		.unwrap();
	assert_eq!(parsed.used_memories, vec!["M2"]);
}

#[test]
fn external_plan_signal_is_accepted_and_ignored() {
	let text = r#"<sup>{"state":"progressing","focus":"surveying sources","next":"compare findings","carry":[],"plan":"request"}</sup>"#;
	assert!(parse_self_report_handoff(text).is_some());
	assert!(parse_self_report_handoff(&text.replace("request", "nonsense")).is_none());
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
fn report_written_without_its_tag_parses_and_is_hidden() {
	// Prod 2026-10-25: the model ended its reply with the bare object.
	let bare = "On it.\n\n{\"state\":\"progressing\",\"focus\":\"retrying carousel 9\",\"next\":\"read the gate result\",\"carry\":[\"lock FREE\"],\"plan\":null,\"memories\":[\"M1\"]}";
	let parsed = parse_self_report_handoff(bare).expect("bare report");
	assert_eq!(parsed.state, SelfReport::Progressing);
	assert_eq!(parsed.handoff.focus, "retrying carousel 9");
	assert_eq!(parsed.used_memories, vec!["M1"]);
	assert_eq!(strip_self_report(bare), "On it.");

	// An opening tag the model never closed.
	let unclosed = "Done.\n<sup>{\"state\":\"done\",\"focus\":\"shipped\",\"next\":null,\"carry\":[]}";
	assert_eq!(
		parse_self_report_handoff(unclosed).map(|p| p.state),
		Some(SelfReport::Done)
	);
	assert_eq!(strip_self_report(unclosed), "Done.");

	// A reply that is nothing but the report shows nothing.
	assert_eq!(
		strip_self_report(r#"{"state":"exploring","focus":"reading","next":"x","carry":[]}"#),
		""
	);
}

#[test]
fn json_the_user_asked_for_is_never_taken_for_a_report() {
	for kept in [
		"Here it is:\n{\"state\":\"CA\",\"city\":\"Los Angeles\"}",
		"Status payload:\n{\"state\":\"done\"}",
		"{\"state\":\"done\",\"focus\":\"x\"}\nand prose after it",
		"x<sup>2",
	] {
		assert_eq!(strip_self_report(kept), kept, "stripped: {kept}");
		assert!(parse_self_report_handoff(kept).is_none(), "parsed: {kept}");
	}
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
	assert_eq!(reason.as_deref(), Some("Phase 1 complete, awaiting user direction"));
}

#[test]
fn handles_non_dot_separators() {
	assert_eq!(strip_self_report("x <sup>done: all good</sup>"), "x");
	assert_eq!(strip_self_report("x <sup>blocked - cannot proceed</sup>"), "x");
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
	assert_eq!(strip_self_report("the 5<sup>th</sup>"), "the 5<sup>th</sup>");
}

#[test]
fn strips_echoed_state_placeholder() {
	// The reported leak: a model copies the literal `STATE` placeholder.
	// It must never reach the screen, and we recover the intended state.
	assert_eq!(strip_self_report("answer\n<sup>STATE · done</sup>"), "answer");
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
	assert_eq!(strip_self_report("ok\n<sup>finished · all good</sup>"), "ok");
}

#[test]
fn prose_sup_mention_before_report_still_strips_report() {
	// The reported leak: prose mentioning `<sup>` paired forward with the
	// real report's `</sup>`, the merged span failed the shape check, and
	// the whole thing — report included — was kept verbatim.
	let text = r#"The `<sup>` token is hidden from the user.

<sup>{"state":"done","focus":"f","next":null,"carry":[]}</sup>"#;
	assert_eq!(strip_self_report(text), "The `<sup>` token is hidden from the user.");
}

#[test]
fn prose_sup_mention_with_separator_keeps_prose_and_strips_report() {
	// The merged span contained `·`, so the shape check passed and the
	// prose between the mention and the report was eaten with the report.
	let text = r#"see <sup> · docs <sup>{"state":"done","focus":"f","next":null,"carry":[]}</sup>"#;
	assert_eq!(strip_self_report(text), "see <sup> · docs");
}

#[test]
fn legitimate_superscript_before_report_keeps_markup() {
	let text = r#"x<sup>2</sup> <sup>{"state":"done","focus":"f","next":null,"carry":[]}</sup>"#;
	assert_eq!(strip_self_report(text), "x<sup>2</sup>");
}

// --- status line fields registered by plugins ---

use crate::plugins::PluginStatusField;

fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
	let start = text.find(from).expect("start marker");
	let end = text.find(to).expect("end marker") + to.len();
	&text[start..end]
}

fn field(
	name: &str,
	instruction: &str,
	default: serde_json::Value,
	example: Option<serde_json::Value>,
) -> PluginStatusField {
	PluginStatusField {
		plugin: "p".into(),
		name: name.into(),
		instruction: instruction.into(),
		default,
		example,
	}
}

fn builtin_supervisor_fields() -> Vec<PluginStatusField> {
	let plan = between(
		SELF_REPORT_INSTRUCTION,
		"`plan` is normally `null`.",
		"always rides on real work.",
	);
	let memories = between(
		SELF_REPORT_INSTRUCTION,
		"Put an active-memory ID",
		"Never list entries merely because they were shown.",
	);
	vec![
		field("plan", plan, json!(null), None),
		field("memories", memories, json!([]), Some(json!(["M2"]))),
	]
}

#[test]
fn composed_instruction_with_the_builtin_fields_equals_the_constant() {
	assert_eq!(
		status_instruction(&builtin_supervisor_fields()),
		SELF_REPORT_INSTRUCTION
	);
}

#[test]
fn base_instruction_has_no_plan_or_memories() {
	let base = status_instruction(&[]);
	assert!(base.starts_with("Finish every response with one compact JSON status line"));
	assert!(base.contains("\"carry\":[\"minimum fact or opaque reference needed after context loss\"]}</sup>"));
	assert!(base.contains("exactly those fields. `carry` may be empty and `next` is `null` when nothing remains to do. Keep only information"));
	assert!(base.contains("- `done` — the user's task is fully complete\nExample: "));
	assert!(base.ends_with("Emit exactly one."));
	assert!(!base.contains("plan") && !base.contains("memories"));
}

#[test]
fn custom_list_and_scalar_fields_land_in_template_sentence_paragraph_and_example() {
	let fields = [
		field("tags", "Name tags.", json!([]), None),
		field("risk", "Rate the risk.", json!("low"), Some(json!("high"))),
		field("refs", "Cite refs.", json!([]), Some(json!(["R1"]))),
	];
	let text = status_instruction(&fields);
	assert!(text.contains("\"carry\":[\"minimum fact or opaque reference needed after context loss\"],\"tags\":[],\"risk\":\"low\",\"refs\":[]}</sup>"));
	assert!(text.contains("`carry`, `tags` and `refs` may be empty and `next` is `null` when nothing remains to do. Name tags. Cite refs. Keep only information"));
	assert!(text.contains("- `done` — the user's task is fully complete\n\nRate the risk.\nExample: "));
	assert!(text.contains("tests/fixtures/tabs.md\"],\"tags\":[],\"risk\":\"high\",\"refs\":[\"R1\"]}</sup>`"));
}

async fn session_with_fields(fields: serde_json::Value) -> String {
	static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
	let id = format!(
		"detect-status-fields-{}",
		COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
	);
	let stub = crate::plugins::test_support::StubPlugin::new(&[]).init_field("status_fields", fields);
	crate::plugins::test_support::connect_stub(&id, "assistant", std::path::Path::new("."), &stub).await;
	id
}

const CUSTOM_LINE: &str = r#"<sup>{"state":"done","focus":"f","next":null,"carry":[],"risk":"low"}</sup>"#;

#[tokio::test]
async fn parser_returns_registered_fields_and_rejects_unregistered_keys() {
	let id = session_with_fields(json!([{"name": "risk", "instruction": "i", "default": "x"}])).await;
	crate::session::context::with_session_id(id.clone(), async {
		let parsed = parse_self_report_handoff(CUSTOM_LINE).expect("registered key accepted");
		assert_eq!(parsed.fields.get("risk"), Some(&json!("low")));
		assert_eq!(parsed.fields.len(), 1);
		assert_eq!(parsed.state, SelfReport::Done);

		let unregistered = CUSTOM_LINE.replace("risk", "danger");
		assert!(parse_self_report_handoff(&unregistered).is_none());

		let untagged = format!("answer\n{}", &CUSTOM_LINE[5..CUSTOM_LINE.len() - 6]);
		assert_eq!(strip_self_report(&untagged), "answer");
		assert!(parse_self_report_handoff(&untagged)
			.expect("untagged form")
			.fields
			.contains_key("risk"));
	})
	.await;
	crate::plugins::end_session(&id);
}

#[test]
fn outside_a_session_registered_keys_do_not_exist() {
	assert!(parse_self_report_handoff(CUSTOM_LINE).is_none());
	let untagged = &CUSTOM_LINE[5..CUSTOM_LINE.len() - 6];
	assert_eq!(
		strip_self_report(&format!("answer\n{untagged}")),
		format!("answer\n{untagged}")
	);
}

#[tokio::test]
async fn memories_stay_typed_and_plan_and_memories_also_appear_in_fields_when_registered() {
	let id = session_with_fields(json!([
		{"name": "plan", "instruction": "i", "default": null},
		{"name": "memories", "instruction": "i", "default": []}
	]))
	.await;
	let line =
		r#"<sup>{"state":"progressing","focus":"f","next":"n","carry":[],"plan":"request","memories":["M2"]}</sup>"#;
	crate::session::context::with_session_id(id.clone(), async {
		let parsed = parse_self_report_handoff(line).expect("parses");
		assert_eq!(parsed.used_memories, vec!["M2".to_string()]);
		assert_eq!(parsed.fields.get("plan"), Some(&json!("request")));
		assert_eq!(parsed.fields.get("memories"), Some(&json!(["M2"])));

		let bad_plan = line.replace("request", "nonsense");
		assert!(parse_self_report_handoff(&bad_plan).is_none());
	})
	.await;
	crate::plugins::end_session(&id);
}
