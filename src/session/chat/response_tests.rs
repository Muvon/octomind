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

#[test]
fn test_preview_value_strings() {
	assert_eq!(preview_value(&json!("short")), "\"short\"");
	// Newlines flatten to spaces
	assert_eq!(preview_value(&json!("a\nb")), "\"a b\"");
	// Over 60 chars → truncated at 59 + ellipsis
	let long = "x".repeat(80);
	let preview = preview_value(&json!(long));
	assert_eq!(preview, format!("\"{}…\"", "x".repeat(59)));
}

#[test]
fn test_preview_value_arrays() {
	assert_eq!(preview_value(&json!([])), "[]");
	assert_eq!(preview_value(&json!(["only"])), "[\"only\"]");
	// Range-like scalar pair shows both values
	assert_eq!(preview_value(&json!([1, 150])), "[1, 150]");
	// Longer arrays collapse to first + count
	assert_eq!(preview_value(&json!([1, 2, 3])), "[1, +2]");
	// Two-element array with a non-scalar member is not a range pair
	assert_eq!(preview_value(&json!([1, {"k": 2}])), "[1, +1]");
}

#[test]
fn test_preview_value_scalars_and_objects() {
	assert_eq!(preview_value(&json!({"a": 1})), "{…}");
	assert_eq!(preview_value(&json!(null)), "null");
	assert_eq!(preview_value(&json!(42)), "42");
	assert_eq!(preview_value(&json!(true)), "true");
}

#[test]
fn test_resolve_tool_calls() {
	let call = crate::mcp::McpToolCall {
		tool_name: "shell".to_string(),
		parameters: json!({"cmd": "ls"}),
		tool_id: "id1".to_string(),
	};
	let mut some_calls = Some(vec![call]);
	let resolved = resolve_tool_calls(&mut some_calls, "ignored");
	assert_eq!(resolved.len(), 1);
	assert_eq!(resolved[0].tool_name, "shell");
	// The Option is consumed
	assert!(some_calls.is_none());

	let mut none_calls = None;
	assert!(resolve_tool_calls(&mut none_calls, "ignored").is_empty());
}

#[test]
fn test_check_cancellation() {
	let (tx, rx) = tokio::sync::watch::channel(false);
	assert!(check_cancellation(&rx).is_ok());

	tx.send(true).expect("send cancellation");
	let err = check_cancellation(&rx).expect_err("cancelled must error");
	assert!(crate::session::cancellation::is_cancelled(&err));
}
