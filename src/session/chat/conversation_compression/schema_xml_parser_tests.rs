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

fn minimal_ok_xml() -> String {
	r#"<should_compress>true</should_compress>
<original_request>do the thing</original_request>
<session_context>session brought to here</session_context>
<current_task>finish the thing</current_task>
<progress>started it</progress>
<analysis_findings><finding>root cause is X</finding></analysis_findings>
<errors_and_corrections><entry>don't do Y</entry></errors_and_corrections>
<recent_exchanges><exchange>user asked Z</exchange></recent_exchanges>
<key_entities>
  <files><file>a.rs:1-10</file></files>
  <names><name>foo_fn</name></names>
  <decisions><decision>chose A over B</decision></decisions>
</key_entities>
<next_steps>do the next thing</next_steps>
<file_context><range filepath="a.rs" start_line="1" end_line="10"/></file_context>
<critical_knowledge><knowledge>arch decision: X</knowledge></critical_knowledge>
<open_loops><open_loop>awaiting user decision on Y</open_loop></open_loops>
<file_states><state>a.rs — added foo_fn, compiles</state></file_states>"#
		.to_string()
}

#[test]
fn parses_full_happy_path() {
	let s = parse_xml_summary(&minimal_ok_xml()).unwrap();
	assert!(s.should_compress);
	assert_eq!(s.original_request, "do the thing");
	assert_eq!(s.current_task, "finish the thing");
	assert_eq!(s.analysis_findings, vec!["root cause is X"]);
	assert_eq!(s.errors_and_corrections, vec!["don't do Y"]);
	assert_eq!(s.recent_exchanges, vec!["user asked Z"]);
	assert_eq!(s.key_entities.files, vec!["a.rs:1-10"]);
	assert_eq!(s.key_entities.names, vec!["foo_fn"]);
	assert_eq!(s.key_entities.decisions, vec!["chose A over B"]);
	assert_eq!(s.next_steps, "do the next thing");
	assert_eq!(s.file_context.len(), 1);
	assert_eq!(s.file_context[0].filepath, "a.rs");
	assert_eq!(s.file_context[0].start_line, 1);
	assert_eq!(s.file_context[0].end_line, 10);
	assert_eq!(s.critical_knowledge, vec!["arch decision: X"]);
	assert_eq!(s.open_loops, vec!["awaiting user decision on Y"]);
	assert_eq!(s.file_states, vec!["a.rs — added foo_fn, compiles"]);
}

#[test]
fn parses_should_compress_false_with_empty_fields() {
	let xml = r#"<should_compress>false</should_compress>
<original_request></original_request>
<session_context></session_context>
<current_task></current_task>
<progress></progress>
<analysis_findings></analysis_findings>
<errors_and_corrections></errors_and_corrections>
<recent_exchanges></recent_exchanges>
<key_entities><files></files><names></names><decisions></decisions></key_entities>
<next_steps></next_steps>
<file_context></file_context>
<critical_knowledge></critical_knowledge>"#;
	let s = parse_xml_summary(xml).unwrap();
	assert!(!s.should_compress);
	assert!(s.analysis_findings.is_empty());
	assert!(s.file_context.is_empty());
}

#[test]
fn strips_code_fence_envelope() {
	let xml = format!("```xml\n{}\n```", minimal_ok_xml());
	let s = parse_xml_summary(&xml).unwrap();
	assert!(s.should_compress);
}

#[test]
fn rejects_missing_should_compress() {
	let xml = "<original_request>x</original_request>";
	let err = parse_xml_summary(xml).unwrap_err().to_string();
	assert!(err.contains("should_compress"), "got: {err}");
}

#[test]
fn rejects_invalid_bool() {
	let xml = "<should_compress>maybe</should_compress>";
	let err = parse_xml_summary(xml).unwrap_err().to_string();
	assert!(err.contains("true/false"), "got: {err}");
}

#[test]
fn rejects_inverted_line_range() {
	let xml = r#"<should_compress>true</should_compress>
<file_context><range filepath="a.rs" start_line="20" end_line="10"/></file_context>"#;
	let err = parse_xml_summary(xml).unwrap_err().to_string();
	assert!(err.contains("start_line > end_line"), "got: {err}");
}

#[test]
fn rejects_out_of_range_line() {
	let xml = r#"<should_compress>true</should_compress>
<file_context><range filepath="a.rs" start_line="0" end_line="10"/></file_context>"#;
	let err = parse_xml_summary(xml).unwrap_err().to_string();
	assert!(err.contains("out of range"), "got: {err}");
}

#[test]
fn rejects_empty_filepath() {
	let xml = r#"<should_compress>true</should_compress>
<file_context><range filepath="" start_line="1" end_line="10"/></file_context>"#;
	let err = parse_xml_summary(xml).unwrap_err().to_string();
	assert!(err.contains("empty filepath"), "got: {err}");
}

#[test]
fn parses_multiple_items_and_drops_empties() {
	let xml = r#"<should_compress>true</should_compress>
<analysis_findings>
  <finding>a</finding>
  <finding>  </finding>
  <finding>b</finding>
</analysis_findings>"#;
	let s = parse_xml_summary(xml).unwrap();
	assert_eq!(s.analysis_findings, vec!["a", "b"]);
}

#[test]
fn tolerates_prose_before_and_after() {
	let xml = format!(
		"Sure, here is the output:\n\n{}\n\nLet me know if you need more.",
		minimal_ok_xml()
	);
	let s = parse_xml_summary(&xml).unwrap();
	assert!(s.should_compress);
}

#[test]
fn pact_schema_requires_folds_while_legacy_schema_does_not_expose_them() {
	let pact = build_compression_schema(false, true);
	assert!(pact["properties"].get("folded_units").is_some());
	assert!(pact["required"]
		.as_array()
		.unwrap()
		.iter()
		.any(|field| field == "folded_units"));
	assert_eq!(pact["properties"]["current_task"]["maxLength"], 0);
	assert_eq!(pact["properties"]["critical_knowledge"]["maxItems"], 0);
	assert_eq!(
		pact["properties"]["key_entities"]["properties"]["files"]["maxItems"],
		0
	);

	let legacy = build_compression_schema(false, false);
	assert!(legacy["properties"].get("folded_units").is_none());
	assert!(!legacy["required"]
		.as_array()
		.unwrap()
		.iter()
		.any(|field| field == "folded_units"));
	assert!(legacy["properties"]["current_task"]
		.get("maxLength")
		.is_none());
	assert_eq!(legacy["properties"]["critical_knowledge"]["maxItems"], 15);
}

#[test]
fn parses_and_renders_attributed_fold_with_stable_escaped_id() {
	let xml = format!(
			"{}\n<folded_units><unit><text>A & B</text><kind>outcome</kind><status>established</status><refs><ref>b:one</ref></refs></unit></folded_units>",
			minimal_ok_xml()
		);
	let parsed = parse_xml_summary(&xml).unwrap();
	assert_eq!(parsed.folded_units.len(), 1);
	let rendered = render_summary(&parsed);
	let expected_id = super::super::attention::folded_unit_id(&parsed.folded_units[0]);
	assert!(rendered.contains(&format!("id=\"{expected_id}\"")));
	assert!(rendered.contains("A &amp; B"));
}

#[test]
fn superseded_fold_renders_only_a_runtime_tombstone() {
	let stale = "obsolete state that must not interfere";
	let rendered = render_pact_summary(&CompressionSummary {
		folded_units: vec![FoldedUnit {
			text: stale.into(),
			kind: "observation".into(),
			status: "superseded".into(),
			refs: vec!["b:old".into()],
		}],
		..Default::default()
	});
	assert!(!rendered.contains(stale));
	assert!(rendered.contains("status=\"superseded\""));
	assert!(rendered.contains("must not be treated as current"));
	assert!(rendered.contains("refs=\"b:old\""));
}
