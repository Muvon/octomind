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

//! Verify-gate evidence protocol: the readback round and the three-valued
//! shape verdict. Both exist to keep the gate from ruling on evidence it was
//! never shown — the failure mode where a search runs, the ledger records only
//! that it ran, and the verifier flags the enumeration it could have read.

use super::*;

const CLEAN_SHAPES: &str = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>"#;

/// The number the verifier reads in `<recorded_actions>` must be the number a
/// readback resolves. If these two ever drift the round silently answers about
/// the wrong call, which is worse than not answering at all.
#[test]
fn a_readback_resolves_the_number_the_rendered_ledger_shows() {
	let mut ledger = EvidenceLedger::default();
	let listing = ledger.record(
		"view",
		&serde_json::json!({"path":"src/llm/providers/"}),
		false,
		false,
		64,
	);
	ledger.record_ground(listing, "alibaba.rs\nbyteplus.rs\nzai.rs");
	let search = ledger.record(
		"search",
		&serde_json::json!({"pattern":"impl Provider"}),
		false,
		false,
		32,
	);
	ledger.record_ground(search, "28 matches in 27 files");

	let rendered = ledger.render();
	assert!(rendered.contains(&format!("#{listing} [read] view")));
	assert!(rendered.contains(&format!("#{search} [read] search")));

	let answered = render_readback(ledger.grounds(), &[listing]);
	assert!(answered.contains(&format!("<output seq=\"{listing}\" retained=\"yes\">")));
	assert!(answered.contains("byteplus.rs"));
	assert!(!answered.contains("28 matches"));
}

/// An unretained number is answered in words. Silence would read as "that call
/// returned nothing" — the inference the whole round exists to prevent.
#[test]
fn an_unretained_action_is_answered_explicitly() {
	let answered = render_readback(&[], &[7]);
	assert!(answered.contains(r#"<output seq="7" retained="no">"#));
	assert!(answered.contains("says nothing about what the action returned"));
}

#[test]
fn a_readback_request_is_a_response_mode_of_its_own() {
	let asking = r##"<readback seq="3">what the listing returned</readback>
<readback seq="#4">the member set</readback>"##;
	assert_eq!(text_report(asking).readback_request(), vec![3, 4]);

	// A reply that already ruled has spent its round; readback tags inside it
	// are narrative, not a request.
	let ruled =
		format!("{CLEAN_SHAPES}\n<readback seq=\"3\">ignored</readback>\n<verdict>PASS</verdict>");
	assert!(text_report(&ruled).readback_request().is_empty());
}

#[test]
fn a_readback_request_is_deduped_and_bounded() {
	let greedy: String = (0..10)
		.map(|n| format!("<readback seq=\"{n}\">n</readback>\n"))
		.collect();
	assert_eq!(text_report(&greedy).readback_request().len(), READBACK_MAX);
	let repeated = r#"<readback seq="5">a</readback><readback seq="5">b</readback>"#;
	assert_eq!(text_report(repeated).readback_request(), vec![5]);
}

/// A listing's members are at the head and a run's summary at the tail, so a
/// readback that kept one end would reintroduce the blindness it removes.
#[test]
fn a_long_output_is_read_back_from_both_ends() {
	let output = format!(
		"FIRST-MEMBER{}LAST-MEMBER",
		"x".repeat(READBACK_HEAD + READBACK_TAIL + 1_000)
	);
	let bounded = bounded_output(&output);
	assert!(bounded.starts_with("FIRST-MEMBER"));
	assert!(bounded.ends_with("LAST-MEMBER"));
	assert!(bounded.contains("elided from the middle"));
	assert!(bounded.chars().count() < output.chars().count());
}

/// The regression this file exists for: the verifier cannot see what a search
/// returned, says so, and the turn is NOT failed for it.
#[test]
fn an_unsettled_shape_reports_without_blocking() {
	let response = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="unknown">the search ran but its output is not in my input</shape>
<verdict>PASS</verdict>"#;
	assert_eq!(text_report(response).verdict(0), GateVerdict::Pass);
	let reported = text_report(response).reported_findings();
	assert_eq!(reported.len(), 1);
	assert!(reported[0].starts_with("unenumerated-category unsettled"));
}

/// A semantic suspicion is not a directly observed failure. This is the exact
/// class of false positive that previously turned a safe short-circuit bounds
/// check into a mandatory repair: the verifier proposed a rewrite without any
/// failing execution showing that the existing expression violated the task.
#[test]
fn an_unobserved_condition_suspicion_reports_without_blocking() {
	let response = r#"<condition n="1" status="unknown">the verifier suspects `i + 1 >= len` is wrong, but no recorded input demonstrates a failure and removing the guard may read out of bounds</condition>
<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>
<verdict>PASS</verdict>"#;
	let report = text_report(response);
	assert_eq!(report.verdict(1), GateVerdict::Pass);
	assert_eq!(report.reported_findings().len(), 1);
	assert!(report.reported_findings()[0].contains("condition 1 unsettled"));

	let answer = serde_json::json!({
		"conditions": [{
			"n": 1,
			"status": "unknown",
			"observation": "no recorded input demonstrates the suspected bounds failure"
		}],
		"shapes": clean_json_shapes(),
		"gaps": [],
		"verdict": "PASS",
		"readback": []
	});
	let report = json_report(&answer);
	assert_eq!(report.verdict(1), GateVerdict::Pass);
	assert_eq!(report.reported_findings().len(), 1);
}

/// The same false positive when the verifier does write "unmatched": the basis
/// says the observation is its own reading of the code, so the runtime reports
/// the suspicion and does not charge it — whatever the wording claims.
#[test]
fn an_inference_only_unmatched_condition_reports_without_blocking() {
	let response = format!(
		r#"<condition n="1" status="unmatched" basis="inference">`i + 1 >= len` short-circuits before the closing bracket is checked; the check should be rewritten</condition>
{CLEAN_SHAPES}
<verdict>PASS</verdict>"#
	);
	let report = text_report(&response);
	assert_eq!(report.verdict(1), GateVerdict::Pass);
	assert_eq!(report.reported_findings().len(), 1);
	assert!(report.reported_findings()[0].contains("condition 1 suspected by inference only"));
}

/// A violation the verifier was shown — here the absence of a called-for check
/// in the runtime log — is charged exactly as before.
#[test]
fn an_observed_unmatched_condition_is_charged() {
	let response = format!(
		r#"<condition n="1" status="unmatched" basis="absent_action">no recorded action ran the project's test suite</condition>
{CLEAN_SHAPES}
<gap settles="a run of the suite">tests never ran</gap>"#
	);
	assert_eq!(
		text_report(&response).verdict(1),
		GateVerdict::Gaps(vec![
			"Unmatched condition 1: no recorded action ran the project's test suite".into()
		])
	);
}

/// An unmatched condition with no basis, or an invented one, can be neither
/// charged nor excused: it is a protocol violation that gets the bounded
/// format retry — never a silent pass and never a silent gap.
#[test]
fn an_unmatched_condition_without_a_known_basis_is_indeterminate() {
	for attributes in ["", r#" basis="hunch""#] {
		let response = format!(
			r#"<condition n="1" status="unmatched"{attributes}>the function still returns a pair</condition>
{CLEAN_SHAPES}
<verdict>PASS</verdict>"#
		);
		assert!(
			matches!(
				text_report(&response).verdict(1),
				GateVerdict::Indeterminate(_)
			),
			"attributes {attributes:?}"
		);
	}
}

/// The JSON path applies the same basis rule; `basis` is null on every entry
/// that is not unmatched.
#[test]
fn the_json_path_reports_an_inference_only_unmatched_condition_without_blocking() {
	let answer = serde_json::json!({
		"conditions": [
			{"n": 1, "status": "matched", "observation": "suite ran green", "basis": null},
			{"n": 2, "status": "unmatched", "observation": "the bounds check looks wrong", "basis": "inference"}
		],
		"shapes": clean_json_shapes(),
		"gaps": [],
		"verdict": "PASS",
		"readback": []
	});
	let report = json_report(&answer);
	assert_eq!(report.verdict(2), GateVerdict::Pass);
	assert_eq!(report.reported_findings().len(), 1);
	assert!(report.reported_findings()[0].contains("condition 2 suspected by inference only"));
}

/// The refutation pass can only remove findings: one the second verifier marks
/// refuted is dropped, one it marks stands (or never mentions) is kept, a
/// number outside the list is ignored, and an off-protocol answer drops nothing.
#[test]
fn refutation_drops_only_findings_the_second_verifier_refuted() {
	let gaps = vec!["a".to_string(), "b".to_string(), "c".to_string()];
	let text = r#"<finding n="1" verdict="refuted">#4 ran the suite green</finding>
<finding n="2" verdict="stands">no listing recorded</finding>
<finding n="9" verdict="refuted">out of range</finding>"#;
	let (standing, refuted) = split_refuted(&gaps, &refuted_from_text(text));
	assert_eq!(standing, vec!["b".to_string(), "c".to_string()]);
	assert_eq!(refuted, vec!["a".to_string()]);

	let json = serde_json::json!({"findings": [
		{"n": 3, "verdict": "refuted", "citation": "the diff hunk contains the guard"},
		{"n": "1", "verdict": "stands", "citation": "no action ran it"}
	]});
	let (standing, refuted) = split_refuted(&gaps, &refuted_from_json(&json));
	assert_eq!(standing, vec!["a".to_string(), "b".to_string()]);
	assert_eq!(refuted, vec!["c".to_string()]);

	assert!(refuted_from_text("I think it is fine").is_empty());
	assert!(refuted_from_json(&serde_json::json!({})).is_empty());
}

/// An accusation no action can close cannot be repaired — re-running only
/// spends the budget to arrive at the same verdict. It is not silently dropped
/// either: whatever the runtime declines to charge, the user sees.
#[test]
fn a_finding_with_no_settling_observation_is_reported_not_charged() {
	let response = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="yes">the set is not bounded</shape>
<verdict>PASS</verdict>"#;
	assert_eq!(text_report(response).verdict(0), GateVerdict::Pass);
	let reported = text_report(response).reported_findings();
	assert_eq!(reported.len(), 1);
	assert!(reported[0].contains("names no closing observation"));
}

/// The same bar on a free-form gap: without it, an unanswerable finding could
/// still enter the repair loop through the one channel the shapes do not cover.
#[test]
fn a_free_form_gap_answers_to_the_same_rule() {
	let unanswerable =
		format!("{CLEAN_SHAPES}\n<gap>the set is not bounded</gap>\n<verdict>PASS</verdict>");
	assert_eq!(text_report(&unanswerable).verdict(0), GateVerdict::Pass);
	let reported = text_report(&unanswerable).reported_findings();
	assert_eq!(reported.len(), 1);
	assert!(reported[0].contains("gap names no closing observation"));

	let answerable = format!(
		"{CLEAN_SHAPES}\n<gap settles=\"a read of stats.rs\">the counter is unverified</gap>"
	);
	let GateVerdict::Gaps(gaps) = text_report(&answerable).verdict(0) else {
		panic!("a gap naming its observation is charged");
	};
	assert_eq!(
		gaps,
		["the counter is unverified — clear it by: a read of stats.rs"]
	);
	assert!(text_report(&answerable).reported_findings().is_empty());
}

#[test]
fn a_settled_finding_carries_the_observation_that_would_clear_it() {
	let response = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="yes" settles="a listing of src/llm/providers naming every member">only touched members are covered</shape>"#;
	let GateVerdict::Gaps(gaps) = text_report(response).verdict(0) else {
		panic!("a settled finding is a gap");
	};
	assert_eq!(gaps.len(), 1);
	assert!(gaps[0].contains("clear it by: a listing of src/llm/providers naming every member"));
}

#[test]
fn a_shape_value_outside_the_contract_is_indeterminate() {
	let response = r#"<shape name="circular" found="maybe">unsure</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>
<verdict>PASS</verdict>"#;
	assert!(matches!(
		text_report(response).verdict(0),
		GateVerdict::Indeterminate(_)
	));
}

#[test]
fn an_unchanged_finding_is_recognized_across_rewording_of_order_and_case() {
	let prior = vec![
		"Evidence shape 'unenumerated-category' present:  the set   is not bounded".to_string(),
		"Unmatched condition 2: no listing".to_string(),
	];
	let same = vec![
		"unmatched condition 2: no listing".to_string(),
		"evidence shape 'unenumerated-category' present: the set is not bounded".to_string(),
	];
	assert!(gaps_unchanged(&prior, &same));

	// A genuinely different finding, a different count, or a first pass with
	// nothing to compare against all leave the ordinary bounded retry in charge.
	assert!(!gaps_unchanged(&prior, &[same[0].clone()]));
	assert!(!gaps_unchanged(&[], &same));
	assert!(!gaps_unchanged(
		&prior,
		&[same[0].clone(), "a different finding entirely".to_string()]
	));
}

/// The gate loop distinguishes a re-run that gathered evidence from one that
/// only reworded its answer; without that, an ignored advisory would look the
/// same as an unsatisfiable check.
#[test]
fn the_ledger_measures_what_a_re_run_added() {
	let mut ledger = EvidenceLedger::default();
	ledger.record("view", &serde_json::json!({"path":"a"}), false, false, 8);
	assert_eq!(ledger.actions_since_gate(), 1);
	ledger.mark_gate_checkpoint();
	assert_eq!(ledger.actions_since_gate(), 0);
	ledger.record(
		"search",
		&serde_json::json!({"pattern":"b"}),
		false,
		false,
		8,
	);
	assert_eq!(ledger.actions_since_gate(), 1);
	ledger.reset();
	assert_eq!(ledger.actions_since_gate(), 0);
}

/// The four shapes, all absent — the JSON twin of [`CLEAN_SHAPES`].
fn clean_json_shapes() -> Vec<serde_json::Value> {
	REQUIRED_SHAPES
		.iter()
		.map(|name| {
			serde_json::json!({
				"name": name,
				"found": "no",
				"reason": "not present",
				"settles": null
			})
		})
		.collect()
}

/// The defect this bound exists for: an unverifiable verdict used to fall
/// through as a completed turn. It now spends the same bounded re-entry a
/// substantive gap does — the turn may finish once the budget is out, but never
/// as verified work.
#[test]
fn an_unverifiable_verdict_spends_a_bounded_re_entry() {
	const MAX_ITERATIONS: u8 = 3;
	let advisory = unverified_reentry(1, MAX_ITERATIONS).expect("budget remains");
	assert!(advisory.contains("could not be completed"));
	assert!(advisory.contains("numbered list of the conditions"));
	assert!(advisory.contains("the observation that satisfies it"));
	// It asks for evidence; it never charges the agent with a finding.
	assert!(!advisory.contains("gap"));
	assert!(unverified_reentry(MAX_ITERATIONS - 1, MAX_ITERATIONS).is_some());
	// The bound is the gate's own iteration budget, spent like any other pass.
	assert!(unverified_reentry(MAX_ITERATIONS, MAX_ITERATIONS).is_none());
	assert!(unverified_reentry(MAX_ITERATIONS + 1, MAX_ITERATIONS).is_none());
	assert!(unverified_reentry(0, 0).is_none());
}

/// The JSON path derives the verdict from the same checklist: an itemized
/// condition left unmatched is a gap even when the answer's own verdict is PASS.
#[test]
fn the_json_path_charges_an_unmatched_condition_over_a_holistic_pass() {
	let answer = serde_json::json!({
		"conditions": [
			{"n": 1, "status": "matched", "observation": "suite ran green"},
			{"n": 2, "status": "unmatched", "observation": "no test shows custom prettifier output preserved", "basis": "absent_action"}
		],
		"shapes": clean_json_shapes(),
		"gaps": [],
		"verdict": "PASS",
		"readback": []
	});
	assert_eq!(
		json_report(&answer).verdict(2),
		GateVerdict::Gaps(vec![
			"Unmatched condition 2: no test shows custom prettifier output preserved".into()
		])
	);
}

/// All four evidence shapes are required of both encodings. A JSON answer
/// missing one is a protocol violation, never a pass.
#[test]
fn the_json_path_rejects_a_missing_evidence_shape() {
	let mut shapes = clean_json_shapes();
	shapes.pop();
	let answer = serde_json::json!({
		"conditions": [],
		"shapes": shapes,
		"gaps": [],
		"verdict": "PASS",
		"readback": []
	});
	assert!(matches!(
		json_report(&answer).verdict(0),
		GateVerdict::Indeterminate(reason) if reason.contains("incomplete evidence-shape checklist")
	));
	// A shape claimed twice is the same violation from the other side.
	let mut duplicated = clean_json_shapes();
	duplicated[3] = duplicated[0].clone();
	let answer = serde_json::json!({
		"conditions": [],
		"shapes": duplicated,
		"gaps": [],
		"verdict": "PASS",
		"readback": []
	});
	assert!(matches!(
		json_report(&answer).verdict(0),
		GateVerdict::Indeterminate(_)
	));
}

/// Equivalent content must reach an identical verdict in either encoding — a
/// provider's wire format is not allowed to change what the gate concludes.
#[test]
fn both_encodings_reach_the_same_verdict() {
	let charged_text = format!(
		"{CLEAN_SHAPES}\n<gap settles=\"a read of stats.rs\">the counter is unverified</gap>"
	);
	let charged_json = serde_json::json!({
		"conditions": [],
		"shapes": clean_json_shapes(),
		"gaps": [{"gap": "the counter is unverified", "settles": "a read of stats.rs"}],
		"verdict": "GAPS",
		"readback": []
	});
	assert_eq!(
		text_report(&charged_text).verdict(0),
		json_report(&charged_json).verdict(0)
	);

	// And a finding that names no closing observation is reported, not charged,
	// on both paths.
	let unactionable_text =
		format!("{CLEAN_SHAPES}\n<gap>the set is not bounded</gap>\n<verdict>PASS</verdict>");
	let unactionable_json = serde_json::json!({
		"conditions": [],
		"shapes": clean_json_shapes(),
		"gaps": [{"gap": "the set is not bounded", "settles": null}],
		"verdict": "PASS",
		"readback": []
	});
	assert_eq!(
		text_report(&unactionable_text).verdict(0),
		GateVerdict::Pass
	);
	assert_eq!(
		json_report(&unactionable_json).verdict(0),
		GateVerdict::Pass
	);
	assert_eq!(
		text_report(&unactionable_text).reported_findings(),
		json_report(&unactionable_json).reported_findings()
	);
}

/// The readback round survives the JSON encoding: a request-only answer asks,
/// a ruled answer has already spent its round.
#[test]
fn the_json_path_keeps_the_readback_round() {
	let asking = serde_json::json!({
		"conditions": [],
		"shapes": [],
		"gaps": [],
		"verdict": "READBACK",
		"readback": [
			{"seq": 3, "need": "what the listing returned"},
			{"seq": 3, "need": "the same call again"},
			{"seq": 4, "need": "the member set"}
		]
	});
	assert_eq!(json_report(&asking).readback_request(), vec![3, 4]);

	let ruled = serde_json::json!({
		"conditions": [],
		"shapes": clean_json_shapes(),
		"gaps": [],
		"verdict": "PASS",
		"readback": [{"seq": 3, "need": "ignored"}]
	});
	assert!(json_report(&ruled).readback_request().is_empty());
}

/// The schema and the checklist are one contract; drift between them would let
/// a schema-enforced answer be structurally valid and still unreadable.
#[test]
fn the_schema_asks_for_the_protocol_the_checklist_enforces() {
	let schema = build_gate_schema(2);
	let properties = &schema["properties"];
	assert_eq!(
		properties["shapes"]["items"]["properties"]["name"]["enum"],
		serde_json::json!(REQUIRED_SHAPES)
	);
	assert_eq!(properties["conditions"]["maxItems"].as_u64(), Some(2));
	assert_eq!(
		properties["readback"]["maxItems"].as_u64(),
		Some(READBACK_MAX as u64)
	);
	// Strict mode rejects a partial object: every field is required, so an
	// enforced answer can never omit the checklist.
	assert_eq!(
		schema["required"],
		serde_json::json!(["conditions", "shapes", "gaps", "verdict", "readback"])
	);
}

// ---------------------------------------------------------------------------
// record_ground: retention bounds for retained tool output.
// ---------------------------------------------------------------------------

#[test]
fn record_ground_ignores_empty_output() {
	let mut ledger = EvidenceLedger::default();
	ledger.record_ground(1, "");
	assert!(ledger.grounds().is_empty(), "empty output records nothing");
}

#[test]
fn record_ground_bounds_a_single_oversized_output() {
	let mut ledger = EvidenceLedger::default();
	let huge: String = "x".repeat(CITATION_GROUNDS_CHARS + 100);
	ledger.record_ground(7, &huge);
	let grounds = ledger.grounds();
	assert_eq!(grounds.len(), 1);
	assert_eq!(grounds[0].0, 7);
	assert_eq!(
		grounds[0].1.chars().count(),
		CITATION_GROUNDS_CHARS,
		"one output alone is clipped to the cap, never dropped"
	);
}

#[test]
fn record_ground_evicts_the_oldest_once_the_total_exceeds_the_cap() {
	let mut ledger = EvidenceLedger::default();
	let three_quarters: String = "a".repeat(CITATION_GROUNDS_CHARS * 3 / 4);
	ledger.record_ground(1, &three_quarters);
	ledger.record_ground(2, &three_quarters);
	let grounds = ledger.grounds();
	assert_eq!(grounds.len(), 1, "the oldest ground is evicted, not the newest");
	assert_eq!(grounds[0].0, 2);
}

// ---------------------------------------------------------------------------
// verdict(): protocol violations the checklist rejects in both encodings.
// ---------------------------------------------------------------------------

#[test]
fn a_shape_without_a_name_is_indeterminate() {
	let resp = format!(r#"<shape found="no">nameless</shape>{CLEAN_SHAPES}"#);
	assert!(matches!(
		text_report(&resp).verdict(0),
		GateVerdict::Indeterminate(reason) if reason.contains("shape without name")
	));
}

#[test]
fn a_condition_without_a_numeric_index_is_indeterminate() {
	let resp =
		format!(r#"{CLEAN_SHAPES}<condition status="matched">ok</condition><verdict>PASS</verdict>"#);
	assert!(matches!(
		text_report(&resp).verdict(1),
		GateVerdict::Indeterminate(reason) if reason.contains("condition without numeric index")
	));
}

#[test]
fn a_duplicate_condition_index_is_indeterminate() {
	let resp = format!(
		r#"{CLEAN_SHAPES}<condition n="1" status="matched">a</condition><condition n="1" status="matched">b</condition><verdict>PASS</verdict>"#
	);
	assert!(matches!(
		text_report(&resp).verdict(1),
		GateVerdict::Indeterminate(reason) if reason.contains("duplicate condition: 1")
	));
}

#[test]
fn a_condition_status_outside_the_contract_is_indeterminate() {
	let resp =
		format!(r#"{CLEAN_SHAPES}<condition n="1" status="bogus">x</condition><verdict>PASS</verdict>"#);
	assert!(matches!(
		text_report(&resp).verdict(1),
		GateVerdict::Indeterminate(reason) if reason.contains("condition 1 has invalid status")
	));
}

#[test]
fn a_condition_checklist_mismatch_is_indeterminate() {
	let resp =
		format!(r#"{CLEAN_SHAPES}<condition n="1" status="matched">only one</condition><verdict>PASS</verdict>"#);
	assert!(matches!(
		text_report(&resp).verdict(2),
		GateVerdict::Indeterminate(reason) if reason
			.contains("condition checklist mismatch: expected 2, received 1")
	));
}

#[test]
fn a_ruled_answer_with_no_verdict_marker_is_indeterminate() {
	// All four shapes clean, conditions match (none expected), no gap charged —
	// but the answer never said PASS. That is unreadable, not accepted.
	assert!(matches!(
		text_report(CLEAN_SHAPES).verdict(0),
		GateVerdict::Indeterminate(reason) if reason.contains("missing verdict markers")
	));
}

// ---------------------------------------------------------------------------
// The one element scanner every tag of the protocol passes through.
// ---------------------------------------------------------------------------

#[test]
fn the_element_scanner_drops_unterminated_and_unclosed_tags() {
	assert!(elements("no tags at all", "shape").is_empty());
	assert!(
		elements("<shape", "shape").is_empty(),
		"an open tag with no '>' is dropped"
	);
	assert!(
		elements(r#"<shape name="circular""#, "shape").is_empty(),
		"attributes without a closing bracket are dropped"
	);
	assert!(
		elements("<shape>body", "shape").is_empty(),
		"an element that is never closed is dropped"
	);
	let found = elements(r#"<shape name="circular">body</shape>"#, "shape");
	assert_eq!(found.len(), 1);
	assert_eq!(found[0].0.trim(), r#"name="circular""#);
	assert_eq!(found[0].1, "body");
}

// ---------------------------------------------------------------------------
// Refutation pass helpers.
// ---------------------------------------------------------------------------

#[test]
fn the_refutation_schema_bounds_findings_to_the_charged_count() {
	let schema = refute_schema(2);
	assert_eq!(schema["properties"]["findings"]["maxItems"], 2);
	assert_eq!(
		schema["properties"]["findings"]["items"]["properties"]["verdict"]["enum"],
		serde_json::json!(["stands", "refuted"])
	);
	assert_eq!(
		schema["properties"]["findings"]["items"]["required"],
		serde_json::json!(["n", "verdict", "citation"])
	);
	assert_eq!(schema["required"], serde_json::json!(["findings"]));
}

#[test]
fn refuted_numbers_decode_from_json_including_string_numbers() {
	let value = serde_json::json!({"findings": [
		{"n": 1, "verdict": "refuted", "citation": "cited"},
		{"n": "2", "verdict": "refuted", "citation": "cited"},
		{"n": 3, "verdict": "stands", "citation": "cited"},
		{"n": true, "verdict": "refuted", "citation": "not a number"}
	]});
	let refuted = refuted_from_json(&value);
	assert!(refuted.contains(&1), "plain number");
	assert!(refuted.contains(&2), "number written as a string");
	assert!(!refuted.contains(&3), "a standing finding is not refuted");
	assert_eq!(refuted.len(), 2, "non-numeric entries are dropped");
}

#[test]
fn split_refuted_preserves_order_and_ignores_unknown_numbers() {
	let gaps = vec!["one".to_string(), "two".to_string(), "three".to_string()];
	let refuted = std::collections::HashSet::from([2usize, 9usize]);
	let (standing, dropped) = split_refuted(&gaps, &refuted);
	assert_eq!(standing, vec!["one".to_string(), "three".to_string()]);
	assert_eq!(dropped, vec!["two".to_string()]);
}

// ---------------------------------------------------------------------------
// Wire-mode selection and the charge rule.
// ---------------------------------------------------------------------------

#[test]
fn an_unresolvable_model_keeps_the_text_wire_mode() {
	assert!(matches!(Encoding::for_model("nope:nope"), Encoding::Text));
	assert_eq!(Encoding::for_model("nope:nope").as_str(), "text");
	assert_eq!(Encoding::Json.as_str(), "json");
	assert_eq!(Encoding::Text.as_str(), "text");
}

#[test]
fn a_finding_is_charged_only_when_it_names_a_closing_observation() {
	assert_eq!(charged("", "gap text"), None);
	assert_eq!(
		charged("do X", "gap text").as_deref(),
		Some("gap text — clear it by: do X")
	);
}

#[test]
fn a_follow_up_without_context_sources_names_them_unspecified() {
	let rendered = render_gate_input(&GateInput {
		original_task: "and the second part?",
		task: "Apply the earlier decision to the config",
		task_scope: crate::supervisor::resolve::ResolutionScope::FollowUp,
		context_sources: &[],
		resolution_evidence: &[],
		result: "applied",
		claim: None,
		actions: "",
		grounds: &[],
		plan: "",
		ground_truth: "",
		prior_gaps: &[],
		role_context: "",
		evidence_conditions: &[],
	});
	assert!(
		rendered.contains(r#"sources="unspecified""#),
		"an empty source list must be explicit, not silently omitted"
	);
}

// ---------------------------------------------------------------------------
// verify(): the full round trip against the scripted fake provider.
// ---------------------------------------------------------------------------

mod verify_round_trip {
	use super::*;
	use crate::session::chat::test_support::{
		fake_provider_config, final_response, spawn_stub, spawn_stub_with_status, ENV_LOCK,
	};

	fn gate_config() -> Config {
		let mut config = fake_provider_config();
		config.supervisor.gate.verifier_model = "ollama:fake-model".to_string();
		config.supervisor.model = "ollama:fake-model".to_string();
		config
	}

	fn pass_answer() -> String {
		format!("{CLEAN_SHAPES}\n<verdict>PASS</verdict>")
	}

	fn input<'a>(grounds: &'a [(u64, String)], conditions: &'a [String]) -> GateInput<'a> {
		GateInput {
			original_task: "ship the feature",
			task: "ship the feature",
			task_scope: crate::supervisor::resolve::ResolutionScope::SelfContained,
			context_sources: &[],
			resolution_evidence: &[],
			result: "the feature is shipped and tested",
			claim: Some("done: tests pass"),
			actions: "[read] view src/main.rs → ok",
			grounds,
			plan: "",
			ground_truth: "",
			prior_gaps: &[],
			role_context: "",
			evidence_conditions: conditions,
		}
	}

	fn rx() -> (tokio::sync::watch::Sender<bool>, tokio::sync::watch::Receiver<bool>) {
		tokio::sync::watch::channel(false)
	}

	#[tokio::test]
	async fn an_empty_task_or_result_never_reaches_a_model() {
		let (_tx, rx1) = rx();
		let mut empty_task = input(&[], &[]);
		empty_task.original_task = "";
		empty_task.task = "   ";
		let verdict = verify(&gate_config(), empty_task, rx1).await;
		assert!(matches!(
			verdict,
			GateVerdict::Indeterminate(reason) if reason.contains("empty task or result")
		));
		let (_tx, rx2) = rx();
		let mut empty_result = input(&[], &[]);
		empty_result.result = "";
		let verdict = verify(&gate_config(), empty_result, rx2).await;
		assert!(matches!(
			verdict,
			GateVerdict::Indeterminate(reason) if reason.contains("empty task or result")
		));
	}

	#[tokio::test]
	async fn an_unresolvable_verifier_model_is_an_explicit_indeterminate() {
		let (_tx, rx) = rx();
		let mut config = gate_config();
		config.supervisor.gate.verifier_model = "nope:nope".to_string();
		let verdict = verify(&config, input(&[], &[]), rx).await;
		assert!(
			matches!(verdict, GateVerdict::Indeterminate(_)),
			"a transport failure must never masquerade as verification: {verdict:?}"
		);
	}

	#[tokio::test]
	async fn a_clean_pass_answer_verifies_the_turn() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub(vec![final_response(&pass_answer())]).await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&[], &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert_eq!(verdict, GateVerdict::Pass);
	}

	#[tokio::test]
	async fn a_readback_request_gets_one_answered_round_before_the_verdict() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub(vec![
			final_response(r#"<readback seq="1">what the search actually returned</readback>"#),
			final_response(&pass_answer()),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let grounds = vec![(1u64, "the search output".to_string())];
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&grounds, &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert_eq!(
			verdict,
			GateVerdict::Pass,
			"the readback round must resolve from retained grounds and then rule"
		);
	}

	#[tokio::test]
	async fn a_readback_round_that_cannot_be_answered_is_indeterminate() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub_with_status(vec![
			(200, final_response(r#"<readback seq="1">need the output</readback>"#)),
			(500, serde_json::json!({"error": "verifier unavailable"})),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let grounds = vec![(1u64, "the search output".to_string())];
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&grounds, &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert!(matches!(verdict, GateVerdict::Indeterminate(_)));
	}

	#[tokio::test]
	async fn one_malformed_answer_gets_a_single_format_repair() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub(vec![
			final_response(r#"<shape name="circular" found="no">only one shape</shape>"#),
			final_response(&pass_answer()),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&[], &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert_eq!(
			verdict,
			GateVerdict::Pass,
			"a structurally malformed answer gets exactly one bounded retry"
		);
	}

	#[tokio::test]
	async fn a_failed_format_repair_leaves_the_verdict_indeterminate() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub_with_status(vec![
			(200, final_response("I looked at it and it seems fine.")),
			(500, serde_json::json!({"error": "verifier unavailable"})),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&[], &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert!(matches!(
			verdict,
			GateVerdict::Indeterminate(reason) if reason.contains("incomplete evidence-shape checklist")
		));
	}

	#[tokio::test]
	async fn a_refuted_gap_is_cleared_by_the_second_verifier() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub(vec![
			final_response(&format!(
				r#"{CLEAN_SHAPES}<gap settles="run the suite">no test run demonstrates the fix</gap>"#
			)),
			final_response(
				r#"<finding n="1" verdict="refuted">the ledger records the suite passing</finding>"#,
			),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&[], &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert_eq!(
			verdict,
			GateVerdict::Pass,
			"a finding the refuter clears must not cost a re-run"
		);
	}

	#[tokio::test]
	async fn a_gap_the_refuter_upholds_stands_and_blocks() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub(vec![
			final_response(&format!(
				r#"{CLEAN_SHAPES}<gap settles="run the suite">no test run demonstrates the fix</gap>"#
			)),
			final_response(r#"<finding n="1" verdict="stands">nothing in the input shows a run"#),
		])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&[], &[]), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert_eq!(
			verdict,
			GateVerdict::Gaps(vec![
				"no test run demonstrates the fix — clear it by: run the suite".to_string()
			])
		);
	}

	#[tokio::test]
	async fn an_unknown_condition_is_reported_without_blocking_a_pass() {
		let _guard = ENV_LOCK.lock().await;
		let url = spawn_stub(vec![final_response(&format!(
			r#"{CLEAN_SHAPES}<condition n="1" status="unknown" observation="the log is not retained">cannot decide</condition><verdict>PASS</verdict>"#
		))])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);
		let conditions = vec!["the log shows the fix".to_string()];
		let (_tx, rx) = rx();
		let verdict = verify(&gate_config(), input(&[], &conditions), rx).await;
		std::env::remove_var("OLLAMA_API_URL");
		assert_eq!(
			verdict,
			GateVerdict::Pass,
			"an unsettled condition is a limit of the input, never a defect in the work"
		);
	}
}
