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

/// OCTOMIND_SKILLS env loading: the named project skill is force-activated
/// and its instructions land in the session; unknown names fail without
/// aborting; a second load is an idempotent no-op.
#[tokio::test]
#[serial]
async fn test_load_env_skills_injects_and_is_idempotent() {
	let sid = "__skilltest_env_load".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = skill_workdir(&sid);
		let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());

		std::env::set_var(
			"OCTOMIND_SKILLS",
			format!("{SKILL_NAME}, ,__skilltest_env_nope"),
		);
		crate::mcp::runtime::skill_auto::load_env_skills(&mut session).await;

		let injected = session
			.session
			.messages
			.iter()
			.filter(|m| m.content.contains(INSTRUCTIONS_MARKER))
			.count();
		assert_eq!(injected, 1, "skill instructions must be injected once");

		// Second load: the active-skill guard suppresses re-injection
		crate::mcp::runtime::skill_auto::load_env_skills(&mut session).await;
		std::env::remove_var("OCTOMIND_SKILLS");
		let injected = session
			.session
			.messages
			.iter()
			.filter(|m| m.content.contains(INSTRUCTIONS_MARKER))
			.count();
		assert_eq!(injected, 1, "env skill loading must be idempotent");
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

/// Multi-skill fixture: three project skills with distinct metadata so list
/// pagination, markers, and the compatibility line are all observable. The
/// pattern filter scopes assertions to exactly these three regardless of what
/// taps exist on the machine.
fn multi_skill_workdir(session_id: &str) -> tempfile::TempDir {
	let tmp = tempfile::tempdir().expect("tempdir");
	let base = tmp.path().join(".agents/skills");
	for (name, extra, body) in [
		("skilltest-page-a", "", "Alpha paging instructions"),
		(
			"skilltest-page-b",
			"\ncompatibility: developer",
			"Beta paging instructions",
		),
		(
			"skilltest-page-c",
			"\nallowed-tools: no_such_tool_xyz",
			"Gamma paging instructions",
		),
	] {
		let dir = base.join(name);
		std::fs::create_dir_all(&dir).expect("skill dir");
		std::fs::write(
			dir.join("SKILL.md"),
			format!(
				"---\nname: {name}\ndescription: Paging test skill {name}{extra}\n---\n\n{body}\n"
			),
		)
		.expect("write SKILL.md");
	}
	crate::session::context::set_session_workdir(&session_id.to_string(), tmp.path().to_path_buf());
	tmp
}

#[tokio::test]
#[serial]
async fn test_skill_list_pagination_and_markers() {
	let sid = "__skilltest_paging".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = multi_skill_workdir(&sid);

		// Page 1 of 2: totals, page hint with the next offset.
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "list", "pattern": "skilltest-page", "limit": 2}),
		))
		.await
		.expect("dispatch");
		let msg = text_of(&result);
		assert!(
			msg.contains("Found 3 skill(s) matching pattern:"),
			"got: {msg}"
		);
		assert!(
			msg.contains("Showing 1-2 of 3. Use offset=2 to see more."),
			"got: {msg}"
		);

		// Page 2: last skill, no further-pages hint.
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "list", "pattern": "skilltest-page", "offset": 2, "limit": 2}),
		))
		.await
		.expect("dispatch");
		let msg = text_of(&result);
		// find_all_skills() yields readdir order (unspecified on ext4), so
		// page 2 must show exactly the one remaining skill — whichever it is.
		assert_eq!(msg.matches("**skilltest-page-").count(), 1, "got: {msg}");
		assert!(!msg.contains("Use offset="), "got: {msg}");

		// Active marker reflects the session's active-skill set.
		crate::session::context::add_active_skill(&sid, "skilltest-page-a");
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "list", "pattern": "skilltest-page"}),
		))
		.await
		.expect("dispatch");
		let msg = text_of(&result);
		assert!(
			msg.contains("**skilltest-page-a** ✓ [active]"),
			"got: {msg}"
		);
		crate::session::context::remove_active_skill(&sid, "skilltest-page-a");

		// Compat marker: allowed-tools entry absent from the tool map.
		assert!(
			msg.contains("**skilltest-page-c** ⚠️ [missing tools: no_such_tool_xyz]"),
			"got: {msg}"
		);
		// Compatibility line comes from frontmatter.
		assert!(msg.contains("Compatibility: developer"), "got: {msg}");
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_silent_round_trip() {
	let sid = "__skilltest_silent".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = skill_workdir(&sid);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use_silent", "name": SKILL_NAME}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use_silent failed: {}", text_of(&result));
		assert!(text_of(&result).contains("now active"));
		assert!(crate::session::context::has_active_skill(&sid, SKILL_NAME));

		// Silent mode stashes the wrapped body for the caller to inject.
		let content = take_silent_skill_content().expect("silent content stored");
		assert!(
			content.contains(&format!("<skill name=\"{SKILL_NAME}\"")),
			"got: {content}"
		);
		assert!(content.contains(INSTRUCTIONS_MARKER), "got: {content}");
		assert!(content.contains("</skill>"), "got: {content}");
		// Taking is destructive — a second take returns None.
		assert!(take_silent_skill_content().is_none());

		// Second silent use reports already-active without re-injecting.
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use_silent", "name": SKILL_NAME}),
		))
		.await
		.expect("dispatch");
		assert!(text_of(&result).contains("already active"));

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "forget", "name": SKILL_NAME}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "forget failed: {}", text_of(&result));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_forget_validation_and_offload() {
	let sid = "__skilltest_offload".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let _tmp = skill_workdir(&sid);

		// name validation arms for use and forget.
		for action in ["use", "forget"] {
			for params in [
				serde_json::json!({"action": action, "name": 42}),
				serde_json::json!({"action": action, "name": "   "}),
				serde_json::json!({"action": action}),
			] {
				let result = execute_skill_tool(&skill_call(params))
					.await
					.expect("dispatch");
				assert!(
					is_err(&result),
					"{action} must reject: {}",
					text_of(&result)
				);
				assert!(
					text_of(&result).contains("name"),
					"{action} validation: {}",
					text_of(&result)
				);
			}
		}

		// forget of a non-active skill is a structured error.
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "forget", "name": "__skilltest_inactive"}),
		))
		.await
		.expect("dispatch");
		assert!(is_err(&result));
		assert!(text_of(&result).contains("not currently active"));

		// Offload path: a skill that "loaded" a server forgets → refcount hits
		// zero → server disabled + removed, and the message names it.
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": SKILL_NAME}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use failed: {}", text_of(&result));
		crate::session::context::set_skill_capability_servers(
			&sid,
			SKILL_NAME,
			vec!["__skilltest_offload_srv".to_string()],
		);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "forget", "name": SKILL_NAME}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "forget failed: {}", text_of(&result));
		assert!(
			text_of(&result).contains("offloaded servers: __skilltest_offload_srv"),
			"got: {}",
			text_of(&result)
		);
		assert!(!crate::session::context::has_active_skill(&sid, SKILL_NAME));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_unknown_capability_warns_but_activates() {
	let sid = "__skilltest_capwarn".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let tmp = tempfile::tempdir().expect("tempdir");
		let skill_dir = tmp.path().join(".agents/skills").join("skilltest-caps");
		std::fs::create_dir_all(&skill_dir).expect("skill dir");
		std::fs::write(
			skill_dir.join("SKILL.md"),
			"---\nname: skilltest-caps\ndescription: Capability warning fixture\ncapabilities: __skilltest_nocap\n---\n\nBody\n",
		)
		.expect("write SKILL.md");
		crate::session::context::set_session_workdir(&sid.to_string(), tmp.path().to_path_buf());

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-caps"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use must still succeed: {}", text_of(&result));
		assert!(
			text_of(&result).contains("⚠️ Capability '__skilltest_nocap' not found"),
			"got: {}",
			text_of(&result)
		);
		assert!(crate::session::context::has_active_skill(&sid, "skilltest-caps"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_and_forget_require_active_session() {
	// Outside any with_session_id scope both actions refuse with a clear error.
	for action in ["use", "forget"] {
		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": action, "name": "any-skill"}),
		))
		.await
		.expect("dispatch");
		assert!(
			is_err(&result),
			"{action} outside session must error: {}",
			text_of(&result)
		);
		assert!(
			text_of(&result).contains("requires an active session"),
			"{action}: {}",
			text_of(&result)
		);
	}
}
/// Point `OCTOMIND_DATA_DIR` at a tempdir so tap capability resolution sees
/// only fixtures. Tests using it must be `#[serial]` (env is process-global).
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

#[tokio::test]
#[serial]
async fn test_skill_use_warns_on_unavailable_allowed_tools() {
	let sid = "__skilltest_toolwarn".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let tmp = tempfile::tempdir().expect("tempdir");
		let skill_dir = tmp.path().join(".agents/skills").join("skilltest-toolwarn");
		std::fs::create_dir_all(&skill_dir).expect("skill dir");
		std::fs::write(
			skill_dir.join("SKILL.md"),
			"---\nname: skilltest-toolwarn\ndescription: Missing tool fixture\nallowed-tools: __skilltest_no_such_tool\n---\n\nBody\n",
		)
		.expect("write SKILL.md");
		crate::session::context::set_session_workdir(&sid.to_string(), tmp.path().to_path_buf());

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-toolwarn"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use must succeed: {}", text_of(&result));
		assert!(
			text_of(&result)
				.contains("⚠️ Some tools still unavailable after capability loading: __skilltest_no_such_tool"),
			"got: {}",
			text_of(&result)
		);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_loads_capability_with_config_level_server() {
	let _guard = DataDirGuard::new();
	let sid = "__skilltest_capload".to_string();

	// A tap-provided capability whose server is ALSO a config-level server:
	// enable_skill_server must count it as loaded but skip registration.
	let tap_cap = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("capabilities")
		.join("skilltest-cap");
	std::fs::create_dir_all(&tap_cap).expect("tap cap dir");
	std::fs::write(tap_cap.join("config.toml"), "triggers = [\"x\"]\n").expect("config.toml");
	std::fs::write(
		tap_cap.join("default.toml"),
		"[[mcp.servers]]\nname = \"skilltest-cap-srv\"\ntype = \"builtin\"\ntimeout_seconds = 30\ntools = []\n",
	)
	.expect("default.toml");

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::builtin(
			"skilltest-cap-srv",
			30,
			vec![],
		));
	crate::session::context::set_session_config(&sid, &config);

	crate::session::context::with_session_id(sid.clone(), async {
		let tmp = tempfile::tempdir().expect("tempdir");
		let skill_dir = tmp.path().join(".agents/skills").join("skilltest-capload");
		std::fs::create_dir_all(&skill_dir).expect("skill dir");
		std::fs::write(
			skill_dir.join("SKILL.md"),
			"---\nname: skilltest-capload\ndescription: Capability loading fixture\ncapabilities: skilltest-cap\n---\n\nBody\n",
		)
		.expect("write SKILL.md");
		crate::session::context::set_session_workdir(&sid.to_string(), tmp.path().to_path_buf());

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-capload"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use failed: {}", text_of(&result));
		assert!(
			text_of(&result).contains("Loaded capability 'skilltest-cap' (servers: skilltest-cap-srv)"),
			"got: {}",
			text_of(&result)
		);

		// Config-level skip: no dynamic registration, no skill-owned servers.
		assert!(crate::session::context::get_dynamic_server_for_session(
			&sid,
			"skilltest-cap-srv"
		)
		.is_none());
		let owned = crate::session::context::take_skill_capability_servers(&sid, "skilltest-capload");
		assert!(owned.is_empty(), "config-level servers are not skill-owned");
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_forget_shared_server_keeps_it() {
	let sid = "__skilltest_shared".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let tmp = tempfile::tempdir().expect("tempdir");
		let skill_dir = tmp.path().join(".agents/skills").join("skilltest-shared-a");
		std::fs::create_dir_all(&skill_dir).expect("skill dir");
		std::fs::write(
			skill_dir.join("SKILL.md"),
			"---\nname: skilltest-shared-a\ndescription: Shared server fixture\n---\n\nBody\n",
		)
		.expect("write SKILL.md");
		crate::session::context::set_session_workdir(&sid.to_string(), tmp.path().to_path_buf());

		// Two skills share one server with refcount 2: forgetting one must
		// leave the server in place for the other.
		crate::session::context::add_active_skill(&sid, "skilltest-shared-a");
		crate::session::context::add_active_skill(&sid, "skilltest-shared-b");
		for skill in ["skilltest-shared-a", "skilltest-shared-b"] {
			crate::session::context::set_skill_capability_servers(
				&sid,
				skill,
				vec!["__skilltest_shared_srv".to_string()],
			);
		}
		crate::session::context::increment_capability_refcount(&sid, "__skilltest_shared_srv");
		crate::session::context::increment_capability_refcount(&sid, "__skilltest_shared_srv");

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "forget", "name": "skilltest-shared-a"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "forget failed: {}", text_of(&result));
		assert!(
			!text_of(&result).contains("offloaded servers"),
			"shared server must not be offloaded: {}",
			text_of(&result)
		);

		// The surviving skill still owns the server.
		let remaining =
			crate::session::context::take_skill_capability_servers(&sid, "skilltest-shared-b");
		assert_eq!(remaining, vec!["__skilltest_shared_srv".to_string()]);
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

// ---------------------------------------------------------------------------
// Wave-1 coverage additions: multi-source discovery (universal dirs, agent
// plugins, evolution-generated skills), empty-list message, capability
// env-gating / refcount / enable-failure branches, plugin server loading,
// resource-catalog injection, and forget offload of an untracked server.
// ---------------------------------------------------------------------------

/// Pins HOME to a tempdir so the global universal/plugin dirs stay empty.
struct HomeGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl HomeGuard {
	fn new() -> Self {
		let previous = std::env::var_os("HOME");
		let dir = tempfile::tempdir().expect("home tempdir");
		std::env::set_var("HOME", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for HomeGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("HOME", v),
			None => std::env::remove_var("HOME"),
		}
	}
}

/// Write a tap capability fixture: `capabilities/<name>/{config,default}.toml`.
fn write_tap_capability(name: &str, default_toml: &str) {
	let cap_dir = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("capabilities")
		.join(name);
	std::fs::create_dir_all(&cap_dir).expect("cap dir");
	std::fs::write(cap_dir.join("config.toml"), "triggers = [\"x\"]\n").expect("config.toml");
	std::fs::write(cap_dir.join("default.toml"), default_toml).expect("default.toml");
}

/// Write a project skill into `<workdir>/.agents/skills/<name>/SKILL.md`.
fn write_project_skill(workdir: &std::path::Path, name: &str, frontmatter: &str) {
	let skill_dir = workdir.join(".agents").join("skills").join(name);
	std::fs::create_dir_all(&skill_dir).expect("skill dir");
	std::fs::write(
		skill_dir.join("SKILL.md"),
		format!("---\nname: {name}\n{frontmatter}---\n\n{name}-BODY\n"),
	)
	.expect("write SKILL.md");
}

#[tokio::test]
#[serial]
async fn test_find_skills_across_universal_plugin_and_evolution_sources() {
	let _data = DataDirGuard::new();
	let _home = HomeGuard::new();
	let sid = "__skilltest_multisource".to_string();

	// Universal (project) skill.
	let workdir = tempfile::tempdir().expect("workdir");
	write_project_skill(
		workdir.path(),
		"skilltest-uni",
		"description: Universal fixture\n",
	);

	// Agent plugin with one skill.
	let plugin_root = workdir.path().join(".agents").join("plugins").join("plug1");
	std::fs::create_dir_all(plugin_root.join("skills").join("skilltest-plug")).expect("dirs");
	std::fs::write(
		plugin_root.join("plugin.json"),
		serde_json::json!({
			"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
			"name": "plug1"
		})
		.to_string(),
	)
	.expect("plugin.json");
	std::fs::write(
		plugin_root
			.join("skills")
			.join("skilltest-plug")
			.join("SKILL.md"),
		"---\nname: skilltest-plug\ndescription: Plugin fixture\n---\n\nPLUG-BODY\n",
	)
	.expect("write SKILL.md");

	// Evolution-generated skill: registry record + artifact on disk.
	let evo_dir = crate::directories::get_learning_evolution_dir().expect("evolution dir");
	let artifact = evo_dir.join("evo1").join("artifact");
	std::fs::create_dir_all(&artifact).expect("artifact dir");
	std::fs::write(
		artifact.join("SKILL.md"),
		"---\nname: skilltest-evo\ndescription: Generated fixture\n---\n\nEVO-BODY\n",
	)
	.expect("write SKILL.md");
	let registry = serde_json::json!({
		"schema_version": 1,
		"records": [{
			"schema_version": 1,
			"id": "evo1",
			"name": "skilltest-evo",
			"description": "generated skill",
			"kind": "skill",
			"scope": { "project": null, "domain": null },
			"state": "trial",
			"effect": "advisory",
			"explicit_authorization": true,
			"source_memory_ids": [],
			"evidence": [],
			"artifact_version": 1,
			"parent_version": null,
			"superseded_ids": [],
			"generator_model": "m",
			"verifier_model": "v",
			"artifact_path": "SKILL.md",
			"script_path": null,
			"shadow_matches": 0,
			"trial_uses": 0,
			"successes": 0,
			"failures": 0,
			"false_triggers": 0,
			"created": "2026-01-01T00:00:00Z",
			"updated": "2026-01-01T00:00:00Z",
			"promoted": null,
			"last_used": null,
			"retired": null,
			"history": []
		}]
	});
	std::fs::write(
		evo_dir.join("registry.json"),
		serde_json::to_string(&registry).expect("registry json"),
	)
	.expect("write registry");

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.supervisor.learning.evolution.enabled = true;
	crate::session::context::set_session_config(&sid, &config);

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);
		crate::supervisor::learning::evolution::init_for_session("developer");

		let names: Vec<String> = find_all_skills_with_details()
			.into_iter()
			.map(|(meta, _)| meta.name)
			.collect();
		for expected in ["skilltest-uni", "skilltest-plug", "skilltest-evo"] {
			assert!(
				names.iter().any(|n| n == expected),
				"{expected} missing from discovery: {names:?}"
			);
		}

		// Each source also resolves by name through its own branch.
		let (_, _, uni) = find_skill_by_name_pub("skilltest-uni").expect("universal resolves");
		assert!(uni.contains("skilltest-uni-BODY"));
		let (_, dir, plug) = find_skill_by_name_pub("skilltest-plug").expect("plugin resolves");
		assert!(plug.contains("PLUG-BODY"));
		assert!(dir.starts_with(&plugin_root), "plugin skill dir: {dir:?}");
		let (_, _, evo) = find_skill_by_name_pub("skilltest-evo").expect("evolution resolves");
		assert!(evo.contains("EVO-BODY"));
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_list_empty_reports_no_skills() {
	let _data = DataDirGuard::new();
	let _home = HomeGuard::new();
	let sid = "__skilltest_listempty".to_string();

	let workdir = tempfile::tempdir().expect("workdir");
	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);

		let result = execute_skill_tool(&skill_call(serde_json::json!({"action": "list"})))
			.await
			.expect("dispatch");
		assert!(!is_err(&result), "list failed: {}", text_of(&result));
		assert_eq!(
			text_of(&result),
			"No skills found. Add skills to your tap under skills/<name>/SKILL.md"
		);
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_env_gated_capability_skips_with_warning() {
	let _data = DataDirGuard::new();
	let sid = "__skilltest_envcap".to_string();

	write_tap_capability(
		"skilltest-envcap",
		"[[mcp.servers]]\nname = \"skilltest-envcap-srv\"\ntype = \"stdio\"\ncommand = \"echo\"\nargs = []\ntimeout_seconds = 30\ntools = []\nenv = { TOKEN = \"{{ENV:SKILLTEST_MISSING_VAR}}\" }\n",
	);

	let workdir = tempfile::tempdir().expect("workdir");
	write_project_skill(
		workdir.path(),
		"skilltest-envcap-skill",
		"description: Env-gated capability fixture\ncapabilities: skilltest-envcap\n",
	);

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-envcap-skill"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use failed: {}", text_of(&result));
		assert!(
			text_of(&result).contains(
				"⚠️ Capability 'skilltest-envcap' skipped — missing env vars: SKILLTEST_MISSING_VAR"
			),
			"got: {}",
			text_of(&result)
		);
		// The skill still activates without the unconfigured capability.
		assert!(crate::session::context::has_active_skill(
			&sid,
			"skilltest-envcap-skill"
		));
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_refcounts_already_enabled_server() {
	let _data = DataDirGuard::new();
	let sid = "__skilltest_refcap".to_string();

	write_tap_capability(
		"skilltest-refcap",
		"[[mcp.servers]]\nname = \"skilltest-refcap-srv\"\ntype = \"builtin\"\ntimeout_seconds = 30\ntools = []\n",
	);

	let workdir = tempfile::tempdir().expect("workdir");
	write_project_skill(
		workdir.path(),
		"skilltest-refcap-skill",
		"description: Refcount capability fixture\ncapabilities: skilltest-refcap\n",
	);

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);

		// Pre-register and mark the server enabled in the session registry so
		// the skill hits the already-enabled refcount branch instead of a
		// fresh enable (builtin servers cannot go through enable_server).
		crate::mcp::runtime::dynamic::register_server(crate::config::McpServerConfig::builtin(
			"skilltest-refcap-srv",
			30,
			vec![],
		))
		.expect("register");
		crate::session::context::enable_dynamic_server_for_session(
			&sid,
			"skilltest-refcap-srv",
			Vec::new(),
		);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-refcap-skill"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use failed: {}", text_of(&result));
		assert!(
			text_of(&result)
				.contains("Loaded capability 'skilltest-refcap' (servers: skilltest-refcap-srv)"),
			"got: {}",
			text_of(&result)
		);

		// The skill now owns the server for forget-time offloading.
		let owned =
			crate::session::context::take_skill_capability_servers(&sid, "skilltest-refcap-skill");
		assert_eq!(owned, vec!["skilltest-refcap-srv".to_string()]);
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_capability_enable_failures_fail_open() {
	let _data = DataDirGuard::new();
	let sid = "__skilltest_capfail".to_string();

	// Empty command: registration itself is rejected.
	write_tap_capability(
		"skilltest-badreg",
		"[[mcp.servers]]\nname = \"skilltest-badreg-srv\"\ntype = \"stdio\"\ncommand = \"\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);
	// Valid registration, but the command cannot be spawned.
	write_tap_capability(
		"skilltest-broken",
		"[[mcp.servers]]\nname = \"skilltest-broken-srv\"\ntype = \"stdio\"\ncommand = \"definitely-not-a-real-binary-xyz\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);

	let workdir = tempfile::tempdir().expect("workdir");
	write_project_skill(
		workdir.path(),
		"skilltest-capfail-skill",
		"description: Broken capability fixture\ncapabilities: skilltest-badreg skilltest-broken\n",
	);

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-capfail-skill"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use must fail open: {}", text_of(&result));
		let text = text_of(&result);
		// Both capabilities must RESOLVE (no "not found") so the assertions
		// below prove the registration/enable failure branches, not a parse skip.
		assert!(
			!text.contains("not found"),
			"capabilities failed to resolve: {text}"
		);
		assert!(
			!text.contains("Loaded capability 'skilltest-badreg'"),
			"unregistrable server must not load: {text}"
		);
		assert!(
			!text.contains("Loaded capability 'skilltest-broken'"),
			"unspawnable server must not load: {text}"
		);
		assert!(crate::session::context::has_active_skill(
			&sid,
			"skilltest-capfail-skill"
		));
		// Neither broken server became skill-owned.
		let owned =
			crate::session::context::take_skill_capability_servers(&sid, "skilltest-capfail-skill");
		assert!(
			owned.is_empty(),
			"broken servers must not be owned: {owned:?}"
		);
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_plugin_servers_fail_open() {
	let _data = DataDirGuard::new();
	let _home = HomeGuard::new();
	let sid = "__skilltest_plugfail".to_string();

	let workdir = tempfile::tempdir().expect("workdir");
	let plugin_root = workdir.path().join(".agents").join("plugins").join("plug2");
	std::fs::create_dir_all(plugin_root.join("skills").join("skilltest-plugfail")).expect("dirs");
	std::fs::write(
		plugin_root.join("plugin.json"),
		serde_json::json!({
			"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
			"name": "plug2"
		})
		.to_string(),
	)
	.expect("plugin.json");
	std::fs::write(
		plugin_root
			.join("skills")
			.join("skilltest-plugfail")
			.join("SKILL.md"),
		"---\nname: skilltest-plugfail\ndescription: Plugin server fixture\n---\n\nPLUGFAIL-BODY\n",
	)
	.expect("write SKILL.md");
	// mcp.json with an unspawnable stdio server: the plugin branch must run
	// and fail open — the skill activates without the server.
	std::fs::write(
		plugin_root.join("mcp.json"),
		serde_json::json!({
			"$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
			"mcpServers": {
				"plug2-srv": {
					"type": "stdio",
					"command": "no-such-binary-xyz",
					"args": [],
					"env": {}
				}
			}
		})
		.to_string(),
	)
	.expect("mcp.json");

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-plugfail"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use must fail open: {}", text_of(&result));
		let text = text_of(&result);
		assert!(
			!text.contains("Loaded plugin 'plug2'"),
			"unspawnable plugin server must not load: {text}"
		);
		assert!(crate::session::context::has_active_skill(
			&sid,
			"skilltest-plugfail"
		));
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_use_appends_resource_catalog() {
	let _data = DataDirGuard::new();
	let _home = HomeGuard::new();
	let sid = "__skilltest_resources".to_string();

	let workdir = tempfile::tempdir().expect("workdir");
	let skill_dir = workdir
		.path()
		.join(".agents")
		.join("skills")
		.join("skilltest-res");
	std::fs::create_dir_all(skill_dir.join("scripts")).expect("scripts dir");
	std::fs::write(
		skill_dir.join("SKILL.md"),
		"---\nname: skilltest-res\ndescription: Resource catalog fixture\n---\n\nRES-BODY\n",
	)
	.expect("write SKILL.md");
	std::fs::write(
		skill_dir.join("scripts").join("hello.sh"),
		"#!/bin/sh\necho hi\n",
	)
	.expect("write script");

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);
		crate::session::inbox::init_inbox_for_session();

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "use", "name": "skilltest-res"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "use failed: {}", text_of(&result));

		// Non-silent use queues the injection (with resource catalog) in the
		// session inbox for the main loop to process.
		let injected =
			crate::session::inbox::try_pop_inbox_message().expect("skill content queued in inbox");
		assert!(
			injected.content.contains("## Skill Resources"),
			"resource catalog missing from injection: {}",
			injected.content
		);
		assert!(
			injected.content.contains("hello.sh"),
			"script entry missing from catalog: {}",
			injected.content
		);
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn test_skill_forget_offload_failure_still_reports() {
	let _data = DataDirGuard::new();
	let _home = HomeGuard::new();
	let sid = "__skilltest_ghostoffload".to_string();

	let workdir = tempfile::tempdir().expect("workdir");
	write_project_skill(
		workdir.path(),
		"skilltest-ghost",
		"description: Ghost server fixture\n",
	);

	crate::session::context::with_session_id(sid.clone(), async {
		crate::session::context::set_session_workdir(
			&sid.to_string(),
			workdir.path().to_path_buf(),
		);

		crate::session::context::add_active_skill(&sid, "skilltest-ghost");
		// A server nobody registered: disable fails (logged), remove returns
		// None, and the offload is still reported.
		crate::session::context::set_skill_capability_servers(
			&sid,
			"skilltest-ghost",
			vec!["skilltest-ghost-srv".to_string()],
		);

		let result = execute_skill_tool(&skill_call(
			serde_json::json!({"action": "forget", "name": "skilltest-ghost"}),
		))
		.await
		.expect("dispatch");
		assert!(!is_err(&result), "forget failed: {}", text_of(&result));
		assert!(
			text_of(&result).contains("offloaded servers: skilltest-ghost-srv"),
			"got: {}",
			text_of(&result)
		);
		assert!(!crate::session::context::has_active_skill(
			&sid,
			"skilltest-ghost"
		));
	})
	.await;

	crate::session::context::cleanup_session(&sid);
}
