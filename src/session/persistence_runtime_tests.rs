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
