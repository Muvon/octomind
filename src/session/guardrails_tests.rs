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

// Each test uses a unique session id: the registries are process globals
// shared by parallel tests.

#[test]
fn test_message_and_pipe_counters() {
	let sid: SessionId = "guardrails-test-counters".to_string();
	assert_eq!(increment_message_count(&sid), 1);
	assert_eq!(increment_message_count(&sid), 2);
	assert_eq!(increment_pipe_run_count(&sid, "p1"), 1);
	assert_eq!(increment_pipe_run_count(&sid, "p1"), 2);
	// Pipes are counted independently
	assert_eq!(increment_pipe_run_count(&sid, "p2"), 1);
}

#[test]
fn test_validator_cursors() {
	let sid: SessionId = "guardrails-test-cursors".to_string();
	// Default cursor is 0 ("since session start")
	assert_eq!(validator_cursor(&sid, "v"), 0);
	set_validator_cursor(&sid, "v", 7);
	assert_eq!(validator_cursor(&sid, "v"), 7);
	assert_eq!(validator_cursor(&sid, "other"), 0);
}

#[test]
fn test_call_log_roundtrip() {
	let sid: SessionId = "guardrails-test-calllog".to_string();
	assert!(get_call_log(&sid).is_empty());
	record_call(
		&sid,
		Some("files-read".to_string()),
		serde_json::json!({"path": "x"}),
	);
	record_call(&sid, None, serde_json::json!({}));
	let log = get_call_log(&sid);
	assert_eq!(log.len(), 2);
	assert_eq!(log[0].0.as_deref(), Some("files-read"));
	assert!(log[1].0.is_none());
}

#[test]
fn authorizer_denied_calls_never_enter_history_and_preview_is_read_only() {
	let sid = "guardrails-authorizer-history".to_string();
	let config: Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
	let calls = vec![
		crate::mcp::McpToolCall {
			tool_name: "a".into(),
			tool_id: "1".into(),
			parameters: serde_json::json!({"step":1}),
		},
		crate::mcp::McpToolCall {
			tool_name: "b".into(),
			tool_id: "2".into(),
			parameters: serde_json::json!({"step":2}),
		},
	];
	preview_batch(&sid, &config, &calls);
	assert!(get_call_log(&sid).is_empty());
	let admissions = vec![
		crate::supervisor::authorizer::Admission {
			message: Some("[authorizer] forbidden".into()),
			..Default::default()
		},
		Default::default(),
	];
	let result = check_batch_admitted(&sid, &config, &calls, &admissions);
	assert!(result[0].is_some());
	assert!(result[1].is_none());
	let log = get_call_log(&sid);
	assert_eq!(log.len(), 1);
	assert_eq!(log[0].1, calls[1].parameters);
	clear_for_session(&sid);
}

#[tokio::test]
#[serial_test::serial]
async fn authorizer_denial_invalidates_a_later_native_history_prerequisite() {
	let sid = "authorizer-ordered-prerequisite".to_string();
	let mut config: Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).unwrap();
	let server = crate::config::McpServerConfig::builtin("authorizer-test", 30, vec![]);
	config.mcp.servers = vec![server.clone()];
	crate::mcp::tool_map::initialize_tool_map(&config)
		.await
		.unwrap();
	let names = vec!["auth_read".to_string(), "auth_write".to_string()];
	crate::mcp::tool_map::register_dynamic_server_tools("authorizer-test", &server, &names);
	CAP_LOOKUP
		.write()
		.unwrap()
		.get_or_insert_with(HashMap::new)
		.insert(
			sid.clone(),
			Arc::new(CapLookup {
				exact: HashMap::from([
					(
						("authorizer-test".into(), "auth_read".into()),
						"read".into(),
					),
					(
						("authorizer-test".into(), "auth_write".into()),
						"write".into(),
					),
				]),
				..Default::default()
			}),
		);
	let rules = Guardrails::parse(
		"[[guard]]\nmatch='write'\nwhen=['-read']\nmessage='Read must be admitted first'\n",
	)
	.unwrap();
	merge_generated_for_session(&sid, rules);
	let calls = names
		.iter()
		.enumerate()
		.map(|(i, name)| crate::mcp::McpToolCall {
			tool_name: name.clone(),
			tool_id: i.to_string(),
			parameters: serde_json::json!({}),
		})
		.collect::<Vec<_>>();
	let preview = preview_batch(&sid, &config, &calls);
	assert_eq!(preview.blocked, vec![None, None]);
	let admissions = vec![
		crate::supervisor::authorizer::Admission {
			message: Some("[authorizer] Outside requested scope".into()),
			..Default::default()
		},
		Default::default(),
	];
	let result = check_batch_admitted(&sid, &config, &calls, &admissions);
	assert_eq!(result[1].as_deref(), Some("Read must be admitted first"));
	assert!(get_call_log(&sid).is_empty());
	crate::mcp::tool_map::unregister_dynamic_server_tools("authorizer-test", &names);
	clear_for_session(&sid);
}
