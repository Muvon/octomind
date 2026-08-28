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

#[test]
fn extract_json_lenient_parses_a_valid_object() {
	let v = extract_json_lenient(r#"{"should_compress": true, "reason": "long session"}"#)
		.expect("valid JSON parses");
	assert_eq!(v["should_compress"], true);
	assert_eq!(v["reason"], "long session");
}

#[test]
fn extract_json_lenient_parses_arrays() {
	let v = extract_json_lenient(r#"[{"a": 1}, {"b": 2}]"#).expect("array parses");
	assert_eq!(v.as_array().map(Vec::len), Some(2));
	assert_eq!(v[1]["b"], 2);
}

#[test]
fn extract_json_lenient_parses_nested_objects() {
	let v = extract_json_lenient(r#"{"outer": {"inner": [1, {"deep": true}]}}"#)
		.expect("nested JSON parses");
	assert_eq!(v["outer"]["inner"][1]["deep"], true);
}

#[test]
fn extract_json_lenient_returns_none_for_text_without_json() {
	assert!(extract_json_lenient("no json here, just prose").is_none());
}

#[test]
fn extract_json_lenient_returns_none_for_a_missing_closing_brace() {
	// Provider got cut off mid-object: the scanner must not hand back a
	// half-balanced fragment.
	assert!(extract_json_lenient(r#"{"should_compress": true"#).is_none());
}

#[test]
fn extract_json_lenient_returns_none_for_empty_input() {
	assert!(extract_json_lenient("").is_none());
	assert!(extract_json_lenient(" \n\t ").is_none());
}

fn summary_with_signal(signal: &str) -> CompressionSummary {
	CompressionSummary {
		current_task: signal.to_string(),
		..Default::default()
	}
}

#[test]
fn evaluate_decision_honours_the_models_veto() {
	let mut summary = summary_with_signal("finish the widget");
	summary.should_compress = false;
	assert!(!evaluate_decision(&summary, false, false));
}

#[test]
fn evaluate_decision_force_overrides_a_veto_but_not_the_substantive_guard() {
	// Forced compression grants the decision model no veto — but an empty
	// summary must still abort to avoid wiping the session.
	let empty = CompressionSummary::default();
	assert!(!evaluate_decision(&empty, true, false));

	let mut substantive = summary_with_signal("finish the widget");
	substantive.should_compress = false;
	assert!(evaluate_decision(&substantive, true, false));
}

#[test]
fn evaluate_decision_requires_a_substantive_summary_without_pact() {
	let empty = CompressionSummary {
		should_compress: true,
		..Default::default()
	};
	assert!(!evaluate_decision(&empty, false, false));

	let mut substantive = summary_with_signal("finish the widget");
	substantive.should_compress = true;
	assert!(evaluate_decision(&substantive, false, false));
}

#[test]
fn evaluate_decision_pact_bypasses_the_substantive_guard() {
	// PACT carries its own attributed evidence, so the narrative-field
	// heuristic must not veto it.
	let empty = CompressionSummary {
		should_compress: true,
		..Default::default()
	};
	assert!(evaluate_decision(&empty, false, true));
}
