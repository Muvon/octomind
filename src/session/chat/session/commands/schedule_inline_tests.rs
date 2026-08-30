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
fn test_tokenize_basic() {
	let toks = tokenize_shell_like("a b c").unwrap();
	assert_eq!(toks, vec!["a", "b", "c"]);
}

#[test]
fn test_tokenize_double_quotes() {
	let toks = tokenize_shell_like(r#"when="in 5m" message="hello world""#).unwrap();
	assert_eq!(toks, vec!["when=in 5m", "message=hello world"]);
}

#[test]
fn test_tokenize_single_quotes() {
	let toks = tokenize_shell_like("when='in 1h 30m' every='10m'").unwrap();
	assert_eq!(toks, vec!["when=in 1h 30m", "every=10m"]);
}

#[test]
fn test_tokenize_mixed_quotes_and_bare() {
	let toks = tokenize_shell_like(r#"abc123 when="9am" message=hi"#).unwrap();
	assert_eq!(toks, vec!["abc123", "when=9am", "message=hi"]);
}

#[test]
fn test_tokenize_unterminated_quote() {
	assert!(tokenize_shell_like(r#"key="value"#).is_err());
}

#[test]
fn test_slice_after_token() {
	assert_eq!(
		slice_after_token("/schedule add when=5m", "add"),
		Some(" when=5m")
	);
	assert_eq!(
		slice_after_token("/schedule edit abc when=5m", "edit"),
		Some(" abc when=5m")
	);
	// Substring match should not count
	assert_eq!(slice_after_token("/schedule adder", "add"), None);
}

#[test]
fn test_parse_kv_add() {
	let mut params = Map::new();
	parse_kv_args(
		r#" when="in 5m" message="hi" every="10m""#,
		"add",
		&mut params,
	)
	.unwrap();
	assert_eq!(params.get("when").and_then(|v| v.as_str()), Some("in 5m"));
	assert_eq!(params.get("message").and_then(|v| v.as_str()), Some("hi"));
	assert_eq!(params.get("every").and_then(|v| v.as_str()), Some("10m"));
}

#[test]
fn test_parse_kv_edit_skips_positional_id() {
	let mut params = Map::new();
	parse_kv_args(r#" abc123 when="9am""#, "edit", &mut params).unwrap();
	assert_eq!(params.get("when").and_then(|v| v.as_str()), Some("9am"));
	assert!(params.get("id").is_none()); // caller inserts id; parse_kv just skips it
}

#[test]
fn test_parse_kv_rejects_bare_for_add() {
	let mut params = Map::new();
	let err = parse_kv_args(" bareword", "add", &mut params).unwrap_err();
	assert!(err.to_string().contains("expected key=value"));
}
