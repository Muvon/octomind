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

fn config(direct_tools: &[&str]) -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../config-templates/default.toml")).expect("parse default config template");
	config.mcp.direct_tools = direct_tools.iter().map(|pattern| pattern.to_string()).collect();
	config
}

fn function(name: &str) -> McpFunction {
	McpFunction {
		name: name.to_string(),
		description: format!("Does {name}."),
		parameters: json!({
			"type": "object",
			"properties": { "path": { "type": "string" } },
			"required": ["path"]
		}),
		annotations: None,
	}
}

/// A builtin tool, the `code` tool, and two external servers.
fn session_tools() -> Vec<(Option<String>, McpFunction)> {
	vec![
		(None, function("plan")),
		(None, get_code_function()),
		(Some("files".to_string()), function("view")),
		(Some("files".to_string()), function("text_edit")),
		(Some("search".to_string()), function("find")),
	]
}

fn names(functions: &[McpFunction]) -> Vec<&str> {
	functions.iter().map(|function| function.name.as_str()).collect()
}

#[test]
fn the_shipped_default_sends_every_external_tool_through_scripts() {
	let (definitions, catalog) = split(&config(&[]), session_tools());
	assert_eq!(names(&definitions), vec!["plan", "code"]);
	assert_eq!(
		catalog.index(),
		"files.view({path: string}) // Does view.\n\
		 files.text_edit({path: string}) // Does text_edit.\n\
		 search.find({path: string}) // Does find."
	);
}

#[test]
fn the_code_tool_lists_what_scripts_can_call() {
	let (definitions, _) = split(&config(&[]), session_tools());
	let code = definitions.iter().find(|function| function.name == TOOL_NAME).unwrap();
	assert!(code.description.starts_with(DESCRIPTION));
	assert!(code.description.ends_with(
		"\n\nTools, called like `await files.view({...})`:\n\
		 files.view({path: string}) // Does view.\n\
		 files.text_edit({path: string}) // Does text_edit.\n\
		 search.find({path: string}) // Does find."
	));
}

/// A list longer than a tool result may be is not sent with every request:
/// the `code` tool names the servers and how to find their tools instead.
#[test]
fn a_list_too_long_to_send_becomes_the_servers() {
	let mut config = config(&[]);
	config.mcp_response_tokens_threshold = 1;
	let (definitions, catalog) = split(&config, session_tools());
	let code = definitions.iter().find(|function| function.name == TOOL_NAME).unwrap();
	assert!(code.description.ends_with("\nfiles (2 tools)\nsearch (1 tool)"));
	assert!(code.description.contains("search_tools(\"words\")"));
	assert!(!code.description.contains("Does view."));
	assert!(catalog
		.search("edit text")
		.starts_with("files.text_edit({path: string})"));
}

fn made(tool: &str, output: &str, error: bool) -> ScriptCall {
	ScriptCall {
		id: format!("code-1:{tool}"),
		server: "files".to_string(),
		tool: tool.to_string(),
		arguments: json!({"path": "a.rs"}),
		output: output.to_string(),
		error,
		denied: false,
	}
}

/// The session keeps what the model saw and what changed files; the output
/// of a read the script did not print is dropped.
#[test]
fn kept_calls_drop_only_unprinted_read_output() {
	let calls = vec![
		made("kept_test_view", "1:aa|shown line", false),
		made("kept_test_view", "1:bb|never printed", false),
		made("kept_test_text_edit", "+1:cc|edited", false),
		made("kept_test_view", "no such file", true),
	];
	let kept = kept_calls(&calls, "before\n1:aa|shown line\nafter");
	let outputs: Vec<&str> = kept.iter().map(|made| made.output.as_str()).collect();
	assert_eq!(outputs, vec!["1:aa|shown line", "", "+1:cc|edited", "no such file"]);
	assert_eq!(kept.len(), calls.len());
	assert!(kept.iter().all(|made| made.arguments == json!({"path": "a.rs"})));
}

#[test]
fn direct_tools_keeps_named_tools_as_definitions() {
	let (definitions, catalog) = split(&config(&["files:*"]), session_tools());
	assert_eq!(names(&definitions), vec!["plan", "code", "view", "text_edit"]);
	assert_eq!(catalog.index(), "search.find({path: string}) // Does find.");

	let (definitions, catalog) = split(&config(&["files:view", "fin*"]), session_tools());
	assert_eq!(names(&definitions), vec!["plan", "code", "view", "find"]);
	assert_eq!(catalog.index(), "files.text_edit({path: string}) // Does text_edit.");
}

#[test]
fn a_pattern_for_another_server_does_not_match() {
	assert!(pattern_matches("files:*", "files", "view"));
	assert!(!pattern_matches("files:*", "search", "view"));
	assert!(pattern_matches("view", "search", "view"));
	assert!(!pattern_matches("files:edit", "files", "view"));
	assert!(pattern_matches("*", "anything", "at_all"));
}

#[test]
fn every_tool_direct_turns_the_code_tool_off() {
	let (definitions, catalog) = split(&config(&["*"]), session_tools());
	assert_eq!(names(&definitions), vec!["plan", "view", "text_edit", "find"]);
	assert!(catalog.is_empty());
}

#[test]
fn a_session_without_external_tools_has_no_code_tool() {
	let (definitions, catalog) = split(
		&config(&[]),
		vec![(None, function("plan")), (None, get_code_function())],
	);
	assert_eq!(names(&definitions), vec!["plan"]);
	assert!(catalog.is_empty());
}

#[test]
fn a_role_without_the_code_tool_keeps_every_tool_as_a_definition() {
	let tools: Vec<_> = session_tools()
		.into_iter()
		.filter(|(_, function)| function.name != TOOL_NAME)
		.collect();
	let (definitions, catalog) = split(&config(&[]), tools);
	assert_eq!(names(&definitions), vec!["plan", "view", "text_edit", "find"]);
	assert!(catalog.is_empty());
}

#[test]
fn an_external_tool_named_code_does_not_stand_in_for_the_builtin_one() {
	let tools = vec![
		(None, function("plan")),
		(Some("other".to_string()), function(TOOL_NAME)),
		(Some("files".to_string()), function("view")),
	];
	let (definitions, catalog) = split(&config(&[]), tools);
	assert_eq!(names(&definitions), vec!["plan", "code", "view"]);
	assert!(catalog.is_empty());
}

#[test]
fn a_tool_whose_script_name_is_taken_stays_a_definition() {
	let mut tools = session_tools();
	tools.push((Some("files".to_string()), function("text-edit")));
	let (definitions, catalog) = split(&config(&[]), tools);
	assert_eq!(names(&definitions), vec!["plan", "code", "text-edit"]);
	assert_eq!(catalog.servers[0].tools.len(), 2);
}

#[test]
fn a_rejection_for_an_unknown_tool_lists_the_real_ones() {
	let (_, catalog) = split(&config(&[]), session_tools());
	let problems = vec![check::Problem {
		line: 1,
		column: 13,
		message: "TS2339: Property 'veiw' does not exist on type 'filesTools'.".to_string(),
	}];
	assert_eq!(
		rejection(&catalog, &problems),
		"Script rejected by the type check before it ran; no tool was called.\n\
		 line 1:13 TS2339: Property 'veiw' does not exist on type 'filesTools'.\n\
		 Tools of files: view, text_edit"
	);
}

#[test]
fn a_rejection_for_an_unknown_name_lists_the_servers_and_drops_follow_on_problems() {
	let (_, catalog) = split(&config(&[]), session_tools());
	let problem = |line, column, message: &str| check::Problem {
		line,
		column,
		message: message.to_string(),
	};
	let problems = vec![
		problem(1, 20, "TS2304: Cannot find name 'server'."),
		problem(2, 39, "TS7006: Parameter 'f' implicitly has an 'any' type."),
	];
	assert_eq!(
		rejection(&catalog, &problems),
		"Script rejected by the type check before it ran; no tool was called.\n\
		 line 1:20 TS2304: Cannot find name 'server'.\n\
		 Servers: files, search"
	);
	// Without an unknown name, an untyped value is a problem of its own.
	assert!(rejection(&catalog, &problems[1..]).contains("TS7006"));
}

#[test]
fn a_script_that_mentions_a_writing_tool_may_have_written() {
	let writers = vec!["text_edit".to_string(), "shell".to_string()];
	assert!(names_any("await files.text_edit(input)", &writers));
	assert!(names_any(
		"const run = files . shell;\nawait run({command: 'ls'})",
		&writers
	));
	assert!(names_any("await files['text_edit'](input)", &writers));
	assert!(names_any(
		"const {shell} = files;\nawait shell({command: 'ls'})",
		&writers
	));
	assert!(!names_any(
		"const text = await files.view({path: 'a'});\ntext.split('\\n').map(String)",
		&writers
	));
	assert!(!names_any("print('text_editor shells')", &writers));
}

#[tokio::test]
async fn a_call_outside_a_session_is_refused() {
	let call = McpToolCall {
		tool_name: TOOL_NAME.to_string(),
		parameters: json!({ "script": "print(1)" }),
		tool_id: "call-1".to_string(),
	};
	let result = execute(&call, &config(&[]), None).await;
	assert!(result.is_error());
	assert_eq!(result.extract_content(), "scripts run only inside a session");
}

#[tokio::test]
async fn a_call_without_a_script_is_refused() {
	let call = McpToolCall {
		tool_name: TOOL_NAME.to_string(),
		parameters: json!({ "script": "  " }),
		tool_id: "call-1".to_string(),
	};
	let result = execute(&call, &config(&[]), None).await;
	assert!(result.is_error());
	assert_eq!(result.extract_content(), "script: non-empty string required");
}
