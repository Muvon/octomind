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

use super::is_empty_completion;
use crate::mcp::McpToolCall;
use serde_json::json;

fn call() -> McpToolCall {
	McpToolCall {
		tool_name: "view".into(),
		parameters: json!({}),
		tool_id: "t1".into(),
	}
}

#[test]
fn blank_content_no_tools_no_schema_is_empty() {
	assert!(is_empty_completion("", None, None));
	assert!(is_empty_completion("   \n\t ", None, None));
}

#[test]
fn empty_tool_vec_is_still_empty() {
	// Some providers hand back `Some([])` rather than `None`.
	assert!(is_empty_completion("", Some(&vec![]), None));
}

#[test]
fn text_response_is_not_empty() {
	assert!(!is_empty_completion("hello", None, None));
}

#[test]
fn tool_only_response_is_not_empty() {
	// Tool calls with no prose is a valid turn — must NOT be flagged as empty.
	assert!(!is_empty_completion("", Some(&vec![call()]), None));
}

#[test]
fn structured_output_is_not_empty() {
	// Structured-output replies carry empty content by design — not empty.
	assert!(!is_empty_completion("", None, Some(&json!({"ok": true}))));
}
