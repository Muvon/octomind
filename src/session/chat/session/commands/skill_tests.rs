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
fn test_glob_match() {
	// Trailing wildcard
	assert!(glob_match("git*", "github"));
	assert!(glob_match("git*", "git"));
	assert!(!glob_match("git*", "fugit"));

	// Leading wildcard
	assert!(glob_match("*hub", "github"));
	assert!(!glob_match("*hub", "hubris"));

	// Both sides
	assert!(glob_match("*it*", "github"));
	assert!(glob_match("*it*", "git"));
	assert!(!glob_match("*xyz*", "github"));

	// Case insensitive
	assert!(glob_match("*GIT*", "github"));
	assert!(glob_match("Git*", "github"));

	// Exact (no wildcard)
	assert!(glob_match("git", "git"));
	assert!(!glob_match("git", "github"));
}

const SKILL_NAME: &str = "cov-roundtrip-skill";

/// Temp workdir containing `.agents/skills/<SKILL_NAME>/SKILL.md` so skill
/// discovery has at least one deterministic, machine-independent entry.
fn skill_workdir(tag: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-skill-{tag}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	let skill_dir = dir.join(".agents").join("skills").join(SKILL_NAME);
	std::fs::create_dir_all(&skill_dir).expect("create skill dir");
	std::fs::write(
		skill_dir.join("SKILL.md"),
		format!(
			"---\nname: {SKILL_NAME}\ndescription: Coverage roundtrip skill\n---\n\nFollow the coverage instructions.\n"
		),
	)
	.expect("write SKILL.md");
	dir
}

const PAGE_SKILL_COUNT: usize = 16;

/// Temp workdir containing `cov-page-01` … `cov-page-16` skills, enough to
/// exercise a filtered list across two pages without machine skills leaking in.
fn paginated_skills_workdir(tag: &str) -> std::path::PathBuf {
	let dir = std::env::temp_dir().join(format!("octomind-skill-{tag}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	for index in 1..=PAGE_SKILL_COUNT {
		let name = format!("cov-page-{index:02}");
		let skill_dir = dir.join(".agents").join("skills").join(&name);
		std::fs::create_dir_all(&skill_dir).expect("create skill dir");
		std::fs::write(
			skill_dir.join("SKILL.md"),
			format!("---\nname: {name}\ndescription: Pagination fixture skill\n---\n\nPagination fixture.\n"),
		)
		.expect("write SKILL.md");
	}
	dir
}

fn skill_data(result: CommandResult) -> serde_json::Value {
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Skill { data } = *output else {
		panic!("expected Skill output");
	};
	data
}

#[tokio::test]
async fn page_zero_is_rejected() {
	let mut session = ChatSession::for_tests(Vec::new());
	let result = handle_skill(&mut session, &["0"]).await.expect("dispatch");
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Error { error, .. } = *output else {
		panic!("expected Error output");
	};
	assert_eq!(error, "Page number must be a positive integer");
}

#[tokio::test]
async fn unknown_skill_name_is_reported() {
	let mut session = ChatSession::for_tests(Vec::new());
	let data = skill_data(
		handle_skill(&mut session, &["cov-definitely-missing-skill"])
			.await
			.expect("dispatch"),
	);
	assert_eq!(data["subcommand"], "error");
	assert!(
		data["message"].as_str().expect("message").contains("not found"),
		"unexpected message: {data}"
	);
}

#[tokio::test]
async fn glob_filter_without_matches_lists_empty() {
	let mut session = ChatSession::for_tests(Vec::new());
	let data = skill_data(
		handle_skill(&mut session, &["*zzz-cov-no-match*"])
			.await
			.expect("dispatch"),
	);
	assert_eq!(data["subcommand"], "list");
	assert_eq!(data["total"], 0);
	assert_eq!(data["skills"], serde_json::json!([]));
	assert_eq!(data["pattern"], "*zzz-cov-no-match*");
}

#[tokio::test]
#[serial_test::serial]
async fn use_forget_and_page_range_roundtrip() {
	let session_id = format!("skill-cmd-roundtrip-{}", std::process::id());
	let workdir = skill_workdir("roundtrip");
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::init_session_services("developer");
		crate::mcp::workdir::set_session_working_directory(workdir.clone());

		let mut session = ChatSession::for_tests(Vec::new());

		// List (filtered — the machine's global skills must not crowd
		// the fixture off the page): our skill is present and inactive.
		let data = skill_data(handle_skill(&mut session, &["*cov-roundtrip*"]).await.expect("list"));
		assert_eq!(data["subcommand"], "list");
		let entry = data["skills"]
			.as_array()
			.expect("skills array")
			.iter()
			.find(|s| s["name"] == SKILL_NAME)
			.unwrap_or_else(|| panic!("skill missing from list: {data}"));
		assert_eq!(entry["active"], false);

		// Use: injects the skill content as a system-managed user message.
		let data = skill_data(handle_skill(&mut session, &[SKILL_NAME]).await.expect("use"));
		assert_eq!(data["subcommand"], "use");
		assert_eq!(data["name"], SKILL_NAME);
		assert!(
			session.session.messages.iter().any(|m| m.content.contains(SKILL_NAME)),
			"skill content was not injected"
		);

		// List again (same filter): the skill is now marked active.
		let data = skill_data(handle_skill(&mut session, &["*cov-roundtrip*"]).await.expect("list"));
		let entry = data["skills"]
			.as_array()
			.expect("skills array")
			.iter()
			.find(|s| s["name"] == SKILL_NAME)
			.unwrap_or_else(|| panic!("skill missing from list: {data}"));
		assert_eq!(entry["active"], true);

		// Forget: toggles the active skill off.
		let data = skill_data(handle_skill(&mut session, &[SKILL_NAME]).await.expect("forget"));
		assert_eq!(data["subcommand"], "forget");
		assert_eq!(data["name"], SKILL_NAME);

		// Out-of-range page: at least one skill exists, so page 9999 errors.
		let result = handle_skill(&mut session, &["9999"]).await.expect("page");
		let CommandResult::HandledWithOutput(output) = result else {
			panic!("expected typed output");
		};
		let CommandOutput::Error { error, context } = *output else {
			panic!("expected Error output");
		};
		assert!(error.contains("Page 9999 not found"), "{error}");
		assert_eq!(context.expect("context")["page"], 9999);

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn filtered_list_pages_past_first_page() {
	let session_id = format!("skill-cmd-pagination-{}", std::process::id());
	let workdir = paginated_skills_workdir("pagination");
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::context::init_session_services("developer");
		crate::mcp::workdir::set_session_working_directory(workdir.clone());

		let mut session = ChatSession::for_tests(Vec::new());

		// No page argument: the filtered list defaults to page 1.
		let data = skill_data(handle_skill(&mut session, &["*cov-page-*"]).await.expect("page 1"));
		assert_eq!(data["subcommand"], "list");
		assert_eq!(data["page"], 1);
		assert_eq!(data["total"].as_u64(), Some(PAGE_SKILL_COUNT as u64));
		assert_eq!(data["pattern"], "*cov-page-*");

		// A page argument pages the filtered list instead of resetting to page 1.
		let data = skill_data(handle_skill(&mut session, &["*cov-page-*", "2"]).await.expect("page 2"));
		assert_eq!(data["subcommand"], "list");
		assert_eq!(data["page"], 2);
		assert_eq!(data["total"].as_u64(), Some(PAGE_SKILL_COUNT as u64));
		assert_eq!(data["pattern"], "*cov-page-*");
		let skills = data["skills"].as_array().expect("skills array");
		assert_eq!(skills.len(), 1, "page 2 should hold the last fixture: {data}");
		assert_eq!(skills[0]["name"], "cov-page-16");

		// Page 0 is rejected for filtered lists too.
		let result = handle_skill(&mut session, &["*cov-page-*", "0"])
			.await
			.expect("page zero");
		let CommandResult::HandledWithOutput(output) = result else {
			panic!("expected typed output");
		};
		let CommandOutput::Error { error, .. } = *output else {
			panic!("expected Error output");
		};
		assert_eq!(error, "Page number must be a positive integer");

		crate::session::context::cleanup_session(&session_id);
	})
	.await;
}
