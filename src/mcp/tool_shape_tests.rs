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
fn param_paths_collects_pathish_keys_only() {
	use serde_json::json;
	let p = json!({
		"path": "a.md",
		"from_path": "b.rs",
		"files": ["c.py", ""],
		"command": "rm -rf /",
		"content": "path-like text ignored"
	});
	let mut got = param_paths(&p);
	got.sort();
	assert_eq!(got, vec!["a.md", "b.rs", "c.py"]);
	assert!(param_paths(&json!({"command": "x"})).is_empty());
}

#[test]
fn param_paths_collects_paths_nested_in_item_lists() {
	// One call editing several files names each file inside its own item; the
	// call-level path is only the default for items that omit theirs.
	use serde_json::json;
	let p = json!({
		"command": "str_replace",
		"edits": [
			{"path": "src/a.hpp", "old_text": "x", "new_text": "y"},
			{"path": "src/b.cpp", "old_text": "x", "new_text": "y"},
			{"old_text": "path: not a key", "new_text": "y"}
		]
	});
	assert_eq!(param_paths(&p), vec!["src/a.hpp", "src/b.cpp"]);
}

#[test]
fn mutation_classification_uses_call_intent_and_mcp_hint() {
	use serde_json::json;
	assert!(is_mutation_call("text_tool", &json!({"command":"str_replace"})));
	assert!(is_mutation_call("generic_runner", &json!({"command":"deploy release"})));
	assert!(!is_mutation_call(
		"generic_runner",
		&json!({"command":"check booking status"})
	));
	register_tool_read_only_hint("remotePublisherForTest", Some(false));
	register_tool_read_only_hint("remoteLookupForTest", Some(true));
	assert!(is_mutation_call("remotePublisherForTest", &json!({})));
	assert!(!is_mutation_call("remoteLookupForTest", &json!({})));
}

#[test]
fn mutation_intent_normalizes_camel_case_and_separators() {
	assert!(
		is_mutation_call("sendEmail", &json!({})),
		"camelCase splits into intent words"
	);
	assert!(is_mutation_call("createFile", &json!({})));
	assert!(is_mutation_call("runner", &json!({"command": "git-push release"})));
	assert!(!is_mutation_call("runner", &json!({"command": "cat notes.txt"})));
	assert!(!is_mutation_call("statusLookup", &json!({})));
}

#[test]
fn read_only_hint_none_falls_back_to_call_intent() {
	register_tool_read_only_hint("detectTestsUntold", None);
	assert!(!is_mutation_call("detectTestsUntold", &json!({})));
}

#[test]
fn write_capable_runner_read_only_commands_are_not_mutations() {
	// Every honest command runner annotates itself write-capable (octofs `shell`
	// declares readOnlyHint=false). Answering from that capability classified
	// every build/test/validator run as a mutation — and those runs are the only
	// thing that can verify a change.
	//
	// The fixture name carries an intent word ("Write") on purpose: a runner's
	// identity says no more about the concrete call than its annotation does,
	// and judging the call by either one reopens the same hole.
	let runner = "detectTestsWriteCapableRunner";
	register_tool_read_only_hint(runner, Some(false));
	register_tool_command_shape(runner, true);
	let check = json!({"command": "bash scripts/lint.sh"});
	assert!(
		!is_mutation_call(runner, &check),
		"a runner's read-only command is not a mutation, whatever the tool can do"
	);
	assert!(
		is_mutation_call(runner, &json!({"command": "git push origin master"})),
		"the command's own intent is the only signal a runner leaves behind"
	);
}

#[test]
fn operation_selector_executes_nothing_whatever_the_operation_is_called() {
	// An editor's `command` names one of a fixed set of operations; it executes
	// nothing. Keyword lists miss the ones nobody thought to list ("overwrite"),
	// so the schema decides: a constrained vocabulary is never a command run.
	let editor = "detectTestsOperationSelector";
	register_tool_read_only_hint(editor, Some(false));
	register_tool_command_shape(editor, false);
	for op in ["str_replace", "overwrite", "truncate"] {
		let call = json!({ "command": op, "path": "a.rs" });
		assert!(is_mutation_call(editor, &call), "{op} edits");
		assert_eq!(executed_command(editor, &call), None, "{op} is an operation");
	}
}

#[test]
fn mutating_command_output_remains_verification_evidence() {
	let runner = "detectTestsMixedVerificationRunner";
	register_tool_read_only_hint(runner, Some(false));
	register_tool_command_shape(runner, true);
	for command in [
		"ssh dev \"docker exec php sh -c 'bin/test --filter=coupons && app/scripts/manage-coupons --cmd=create'\"",
		"pytest tests/test_create_coupon.py",
		"printf 'create update delete'",
	] {
		let call = json!({"command": command});
		assert!(is_mutation_call(runner, &call), "heuristic matches: {command}");
		assert!(executed_command(runner, &call).is_some(), "retain evidence: {command}");
	}
	assert_eq!(executed_command(runner, &json!({"command": "  "})), None);
	assert_eq!(executed_command(runner, &json!({"command": 42})), None);
}

#[test]
fn a_schema_declared_command_on_a_mutation_is_still_an_executed_command() {
	// An editor that runs a check after writing: `command` selects the operation
	// and the schema marks another parameter as the command the tool executes.
	let editor = "detectTestsEditorWithDeclaredCommand";
	let schema = json!({"properties": {
		"command": {"type": "string", "enum": ["create", "str_replace"]},
		"content": {"type": "string"},
		"after": {"type": "string", "contentMediaType": "application/x-sh"},
	}});
	assert_eq!(declared_command_params(&schema), vec!["after"]);
	register_tool_read_only_hint(editor, Some(false));
	register_tool_command_shape(editor, command_param_is_free_form(&schema));
	register_tool_command_params(editor, declared_command_params(&schema));

	let checked = json!({"command": "str_replace", "path": "a.rs", "content": "x", "after": "run the covering check"});
	assert!(is_mutation_call(editor, &checked));
	assert_eq!(executed_command(editor, &checked), Some("run the covering check"));

	// An unmarked string is content, however much it reads like a command.
	let plain = json!({"command": "create", "path": "a.rs", "content": "cargo test"});
	assert_eq!(executed_command(editor, &plain), None);
	assert_eq!(
		executed_command(editor, &json!({"command": "create", "after": "  "})),
		None
	);

	// A refreshed inventory without the mark withdraws the declaration.
	register_tool_command_params(editor, Vec::new());
	assert_eq!(executed_command(editor, &checked), None);
}

#[test]
fn command_param_free_form_reads_the_schema_not_the_name() {
	let free_form = json!({"properties": {"command": {"type": "string"}}});
	assert!(command_param_is_free_form(&free_form));
	let nullable = json!({"properties": {"command": {"type": ["string", "null"]}}});
	assert!(command_param_is_free_form(&nullable));
	let argv_or_string = json!({
		"properties": {"command": {"anyOf": [{"type": "string"}, {"type": "array"}]}}
	});
	assert!(command_param_is_free_form(&argv_or_string));
	let vocabulary = json!({
		"properties": {"command": {"type": "string", "enum": ["create", "str_replace"]}}
	});
	assert!(!command_param_is_free_form(&vocabulary));
	// schemars renders a named enum type as a reference, not an inline list.
	let referenced = json!({"properties": {"command": {"$ref": "#/$defs/TextEditorCommand"}}});
	assert!(!command_param_is_free_form(&referenced));
	assert!(!command_param_is_free_form(&json!({"properties": {}})));
	assert!(!command_param_is_free_form(&json!({})));
}

#[test]
fn structured_results_are_not_flattened_for_condensation() {
	let plain = McpToolResult::success("tool".into(), "plain".into(), "text".into());
	assert!(is_plain_text_result(&plain));
	let structured = McpToolResult::success_with_metadata(
		"tool".into(),
		"rich".into(),
		"text".into(),
		serde_json::json!({"important": true}),
	);
	assert!(!is_plain_text_result(&structured));
}

#[test]
fn plain_text_check_accepts_null_structured_and_empty_content_but_not_rich_blocks() {
	let mut null_structured = McpToolResult::success("tool".into(), "t".into(), "text".into());
	null_structured.result.structured_content = Some(serde_json::Value::Null);
	assert!(is_plain_text_result(&null_structured));

	let mut empty = McpToolResult::success("tool".into(), "t".into(), "text".into());
	empty.result.content = Vec::new();
	assert!(is_plain_text_result(&empty));

	let mut mixed = McpToolResult::success("tool".into(), "t".into(), "text".into());
	mixed
		.result
		.content
		.push(rmcp::model::ContentBlock::image("aGk=", "image/png"));
	assert!(!is_plain_text_result(&mixed));
}
