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

#[tokio::test]
async fn test_process_placeholders_system_and_binaries() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let processed = process_placeholders_async("sys={{SYSTEM}} bin={{BINARIES}}", tmp.path()).await;
	assert!(!processed.contains("{{SYSTEM}}"), "processed: {processed}");
	assert!(
		!processed.contains("{{BINARIES}}"),
		"processed: {processed}"
	);
	assert!(processed.contains("==== SYSTEM INFORMATION ===="));
	assert!(processed.contains("==== END SYSTEM INFORMATION ===="));
	assert!(processed.contains("**Shell**"));
}

/// Real git repo: every project placeholder resolves to its populated section.
#[tokio::test]
async fn test_git_placeholders_in_real_repo() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let base = tmp.path();
	std::fs::write(base.join("README.md"), "# Test Project\nhello readme").expect("write readme");
	std::fs::create_dir(base.join("src")).expect("mkdir");
	std::fs::write(base.join("src/main.rs"), "fn main() {}").expect("write src");
	// init + add is enough: status/ls-files work without a commit identity
	std::process::Command::new("git")
		.args(["init", "-q"])
		.current_dir(base)
		.output()
		.expect("git init");
	std::process::Command::new("git")
		.args(["add", "-A"])
		.current_dir(base)
		.output()
		.expect("git add");

	let processed = process_placeholders_async(
		"s={{GIT_STATUS}} t={{GIT_TREE}} r={{README}} c={{CONTEXT}}",
		base,
	)
	.await;
	assert!(
		processed.contains("==== GIT STATUS ===="),
		"processed: {processed}"
	);
	assert!(
		processed.contains("==== FILE TREE ===="),
		"processed: {processed}"
	);
	assert!(processed.contains("src/main.rs"), "processed: {processed}");
	assert!(
		processed.contains("==== README ===="),
		"processed: {processed}"
	);
	assert!(processed.contains("hello readme"), "processed: {processed}");
	assert!(
		processed.contains("==== PROJECT CONTEXT ===="),
		"processed: {processed}"
	);
}

#[tokio::test]
async fn test_get_all_placeholders_in_git_repo() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let base = tmp.path();
	std::fs::write(base.join("README.md"), "# Repo\nreadme body").expect("write readme");
	std::process::Command::new("git")
		.args(["init", "-q"])
		.current_dir(base)
		.output()
		.expect("git init");
	std::process::Command::new("git")
		.args(["add", "-A"])
		.current_dir(base)
		.output()
		.expect("git add");

	let map = get_all_placeholders(base).await;
	assert!(map.contains_key("{{HOME}}"));
	let status = &map["{{GIT_STATUS}}"];
	assert!(status.contains("==== GIT STATUS ===="), "status: {status}");
	assert!(map["{{GIT_TREE}}"].contains("README.md"));
	assert!(map["{{README}}"].contains("readme body"));
	assert!(map["{{CONTEXT}}"].contains("==== PROJECT CONTEXT ===="));
}

#[tokio::test]
async fn test_get_command_version_outcomes() {
	// A real tool reports its version; an unknown binary falls through every
	// probe to "missing".
	let git_version = get_command_version("git").await;
	assert_ne!(git_version, "missing");
	assert!(!git_version.is_empty());
	assert_eq!(
		get_command_version("definitely_missing_tool_xyz").await,
		"missing"
	);
}

#[tokio::test]
async fn test_gather_system_info_shape() {
	let info = gather_system_info().await;
	assert!(!info.date_with_timezone.is_empty());
	assert!(!info.shell_info.is_empty());
	assert!(!info.os_info.is_empty());
	assert!(info.os_info.contains("os:"));
	assert!(!info.binaries.is_empty());
	// One line per probed binary, present or missing
	assert!(info.binaries.lines().any(|l| l.starts_with("git:")));
}
