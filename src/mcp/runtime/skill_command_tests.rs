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

//! Tests for the `skill` tool against a real on-disk skill: a tempdir
//! project workdir carrying `.agents/skills/<name>/SKILL.md` (the
//! universal-skill discovery path — no tap install needed). Everything runs
//! inside a `with_session_id` scope: the session-scoped workdir registry is
//! the only one settable without prior init, and `use` requires an active
//! session.

use super::*;
use serial_test::serial;

const SKILL_NAME: &str = "skilltest-widgets";
const INSTRUCTIONS_MARKER: &str = "SKILLTEST-INSTRUCTIONS-MARKER";

fn skill_call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "skill".to_string(),
		parameters: params,
		tool_id: "t-skill".to_string(),
	}
}

fn text_of(result: &McpToolResult) -> String {
	result
		.result
		.content
		.iter()
		.filter_map(|block| match block {
			rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect()
}

fn is_err(result: &McpToolResult) -> bool {
	result.result.is_error.unwrap_or(false)
}

/// Create a tempdir with one project skill and register it as the scoped
/// session's working directory. The TempDir must stay alive for the test.
fn skill_workdir(session_id: &str) -> tempfile::TempDir {
	let tmp = tempfile::tempdir().expect("tempdir");
	let skill_dir = tmp.path().join(".agents/skills").join(SKILL_NAME);
	std::fs::create_dir_all(&skill_dir).expect("skill dir");
	std::fs::write(
		skill_dir.join("SKILL.md"),
		format!(
			"---\nname: {SKILL_NAME}\ndescription: Testing widget skills end to end\nallowed-tools: shell view\n---\n\n# Widget skill\n{INSTRUCTIONS_MARKER}: always widget carefully.\n"
		),
	)
	.expect("write SKILL.md");
	// set_session_workdir INSERTS the registry entry; set_current_workdir
	// only updates one that already exists.
	crate::session::context::set_session_workdir(&session_id.to_string(), tmp.path().to_path_buf());
	tmp
}

#[tokio::test]
#[serial]
async fn test_skill_tool_validation_arms() {
	let sid = "__skilltest_validation".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = skill_workdir(&sid);

		let result = execute_skill_tool(&skill_call(serde_json::json!({})))
			.await
			.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("action"));

		let result = execute_skill_tool(&skill_call(serde_json::json!({"action": 42})))
			.await
			.expect("dispatch");
		assert!(is_err(&result));

		let result = execute_skill_tool(&skill_call(serde_json::json!({"action": "explode"})))
			.await
			.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("unknown action"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_discovery_and_list() {
	let sid = "__skilltest_discovery".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = skill_workdir(&sid);

		let found = find_all_skills_with_details();
		assert!(
			found.iter().any(|(meta, _)| meta.name == SKILL_NAME),
			"project skill not discovered: {:?}",
			found
				.iter()
				.map(|(m, _)| m.name.clone())
				.collect::<Vec<_>>()
		);

		let (meta, _, content) =
			find_skill_by_name_pub(SKILL_NAME).expect("skill resolvable by name");
		assert_eq!(meta.allowed_tools, vec!["shell", "view"]);
		assert!(content.contains(INSTRUCTIONS_MARKER));

		// list with a matching pattern includes it; a nonsense pattern does not
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "list", "pattern": "widget"}),
		))
		.await
		.expect("list dispatches");
		assert!(!is_err(&result));
		assert!(text_of(&result).contains(SKILL_NAME));

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "list", "pattern": "no-such-skill-anywhere"}),
		))
		.await
		.expect("list dispatches");
		assert!(!is_err(&result));
		assert!(text_of(&result).contains("No skills found"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_and_forget_round_trip() {
	let sid = "__skilltest_use".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = skill_workdir(&sid);

		// use of an unknown skill is a structured error
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "__skilltest_nope"}),
		))
		.await
		.expect("dispatch");
		assert!(is_err(&result), "got: {}", text_of(&result));

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": SKILL_NAME}),
		))
		.await
		.expect("use dispatches");
		assert!(!is_err(&result), "use failed: {}", text_of(&result));

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "forget", "name": SKILL_NAME}),
		))
		.await
		.expect("forget dispatches");
		assert!(!is_err(&result), "forget failed: {}", text_of(&result));

		// Skill-message helpers recognise the injected wrapper format
		let wrapped = format!("<skill name=\"{SKILL_NAME}\">body</skill>");
		assert!(is_skill_message(&wrapped));
		assert_eq!(extract_skill_name(&wrapped), Some(SKILL_NAME));
		assert!(!is_skill_message("plain user text"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}
