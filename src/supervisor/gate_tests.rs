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
	assert_eq!(parse_readback_request(asking), vec![3, 4]);

	// A reply that already ruled has spent its round; readback tags inside it
	// are narrative, not a request.
	let ruled =
		format!("{CLEAN_SHAPES}\n<readback seq=\"3\">ignored</readback>\n<verdict>PASS</verdict>");
	assert!(parse_readback_request(&ruled).is_empty());
}

#[test]
fn a_readback_request_is_deduped_and_bounded() {
	let greedy: String = (0..10)
		.map(|n| format!("<readback seq=\"{n}\">n</readback>\n"))
		.collect();
	assert_eq!(parse_readback_request(&greedy).len(), READBACK_MAX);
	let repeated = r#"<readback seq="5">a</readback><readback seq="5">b</readback>"#;
	assert_eq!(parse_readback_request(repeated), vec![5]);
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
	assert_eq!(parse_verdict(response, 0), GateVerdict::Pass);
	let reported = reported_findings(response);
	assert_eq!(reported.len(), 1);
	assert!(reported[0].starts_with("unenumerated-category unsettled"));
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
	assert_eq!(parse_verdict(response, 0), GateVerdict::Pass);
	let reported = reported_findings(response);
	assert_eq!(reported.len(), 1);
	assert!(reported[0].contains("names no closing observation"));
}

/// The same bar on a free-form gap: without it, an unanswerable finding could
/// still enter the repair loop through the one channel the shapes do not cover.
#[test]
fn a_free_form_gap_answers_to_the_same_rule() {
	let unanswerable =
		format!("{CLEAN_SHAPES}\n<gap>the set is not bounded</gap>\n<verdict>PASS</verdict>");
	assert_eq!(parse_verdict(&unanswerable, 0), GateVerdict::Pass);
	let reported = reported_findings(&unanswerable);
	assert_eq!(reported.len(), 1);
	assert!(reported[0].contains("gap names no closing observation"));

	let answerable = format!(
		"{CLEAN_SHAPES}\n<gap settles=\"a read of stats.rs\">the counter is unverified</gap>"
	);
	let GateVerdict::Gaps(gaps) = parse_verdict(&answerable, 0) else {
		panic!("a gap naming its observation is charged");
	};
	assert_eq!(
		gaps,
		["the counter is unverified — clear it by: a read of stats.rs"]
	);
	assert!(reported_findings(&answerable).is_empty());
}

#[test]
fn a_settled_finding_carries_the_observation_that_would_clear_it() {
	let response = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="yes" settles="a listing of src/llm/providers naming every member">only touched members are covered</shape>"#;
	let GateVerdict::Gaps(gaps) = parse_verdict(response, 0) else {
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
		parse_verdict(response, 0),
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
