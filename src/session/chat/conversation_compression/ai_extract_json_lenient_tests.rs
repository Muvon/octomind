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
use super::extract_json_lenient;

#[test]
fn parses_bare_object() {
	let v = extract_json_lenient(r#"{"should_compress": true, "x": 1}"#).unwrap();
	assert_eq!(v["should_compress"], true);
	assert_eq!(v["x"], 1);
}

#[test]
fn parses_bare_array() {
	let v = extract_json_lenient(r#"[1, 2, 3]"#).unwrap();
	assert_eq!(v.as_array().unwrap().len(), 3);
}

#[test]
fn strips_json_labeled_markdown_fence() {
	let input = "```json\n{\"should_compress\": false}\n```";
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["should_compress"], false);
}

#[test]
fn strips_unlabeled_markdown_fence() {
	let input = "```\n{\"k\": \"v\"}\n```";
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["k"], "v");
}

#[test]
fn recovers_from_chatty_preamble() {
	let input = "Here is the analysis:\n{\"should_compress\": true, \"target\": 2.0}";
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["should_compress"], true);
	assert_eq!(v["target"], 2.0);
}

#[test]
fn recovers_from_preamble_with_fence() {
	let input = "Sure, here you go:\n```json\n{\"a\": 1}\n```\nDone!";
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["a"], 1);
}

#[test]
fn respects_braces_inside_strings() {
	// Naive brace-counting would balance early on the `{` inside the string;
	// the scanner must skip string contents.
	let input = r#"text {"label": "value with } brace", "n": 7}"#;
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["label"], "value with } brace");
	assert_eq!(v["n"], 7);
}

#[test]
fn handles_escaped_quotes_in_strings() {
	let input = r#"prefix {"msg": "she said \"hi\""}"#;
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["msg"], "she said \"hi\"");
}

#[test]
fn returns_none_for_empty_input() {
	assert!(extract_json_lenient("").is_none());
	assert!(extract_json_lenient("   \n\t  ").is_none());
}

#[test]
fn returns_none_for_no_json() {
	assert!(extract_json_lenient("just a plain text response with no JSON").is_none());
}

#[test]
fn returns_none_for_truncated_json() {
	// Opener with no matching close — provider got cut off.
	assert!(extract_json_lenient(r#"{"incomplete": "no closing brace"#).is_none());
}

#[test]
fn skips_invalid_object_finds_later_valid_one() {
	// First {…} has a syntax error; scanner must keep going and find the second.
	let input = r#"garbage {not valid json} more text {"ok": true}"#;
	let v = extract_json_lenient(input).unwrap();
	assert_eq!(v["ok"], true);
}
