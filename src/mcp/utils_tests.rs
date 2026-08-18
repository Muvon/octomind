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
fn test_guess_tool_category() {
	// Exact names
	assert_eq!(guess_tool_category("core"), "system");
	assert_eq!(guess_tool_category("shell"), "filesystem");
	assert_eq!(guess_tool_category("text_editor"), "filesystem");
	assert_eq!(guess_tool_category("plan"), "core");
	// Contains-based rules
	assert_eq!(guess_tool_category("web_fetch"), "web");
	assert_eq!(guess_tool_category("semantic_search"), "search");
	assert_eq!(guess_tool_category("github_prs"), "github");
	assert_eq!(guess_tool_category("git_status"), "git");
	// Unknown → external
	assert_eq!(guess_tool_category("mystery_tool"), "external");
}

#[test]
fn test_extract_content_variants() {
	let ok = McpToolResult::success("t".to_string(), "id".to_string(), "hello".to_string());
	assert!(!ok.is_error());
	assert_eq!(ok.extract_content(), "hello");

	let err = McpToolResult::error("t".to_string(), "id".to_string(), "boom".to_string());
	assert!(err.is_error());
	assert_eq!(err.extract_content(), "boom");

	let with_meta = McpToolResult::success_with_metadata(
		"t".to_string(),
		"id".to_string(),
		"body".to_string(),
		serde_json::json!({"k": "v"}),
	);
	let content = with_meta.extract_content();
	assert!(content.starts_with("body"));
	assert!(content.contains("[Metadata:"));
	assert!(content.contains("\"k\": \"v\""));
}

#[test]
fn test_tool_results_to_messages() {
	let config: crate::config::Config =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("parse default config template");

	let results = vec![
		McpToolResult::success("shell".to_string(), "id1".to_string(), "out".to_string()),
		McpToolResult::error("view".to_string(), "id2".to_string(), "fail".to_string()),
	];
	let messages = tool_results_to_messages(&results, &config);
	assert_eq!(messages.len(), 2);
	assert_eq!(messages[0].role, "tool");
	assert_eq!(messages[0].tool_call_id, "id1");
	assert_eq!(messages[0].name, "shell");
	assert_eq!(messages[0].content, "out");
	assert_eq!(messages[1].content, "fail");
}
