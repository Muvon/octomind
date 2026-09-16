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

//! The `defer_reason` wire contract on both response paths. The field is
//! advisory: a provider that omits, nulls, mistypes, or invents the token must
//! never abort compression, and every degradation must land on `none` — a
//! claim that vetoes nothing and therefore cannot stall a fold.

use super::*;

fn veto_xml(defer_tag: &str) -> String {
	format!(
		"<should_compress>false</should_compress>\n{defer_tag}\n<current_task>finish the thing</current_task>"
	)
}

#[test]
fn schema_declares_defer_reason_as_a_required_enum() {
	// The wire enum and the runtime enum must not drift: the model is only ever
	// asked for tokens `DeferReason::from_token` can name.
	for schema in [
		build_compression_schema(false, false),
		build_compression_schema(false, true),
		build_compression_schema(true, false),
	] {
		assert_eq!(
			schema["properties"]["defer_reason"]["enum"],
			serde_json::json!([
				"none",
				"mid_derivation",
				"verification_in_flight",
				"stuck",
				"transcript_minimal"
			])
		);
		assert!(schema["required"]
			.as_array()
			.expect("required is an array")
			.iter()
			.any(|field| field == "defer_reason"));
	}
}

#[test]
fn json_defer_reason_parses_every_known_token() {
	for (token, expected) in [
		("none", DeferReason::None),
		("mid_derivation", DeferReason::MidDerivation),
		("verification_in_flight", DeferReason::VerificationInFlight),
		("stuck", DeferReason::Stuck),
		("transcript_minimal", DeferReason::TranscriptMinimal),
		("  MID_DERIVATION  ", DeferReason::MidDerivation),
	] {
		let summary: CompressionSummary =
			serde_json::from_value(serde_json::json!({ "defer_reason": token }))
				.expect("tolerant deserialization");
		assert_eq!(summary.defer_reason, expected, "token: {token}");
	}
}

#[test]
fn json_defer_reason_tolerates_missing_null_unknown_and_wrong_type() {
	for value in [
		serde_json::json!({}),
		serde_json::json!({ "defer_reason": null }),
		serde_json::json!({ "defer_reason": "because_i_said_so" }),
		serde_json::json!({ "defer_reason": 7 }),
		serde_json::json!({ "defer_reason": { "reason": "stuck" } }),
		serde_json::json!({ "defer_reason": ["stuck"] }),
	] {
		let summary: CompressionSummary =
			serde_json::from_value(value.clone()).expect("advisory field never aborts parsing");
		assert_eq!(summary.defer_reason, DeferReason::None, "value: {value}");
	}
}

#[test]
fn xml_defer_reason_parses_known_tokens_and_degrades_unknown_ones() {
	assert_eq!(
		parse_xml_summary(&veto_xml(
			"<defer_reason>verification_in_flight</defer_reason>"
		))
		.expect("parses")
		.defer_reason,
		DeferReason::VerificationInFlight
	);

	for tag in [
		"<defer_reason>because_i_said_so</defer_reason>",
		"<defer_reason>  </defer_reason>",
		"<defer_reason>mid_derivation</defer_reasonX>",
		"",
	] {
		let summary = parse_xml_summary(&veto_xml(tag)).expect("parses");
		assert!(!summary.should_compress);
		assert_eq!(summary.defer_reason, DeferReason::None, "tag: {tag}");
	}
}

#[test]
fn claims_step_in_flight_covers_exactly_the_two_in_flight_reasons() {
	assert!(DeferReason::MidDerivation.claims_step_in_flight());
	assert!(DeferReason::VerificationInFlight.claims_step_in_flight());
	assert!(!DeferReason::Stuck.claims_step_in_flight());
	assert!(!DeferReason::TranscriptMinimal.claims_step_in_flight());
	assert!(!DeferReason::None.claims_step_in_flight());
}
