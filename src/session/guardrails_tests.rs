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
