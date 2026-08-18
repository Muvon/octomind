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

//! Runtime-state extraction from a session log built through the real
//! append path (per-append zstd frames): command replay, knowledge
//! restoration, and the restoration-point reset.

use super::*;
use serde_json::json;

fn append(file: &std::path::PathBuf, value: serde_json::Value) {
	append_to_session_file(file, &value.to_string()).expect("append log line");
}

#[test]
fn test_runtime_state_replays_commands_across_restoration_point() {
	let tmp = tempfile::NamedTempFile::new().expect("tmp");
	let path = tmp.path().to_path_buf();

	append(
		&path,
		json!({"type": "COMMAND", "command": "/model openrouter:before"}),
	);
	append(
		&path,
		json!({"type": "COMMAND", "command": "/role developer"}),
	);
	append(&path, json!({"type": "COMMAND", "command": "/effort high"}));
	append(
		&path,
		json!({"type": "KNOWLEDGE_ENTRY", "content": "stale knowledge"}),
	);
	// A restoration point wipes everything recorded before it
	append(&path, json!({"type": "RESTORATION_POINT"}));
	append(
		&path,
		json!({"type": "COMMAND", "command": "/model ollama:after"}),
	);
	append(&path, json!({"type": "COMMAND", "command": "/cache"}));
	append(
		&path,
		json!({"type": "KNOWLEDGE_ENTRY", "content": "fresh knowledge"}),
	);
	// Unknown commands are ignored, not errors
	append(
		&path,
		json!({"type": "COMMAND", "command": "/definitely-unknown x"}),
	);

	let state = extract_runtime_state_from_log(&path).expect("extract state");
	assert_eq!(state.model.as_deref(), Some("ollama:after"));
	assert_eq!(state.role, None, "role predates the restoration point");
	assert_eq!(state.reasoning_effort, None);
	assert!(state.cache_next_message);
	assert_eq!(
		state.critical_knowledge,
		vec!["fresh knowledge".to_string()]
	);
}

#[test]
fn test_load_session_missing_file_errors() {
	let path = std::path::PathBuf::from("/definitely/not/here/session.log");
	assert!(load_session(&path).is_err());
	assert!(extract_runtime_state_from_log(&path).is_err());
}

fn msg_line(role: &str, content: &str, ts: u64) -> serde_json::Value {
	json!({"role": role, "content": content, "timestamp": ts})
}

#[test]
fn test_load_session_restoration_then_truncation() {
	let tmp = tempfile::NamedTempFile::new().expect("tmp");
	let path = tmp.path().to_path_buf();

	append(&path, msg_line("user", "before restoration", 1));
	append(&path, msg_line("assistant", "old answer", 2));
	append(&path, json!({"type": "RESTORATION_POINT"}));
	append(&path, msg_line("user", "after restoration", 3));
	append(&path, msg_line("assistant", "new answer", 4));
	// Ctrl+C cleanup marker: only the first post-restoration message survives
	append(
		&path,
		json!({"type": "TRUNCATION_POINT", "message_count": 1}),
	);

	let session = load_session(&path).expect("load session");
	let contents: Vec<&str> = session
		.messages
		.iter()
		.map(|m| m.content.as_str())
		.collect();
	assert_eq!(contents, vec!["after restoration"], "{contents:?}");
}

#[test]
fn test_load_session_compression_point_and_tool_call_reconstruction() {
	let tmp = tempfile::NamedTempFile::new().expect("tmp");
	let path = tmp.path().to_path_buf();

	append(&path, msg_line("user", "compressed away", 1));
	append(
		&path,
		json!({"type": "COMPRESSION_POINT", "compression_type": "task", "messages_removed": 1}),
	);
	append(&path, msg_line("user", "live request", 2));
	// A TOOL_CALL marker followed by its tool result: the loader must
	// reconstruct the assistant tool_calls message the API pairing needs.
	append(
		&path,
		json!({"type": "TOOL_CALL", "tool_name": "shell", "tool_id": "c1", "parameters": {"cmd": "ls"}}),
	);
	append(
		&path,
		json!({"role": "tool", "content": "listing", "timestamp": 3, "tool_call_id": "c1", "name": "shell"}),
	);
	append(&path, msg_line("assistant", "done", 4));

	let session = load_session(&path).expect("load session");
	let roles: Vec<&str> = session.messages.iter().map(|m| m.role.as_str()).collect();
	assert_eq!(
		roles,
		vec!["user", "assistant", "tool", "assistant"],
		"compressed prefix must be gone and the tool_calls message reconstructed"
	);
	let reconstructed = &session.messages[1];
	let calls = reconstructed
		.tool_calls
		.as_ref()
		.expect("reconstructed tool_calls");
	assert!(calls.to_string().contains("shell"));
	assert_eq!(session.messages[2].tool_call_id.as_deref(), Some("c1"));
}
