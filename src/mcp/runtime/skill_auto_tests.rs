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
	get_pool()
		.write()
		.unwrap()
		.insert("__default__".to_string(), SkillPool { entries });
}

fn clear_pool() {
	get_pool().write().unwrap().clear();
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
		evolution: None,
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
		evolution: None,
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
		evolution: None,
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
		evolution: None,
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
		evolution: None,
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
		evolution: None,
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
			evolution: None,
		},
		PoolEntry {
			name: "rust-b".to_string(),
			rules: content_rule("rust"),
			evolution: None,
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
		let pool = pool.get("__default__").expect("pool initialized");
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
#[tokio::test]
#[serial]
async fn run_activation_skips_shadow_bound_skills() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("shadowed", "developer", &["content(rust)"]);
	install_tap_skill("failclosed", "developer", &["content(rust)"]);

	let sid = "__skillauto_shadow".to_string();

	// A shadow-flagged binding and a non-shadow binding whose id has no
	// registry record: both classify as shadow (fail-closed) and must be
	// skipped despite a deterministic rule match.
	set_pool(vec![
		PoolEntry {
			name: "shadowed".to_string(),
			rules: content_rule("rust"),
			evolution: Some(crate::supervisor::learning::evolution::SkillBinding {
				id: "__skillauto_shadow_binding".to_string(),
				shadow: true,
				path: PathBuf::new(),
			}),
		},
		PoolEntry {
			name: "failclosed".to_string(),
			rules: content_rule("rust"),
			evolution: Some(crate::supervisor::learning::evolution::SkillBinding {
				id: "__skillauto_unknown_binding".to_string(),
				shadow: false,
				path: PathBuf::new(),
			}),
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

	assert!(
		session.session.messages.is_empty(),
		"shadow-bound skills must not inject content"
	);
	assert!(!has_active_skill(&sid, "shadowed"));
	assert!(!has_active_skill(&sid, "failclosed"));
	cleanup_session(&sid);
	clear_pool();
}

// ---------------------------------------------------------------------------
// Wave-1 coverage additions: pool initialization edge cases (malformed tap
// entries, universal dirs, evolution-generated skills), env-skill injection
// failure, and end-to-end validator scheduling.
// ---------------------------------------------------------------------------

/// Names currently in the pool under `key` ("__default__" outside a session).
fn pool_names(key: &str) -> Vec<String> {
	get_pool()
		.read()
		.expect("pool lock")
		.get(key)
		.map(|p| p.entries.iter().map(|e| e.name.clone()).collect())
		.unwrap_or_default()
}

#[tokio::test]
#[serial]
async fn init_pool_skips_malformed_tap_entries() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();

	// A skill directory without SKILL.md.
	std::fs::create_dir_all(tap.join("skills").join("_notaskill")).expect("dir");
	// A SKILL.md that fails frontmatter parsing.
	write_file(
		&tap.join("skills").join("badmeta").join("SKILL.md"),
		"no frontmatter at all\n",
	);
	// A skill for a different domain.
	write_file(
		&tap.join("skills").join("writer-skill").join("SKILL.md"),
		&skill_md("writer-skill", "writer", &["content(rust)"]),
	);

	clear_pool();
	init_pool("developer");

	let names = pool_names("__default__");
	assert!(
		names.is_empty(),
		"malformed entries must be skipped: {names:?}"
	);

	clear_pool();
}

#[test]
#[serial]
fn init_pool_returns_empty_when_taps_broken() {
	let _guard = DataDirGuard::new();
	let data = crate::directories::get_octomind_data_dir().expect("data dir");
	std::fs::write(data.join("taps.toml"), "not valid toml [[[").expect("broken taps.toml");

	clear_pool();
	init_pool("developer");
	assert!(pool_names("__default__").is_empty());

	clear_pool();
}

#[test]
#[serial]
fn init_pool_reads_universal_skill_dirs() {
	let _guard = DataDirGuard::new();
	default_tap_dir(); // empty tap set

	// Pin HOME to a temp dir so the global universal dir is controlled.
	let home = tempfile::tempdir().expect("home tempdir");
	let prev_home = std::env::var_os("HOME");
	std::env::set_var("HOME", home.path());

	let workdir = tempfile::tempdir().expect("workdir");
	let skills = workdir.path().join(".agents").join("skills");
	// Good universal skill.
	write_file(
		&skills.join("uni-ok").join("SKILL.md"),
		&skill_md("uni-ok", "developer", &["content(rust)"]),
	);
	// A plain file where a skill dir would be.
	std::fs::write(skills.join("_plainfile"), "not a dir").expect("plain file");
	// Dir without SKILL.md.
	std::fs::create_dir_all(skills.join("uni-noskill")).expect("dir");
	// Unparseable SKILL.md.
	write_file(&skills.join("uni-badmeta").join("SKILL.md"), "garbage\n");
	// Wrong domain.
	write_file(
		&skills.join("uni-writer").join("SKILL.md"),
		&skill_md("uni-writer", "writer", &["content(rust)"]),
	);

	// set_session_working_directory CREATES the thread-local entry;
	// set_thread_working_directory only updates an existing one.
	crate::mcp::workdir::set_session_working_directory(workdir.path().to_path_buf());
	clear_pool();
	init_pool("developer");

	let names = pool_names("__default__");
	assert_eq!(
		names,
		vec!["uni-ok".to_string()],
		"only the valid universal skill may enter the pool"
	);

	clear_pool();
	crate::mcp::workdir::set_session_working_directory(
		std::env::current_dir().expect("restore cwd"),
	);
	match prev_home {
		Some(v) => std::env::set_var("HOME", v),
		None => std::env::remove_var("HOME"),
	}
}

#[tokio::test]
#[serial]
async fn init_pool_includes_evolution_generated_skills() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	default_tap_dir(); // empty tap set

	// A trial-state skill record in the evolution registry, with its
	// generated artifact on disk.
	let evo_dir = crate::directories::get_learning_evolution_dir().expect("evolution dir");
	let artifact = evo_dir.join("evo1").join("artifact");
	write_file(
		&artifact.join("SKILL.md"),
		&skill_md("evo-skill", "developer", &["content(rust)"]),
	);
	let registry = serde_json::json!({
		"schema_version": 1,
		"records": [{
			"schema_version": 1,
			"id": "evo1",
			"name": "evo-skill",
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
	write_file(
		&evo_dir.join("registry.json"),
		&serde_json::to_string(&registry).expect("registry json"),
	);

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.supervisor.learning.evolution.enabled = true;

	let sid = "__skillauto_evo".to_string();
	set_session_config(&sid, &config);
	clear_pool();
	with_session_id(sid.clone(), async {
		crate::supervisor::learning::evolution::init_for_session("developer");
		init_pool("developer");
	})
	.await;

	let names = pool_names(&sid);
	assert!(
		names.contains(&"evo-skill".to_string()),
		"generated skill must enter the pool: {names:?}"
	);

	cleanup_session(&sid);
	clear_pool();
}

#[tokio::test]
#[serial]
async fn run_activation_with_empty_pool_entries_returns_early() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("rust-helper", "developer", &["content(rust)"]);

	let sid = "__skillauto_emptyentries".to_string();
	// A pool for the session that exists but holds no entries: activation
	// must return before any matching work.
	get_pool().write().expect("pool lock").insert(
		sid.clone(),
		SkillPool {
			entries: Vec::new(),
		},
	);

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
	clear_pool();
}

#[tokio::test]
#[serial]
async fn load_env_skills_injection_failure_leaves_history_untouched() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let _skills = SkillsEnvGuard::set("env-skill");
	install_tap_skill("env-skill", "developer", &[]);

	let sid = "__skillauto_env_injectfail".to_string();
	let mut session = ChatSession::for_tests(Vec::new());
	// Point the session file at an unwritable path: appending the injected
	// skill content must fail, be logged, and skip injection.
	session.session.session_file =
		Some(PathBuf::from("/nonexistent-octomind-test-dir/session.json"));

	with_session_id(sid.clone(), async {
		load_env_skills(&mut session).await;
	})
	.await;

	assert!(
		session.session.messages.is_empty(),
		"failed injection must not leave partial history"
	);
	cleanup_session(&sid);
}

/// Write an executable validate script and return its path.
fn write_validate_script(skill_dir: &Path, body: &str) {
	write_file(&skill_dir.join("validate"), body);
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(
			skill_dir.join("validate"),
			std::fs::Permissions::from_mode(0o755),
		)
		.expect("chmod validate");
	}
}

#[tokio::test]
#[serial]
async fn run_validators_end_to_end_reports_failures_and_skips_bad_skills() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();

	// Failing validator with stderr output.
	let fail_dir = tap.join("skills").join("val-fail");
	write_file(
		&fail_dir.join("SKILL.md"),
		&skill_md("val-fail", "developer", &[]),
	);
	write_validate_script(
		&fail_dir,
		"#!/bin/sh\necho 'val-fail: broken' >&2\nexit 1\n",
	);
	// Existing but non-executable validator: spawn fails.
	let noexec_dir = tap.join("skills").join("val-noexec");
	write_file(
		&noexec_dir.join("SKILL.md"),
		&skill_md("val-noexec", "developer", &[]),
	);
	write_file(&noexec_dir.join("validate"), "#!/bin/sh\nexit 0\n");
	// Skill dir without any validate script.
	write_file(
		&tap.join("skills").join("val-noval").join("SKILL.md"),
		&skill_md("val-noval", "developer", &[]),
	);
	// A second tap without a skills dir — the search must continue past it.
	let bare_tap = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("taps")
		.join("bare")
		.join("tap");
	std::fs::create_dir_all(&bare_tap).expect("bare tap dir");
	std::fs::write(
		crate::directories::get_octomind_data_dir()
			.expect("data dir")
			.join("taps.toml"),
		format!(
			"[[taps]]\nname = \"bare/tap\"\nlocal_path = {}\n",
			toml::Value::String(bare_tap.to_string_lossy().into_owned())
		),
	)
	.expect("taps.toml");

	let sid = "__skillauto_val_e2e".to_string();
	with_session_id(sid.clone(), async {
		add_active_skill(&sid, "val-fail");
		add_active_skill(&sid, "val-noexec");
		add_active_skill(&sid, "val-noval");

		let failures = run_validators("assistant output", Path::new("/tmp")).await;
		assert_eq!(
			failures,
			vec![("val-fail".to_string(), "val-fail: broken\n".to_string())],
			"only the failing executable validator reports: {failures:?}"
		);
	})
	.await;

	// The failure incremented the retry counter for that skill only.
	{
		let retries = get_retry_tracker().read().expect("retry lock");
		assert_eq!(retries.get("val-fail"), Some(&1), "{retries:?}");
		assert!(!retries.contains_key("val-noexec"));
	}

	cleanup_session(&sid);
	get_retry_tracker().write().expect("retry lock").clear();
}

#[tokio::test]
#[serial]
async fn run_validators_skips_retry_capped_skills_and_broken_taps() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	let capped_dir = tap.join("skills").join("val-capped");
	write_file(
		&capped_dir.join("SKILL.md"),
		&skill_md("val-capped", "developer", &[]),
	);
	write_validate_script(&capped_dir, "#!/bin/sh\necho 'still failing' >&2\nexit 1\n");

	let sid = "__skillauto_val_cap".to_string();
	let max_retries = with_session_id(sid.clone(), async {
		let max = get_skills_config().max_retries;
		add_active_skill(&sid, "val-capped");
		// At the cap: the validator must not even be scheduled.
		get_retry_tracker()
			.write()
			.expect("retry lock")
			.insert("val-capped".to_string(), max);
		let failures = run_validators("assistant output", Path::new("/tmp")).await;
		assert!(
			failures.is_empty(),
			"capped validator must be skipped: {failures:?}"
		);
		max
	})
	.await;
	assert!(max_retries > 0, "default config must cap retries");

	// Broken taps enumeration: no validators can be found, answer stays empty.
	let data = crate::directories::get_octomind_data_dir().expect("data dir");
	std::fs::write(data.join("taps.toml"), "not valid toml [[[").expect("broken taps.toml");
	with_session_id(sid.clone(), async {
		let failures = run_validators("assistant output", Path::new("/tmp")).await;
		assert!(
			failures.is_empty(),
			"broken taps must degrade to no validators"
		);
	})
	.await;

	cleanup_session(&sid);
	get_retry_tracker().write().expect("retry lock").clear();
}

#[tokio::test]
#[serial]
async fn run_validators_timeout_zero_passes_and_resets_retries() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	let pass_dir = tap.join("skills").join("val-pass");
	write_file(
		&pass_dir.join("SKILL.md"),
		&skill_md("val-pass", "developer", &[]),
	);
	write_validate_script(&pass_dir, "#!/bin/sh\nexit 0\n");

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	// The template ships auto_validation = false; this test needs validators
	// ON, with the 0 (effectively unlimited) timeout branch.
	config.skills.auto_validation = true;
	config.skills.validation_timeout = 0;

	let sid = "__skillauto_val_timeout0".to_string();
	set_session_config(&sid, &config);
	get_retry_tracker()
		.write()
		.expect("retry lock")
		.insert("val-pass".to_string(), 1);

	with_session_id(sid.clone(), async {
		add_active_skill(&sid, "val-pass");
		let failures = run_validators("assistant output", Path::new("/tmp")).await;
		assert!(
			failures.is_empty(),
			"passing validator reports nothing: {failures:?}"
		);
	})
	.await;

	// A passing validation resets the retry counter.
	assert!(
		!get_retry_tracker()
			.read()
			.expect("retry lock")
			.contains_key("val-pass"),
		"pass must reset retries"
	);

	cleanup_session(&sid);
	get_retry_tracker().write().expect("retry lock").clear();
}
