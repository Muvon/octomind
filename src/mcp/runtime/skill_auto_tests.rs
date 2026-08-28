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

//! Pipeline tests for the skill auto-activation engine: OCTOMIND_SKILLS
//! env loading, `run_activation` gating + deterministic matching, and pool
//! initialization from tap skills. The inline `mod tests` covers the pure
//! helpers (intent gate, XML stripping, validate-script contract) — these
//! tests exercise the session-integrated paths.

use super::*;
use crate::mcp::runtime::skill::ActivateCheck;
use crate::session::chat::session::ChatSession;
use crate::session::context::{
	add_active_skill, cleanup_session, has_active_skill, set_session_config, with_session_id,
};
use crate::session::Message;
use serial_test::serial;
use std::path::{Path, PathBuf};

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir. Tests using it must be
/// `#[serial]` (env is process-global).
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
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

/// Save/restore `OCTOMIND_SKILLS` around a test.
struct SkillsEnvGuard {
	previous: Option<std::ffi::OsString>,
}

impl SkillsEnvGuard {
	fn set(value: &str) -> Self {
		let previous = std::env::var_os("OCTOMIND_SKILLS");
		std::env::set_var("OCTOMIND_SKILLS", value);
		Self { previous }
	}

	fn remove() -> Self {
		let previous = std::env::var_os("OCTOMIND_SKILLS");
		std::env::remove_var("OCTOMIND_SKILLS");
		Self { previous }
	}
}

impl Drop for SkillsEnvGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_SKILLS", v),
			None => std::env::remove_var("OCTOMIND_SKILLS"),
		}
	}
}

/// The default tap's on-disk directory inside the current data dir.
/// `get_taps()` never clones — creating the dir is enough, no network.
fn default_tap_dir() -> PathBuf {
	let dir = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("taps")
		.join("muvon")
		.join("octomind-tap");
	std::fs::create_dir_all(&dir).expect("create default tap dir");
	dir
}

fn write_file(path: &Path, content: &str) {
	std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
	std::fs::write(path, content).expect("write file");
}

/// SKILL.md fixture: frontmatter with name, description, domains and
/// optional AND-group rules (`rules:` list of `- check(args)` lines).
fn skill_md(name: &str, domains: &str, rules: &[&str]) -> String {
	let mut rules_block = String::new();
	if !rules.is_empty() {
		rules_block.push_str("rules:\n");
		for r in rules {
			rules_block.push_str(&format!("  - {r}\n"));
		}
	}
	format!(
		"---\nname: {name}\ndescription: Test skill {name}\ndomains: {domains}\n{rules_block}---\n\n# {name} body\n"
	)
}

/// Install a skill into the default tap and return its name.
fn install_tap_skill(name: &str, domains: &str, rules: &[&str]) -> String {
	let tap = default_tap_dir();
	write_file(
		&tap.join("skills").join(name).join("SKILL.md"),
		&skill_md(name, domains, rules),
	);
	name.to_string()
}

fn set_pool(entries: Vec<PoolEntry>) {
	*get_pool().write().unwrap() = Some(SkillPool { entries });
}

fn clear_pool() {
	*get_pool().write().unwrap() = None;
}

fn content_rule(pattern: &str) -> Vec<Vec<ActivateCheck>> {
	vec![vec![ActivateCheck::Content(pattern.to_string())]]
}

// ---------------------------------------------------------------------------
// OCTOMIND_SKILLS env loading
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn load_env_skills_noop_when_env_unset_or_blank() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let _skills = SkillsEnvGuard::remove();
	let mut session = ChatSession::for_tests(Vec::new());

	load_env_skills(&mut session).await;
	assert!(session.session.messages.is_empty());

	std::env::set_var("OCTOMIND_SKILLS", "  ,  ");
	load_env_skills(&mut session).await;
	assert!(
		session.session.messages.is_empty(),
		"blank entries are filtered"
	);
}

#[tokio::test]
#[serial]
async fn load_env_skills_missing_skill_is_not_injected_or_activated() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let _skills = SkillsEnvGuard::set("ghost-skill");
	default_tap_dir(); // empty tap set — ghost-skill is nowhere on disk

	let sid = "__skillauto_env_missing".to_string();
	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		load_env_skills(&mut session).await;
	})
	.await;

	assert!(session.session.messages.is_empty());
	assert!(!has_active_skill(&sid, "ghost-skill"));
	cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn load_env_skills_injects_tap_skill_and_marks_active() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let _skills = SkillsEnvGuard::set("env-skill");
	install_tap_skill("env-skill", "developer", &[]);

	let sid = "__skillauto_env_inject".to_string();
	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		load_env_skills(&mut session).await;
	})
	.await;

	assert_eq!(session.session.messages.len(), 1);
	let msg = &session.session.messages[0];
	assert_eq!(msg.role, "user");
	assert!(msg.content.contains("<skill name=\"env-skill\""));
	assert!(msg.content.contains("# env-skill body"));
	assert!(msg.content.contains("</skill>"));
	assert!(crate::session::is_system_managed_user_content(&msg.content));
	assert!(has_active_skill(&sid, "env-skill"));
	cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn load_env_skills_skips_already_active_skill() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let _skills = SkillsEnvGuard::set("env-skill");
	install_tap_skill("env-skill", "developer", &[]);

	let sid = "__skillauto_env_skip".to_string();
	add_active_skill(&sid, "env-skill");
	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		load_env_skills(&mut session).await;
	})
	.await;

	assert!(session.session.messages.is_empty(), "no re-injection");
	assert!(has_active_skill(&sid, "env-skill"));
	cleanup_session(&sid);
}

#[tokio::test]
#[serial]
async fn load_env_skills_resume_path_marks_existing_message_active() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let _skills = SkillsEnvGuard::set("env-skill");
	install_tap_skill("env-skill", "developer", &[]);

	let sid = "__skillauto_env_resume".to_string();
	let restored = Message {
		role: "user".to_string(),
		content: "<skill name=\"env-skill\">\nold body\n</skill>".to_string(),
		..Default::default()
	};
	let mut session = ChatSession::for_tests(vec![restored]);
	with_session_id(sid.clone(), async {
		load_env_skills(&mut session).await;
	})
	.await;

	assert_eq!(session.session.messages.len(), 1, "history untouched");
	assert!(has_active_skill(&sid, "env-skill"));
	cleanup_session(&sid);
}

// ---------------------------------------------------------------------------
// run_activation gating
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn run_activation_disabled_in_config_activates_nothing() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("never", "developer", &["content(rust)"]);

	let sid = "__skillauto_disabled".to_string();
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.skills.auto_activation = false;
	set_session_config(&sid, &config);

	set_pool(vec![PoolEntry {
		name: "never".to_string(),
		rules: content_rule("rust"),
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"please help with rust development",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert!(
		session.session.messages.is_empty(),
		"config gate must fire before pool rules"
	);
	assert!(!has_active_skill(&sid, "never"));
	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_skips_system_managed_content() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("never", "developer", &["content(rust)"]);

	let sid = "__skillauto_sysmgd".to_string();
	set_pool(vec![PoolEntry {
		name: "never".to_string(),
		rules: content_rule("rust"),
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"<skill name=\"x\">\nuse rust now please\n</skill>",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert!(session.session.messages.is_empty());
	assert!(!has_active_skill(&sid, "never"));
	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_rejects_low_intent_input() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	// Rule matches the short input verbatim — only the intent gate stops it.
	install_tap_skill("never", "developer", &["content(try)"]);

	let sid = "__skillauto_lowintent".to_string();
	set_pool(vec![PoolEntry {
		name: "never".to_string(),
		rules: content_rule("try"),
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("try", Path::new("/tmp"), &mut session).await;
	})
	.await;

	assert!(session.session.messages.is_empty());
	assert!(!has_active_skill(&sid, "never"));
	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_without_pool_is_noop() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("rust-helper", "developer", &["content(rust)"]);

	let sid = "__skillauto_nopool".to_string();
	clear_pool();

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"please help with rust development",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert!(session.session.messages.is_empty());
	assert!(!has_active_skill(&sid, "rust-helper"));
	cleanup_session(&sid);
}

// ---------------------------------------------------------------------------
// run_activation matching
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn run_activation_deterministic_match_injects_skill() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("rust-helper", "developer", &["content(rust)"]);

	let sid = "__skillauto_match".to_string();
	set_pool(vec![PoolEntry {
		name: "rust-helper".to_string(),
		rules: content_rule("rust"),
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"please help with rust development",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert_eq!(session.session.messages.len(), 1);
	let msg = &session.session.messages[0];
	assert!(msg.content.contains("<skill name=\"rust-helper\""));
	assert!(crate::session::is_system_managed_user_content(&msg.content));
	assert!(has_active_skill(&sid, "rust-helper"));
	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_no_matching_rule_is_silent() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("py-helper", "developer", &["content(python)"]);

	let sid = "__skillauto_nomatch".to_string();
	set_pool(vec![PoolEntry {
		name: "py-helper".to_string(),
		rules: content_rule("python"),
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"please help with rust development",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert!(session.session.messages.is_empty());
	assert!(!has_active_skill(&sid, "py-helper"));
	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_skips_already_active_skills() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("rust-helper", "developer", &["content(rust)"]);

	let sid = "__skillauto_active".to_string();
	add_active_skill(&sid, "rust-helper");
	set_pool(vec![PoolEntry {
		name: "rust-helper".to_string(),
		rules: content_rule("rust"),
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"please help with rust development",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert!(
		session.session.messages.is_empty(),
		"active skills are not re-injected"
	);
	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_multiple_deterministic_matches_all_activate() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("rust-a", "developer", &["content(rust)"]);
	install_tap_skill("rust-b", "developer", &["content(rust)"]);

	let sid = "__skillauto_multi".to_string();
	set_pool(vec![
		PoolEntry {
			name: "rust-a".to_string(),
			rules: content_rule("rust"),
		},
		PoolEntry {
			name: "rust-b".to_string(),
			rules: content_rule("rust"),
		},
	]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation(
			"please help with rust development",
			Path::new("/tmp"),
			&mut session,
		)
		.await;
	})
	.await;

	assert_eq!(session.session.messages.len(), 2, "both matches activate");
	assert!(has_active_skill(&sid, "rust-a"));
	assert!(has_active_skill(&sid, "rust-b"));
	cleanup_session(&sid);
	clear_pool();
}

// ---------------------------------------------------------------------------
// Pool init + config override
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn init_pool_collects_domain_matching_rule_bearing_skills() {
	let _guard = DataDirGuard::new();
	install_tap_skill("in-domain", "developer", &["content(rust)"]);
	// No rules → excluded from the auto-activation pool.
	install_tap_skill("no-rules", "developer", &[]);
	// Different domain → excluded.
	install_tap_skill("other-domain", "medical", &["content(rust)"]);

	init_pool("developer");

	{
		let pool = get_pool().read().unwrap();
		let pool = pool.as_ref().expect("pool initialized");
		let names: Vec<&str> = pool.entries.iter().map(|e| e.name.as_str()).collect();
		assert!(names.contains(&"in-domain"));
		assert!(
			!names.contains(&"no-rules"),
			"rule-less skills stay out of the pool"
		);
		assert!(!names.contains(&"other-domain"), "domain filter applies");
	}
	clear_pool();
}

#[tokio::test]
#[serial]
async fn skills_config_reads_session_override() {
	let sid = "__skillauto_cfg".to_string();
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.skills.validation_timeout = 123;
	config.skills.max_retries = 7;
	set_session_config(&sid, &config);

	let cfg = with_session_id(sid.clone(), async { get_skills_config() }).await;
	assert_eq!(cfg.validation_timeout, 123);
	assert_eq!(cfg.max_retries, 7);

	cleanup_session(&sid);
}
