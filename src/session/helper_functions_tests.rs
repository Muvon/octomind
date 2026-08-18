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

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[test]
fn test_summarize_context() {
	let session = crate::session::Session {
		info: crate::session::SessionInfo::default(),
		messages: vec![
			message("user", "question"),
			message("assistant", "first answer"),
			message("assistant", "second answer"),
		],
		session_file: None,
	};
	let summary = summarize_context(&session, "new input");
	assert!(summary.contains("new input"));
	assert!(summary.contains("first answer"));
	assert!(summary.contains("second answer"));
	// User messages are context, not assistant history
	assert!(!summary.contains("question"));

	// Long histories are truncated at a char boundary with a marker
	let long_session = crate::session::Session {
		info: crate::session::SessionInfo::default(),
		messages: vec![message("assistant", &"é".repeat(3000))],
		session_file: None,
	};
	let summary = summarize_context(&long_session, "x");
	assert!(summary.contains("(truncated)"));
}

#[tokio::test]
async fn test_process_placeholders_replaces_builtins() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let processed = process_placeholders_async(
		"date={{DATE}} cwd={{CWD}} os={{OS}} shell={{SHELL}}",
		tmp.path(),
	)
	.await;
	assert!(!processed.contains("{{DATE}}"), "processed: {processed}");
	assert!(!processed.contains("{{CWD}}"), "processed: {processed}");
	assert!(!processed.contains("{{OS}}"), "processed: {processed}");
	assert!(!processed.contains("{{SHELL}}"), "processed: {processed}");
	assert!(processed.contains(&tmp.path().to_string_lossy().to_string()));
}

#[tokio::test]
async fn test_process_placeholders_role_and_unknown() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let processed = process_placeholders_async_with_role(
		"role={{ROLE}} keep={{NOT_A_PLACEHOLDER}}",
		tmp.path(),
		Some("tester"),
	)
	.await;
	assert!(processed.contains("role=tester"), "processed: {processed}");
	// Unknown placeholders are left for downstream passes
	assert!(
		processed.contains("{{NOT_A_PLACEHOLDER}}"),
		"processed: {processed}"
	);
}

#[tokio::test]
async fn test_git_placeholders_in_non_git_dir() {
	// A directory with no git repo and no README: the placeholders must
	// resolve (to empty/fallback text), never error or remain unexpanded.
	let tmp = tempfile::tempdir().expect("tempdir");
	let processed = process_placeholders_async(
		"status={{GIT_STATUS}} tree={{GIT_TREE}} readme={{README}}",
		tmp.path(),
	)
	.await;
	assert!(
		!processed.contains("{{GIT_STATUS}}"),
		"processed: {processed}"
	);
	assert!(
		!processed.contains("{{GIT_TREE}}"),
		"processed: {processed}"
	);
	assert!(!processed.contains("{{README}}"), "processed: {processed}");
}

#[tokio::test]
async fn test_get_all_placeholders_exposes_core_keys() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let map = get_all_placeholders(tmp.path()).await;
	for key in [
		"{{DATE}}",
		"{{OS}}",
		"{{SHELL}}",
		"{{SYSTEM}}",
		"{{CONTEXT}}",
	] {
		assert!(map.contains_key(key), "missing placeholder {key}");
	}
}
