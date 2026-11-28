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
		Self { previous, _dir: dir }
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
	assert!(session.session.messages.is_empty(), "blank entries are filtered");
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
		toml::from_str(include_str!("../../../config-templates/default.toml")).expect("parse default config template");
	config.skills.auto_activation = false;
	set_session_config(&sid, &config);

	set_pool(vec![PoolEntry {
		name: "never".to_string(),
		description: String::new(),
		rules: content_rule("rust"),
		evolution: None,
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
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
		description: String::new(),
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
		description: String::new(),
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
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
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
		description: String::new(),
		rules: content_rule("rust"),
		evolution: None,
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
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
		description: String::new(),
		rules: content_rule("python"),
		evolution: None,
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
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
		description: String::new(),
		rules: content_rule("rust"),
		evolution: None,
	}]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
	})
	.await;

	assert!(session.session.messages.is_empty(), "active skills are not re-injected");
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
			description: String::new(),
			rules: content_rule("rust"),
			evolution: None,
		},
		PoolEntry {
			name: "rust-b".to_string(),
			description: String::new(),
			rules: content_rule("rust"),
			evolution: None,
		},
	]);

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
	})
	.await;

	assert_eq!(session.session.messages.len(), 2, "both matches activate");
	assert!(has_active_skill(&sid, "rust-a"));
	assert!(has_active_skill(&sid, "rust-b"));
	cleanup_session(&sid);
	clear_pool();
}

// ---------------------------------------------------------------------------
// Judgement by a plugin when the rules abstain
// ---------------------------------------------------------------------------

fn skill_choice(choice: &str, probability: f64) -> serde_json::Value {
	serde_json::json!({"judgements": {"skill": {
		"type": "choice",
		"choice": choice,
		"probabilities": {choice: probability},
	}}})
}

/// A pool entry with no rules: only a judgement can activate it.
fn rule_less_entry(name: &str) -> PoolEntry {
	PoolEntry {
		name: name.to_string(),
		description: format!("Test skill {name}"),
		rules: Vec::new(),
		evolution: None,
	}
}

/// Run an activation for `content` in session `sid` with a plugin that answers
/// every `judge` event with `answer`.
async fn activate_with_judging_plugin(
	sid: &str,
	answer: serde_json::Value,
	entries: Vec<PoolEntry>,
	content: &str,
) -> crate::plugins::test_support::StubPlugin {
	let stub = crate::plugins::test_support::StubPlugin::new(&["judge"]).answer(answer);
	crate::plugins::test_support::connect_stub(sid, "developer", Path::new("/tmp"), &stub).await;
	set_pool(entries);
	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.to_string(), async {
		run_activation(content, Path::new("/tmp"), &mut session).await;
	})
	.await;
	stub
}

fn finish_judging_test(sid: &str) {
	crate::plugins::end_session(sid);
	cleanup_session(&sid.to_string());
	clear_pool();
}

#[tokio::test]
#[serial]
async fn a_skill_the_plugin_picks_with_enough_confidence_is_activated() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("fits", "developer", &[]);
	install_tap_skill("other", "developer", &[]);

	let sid = "__skillauto_judge_pick".to_string();
	let entries = vec![rule_less_entry("fits"), rule_less_entry("other")];
	let stub = activate_with_judging_plugin(
		&sid,
		skill_choice("fits", 0.9),
		entries,
		"please write the release notes",
	)
	.await;

	assert!(has_active_skill(&sid, "fits"));
	assert!(!has_active_skill(&sid, "other"));
	let asked = stub.sent("judge");
	assert_eq!(asked.len(), 1);
	assert_eq!(asked[0]["purpose"], "skills");
	assert_eq!(asked[0]["subject"], "please write the release notes");
	finish_judging_test(&sid);
}

#[tokio::test]
#[serial]
async fn a_skill_the_plugin_picks_below_the_floor_is_not_activated() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("fits", "developer", &[]);

	let sid = "__skillauto_judge_low".to_string();
	let answer = skill_choice("fits", crate::plugins::judgement::SKILL_ACTIVATE_AT - 0.01);
	activate_with_judging_plugin(
		&sid,
		answer,
		vec![rule_less_entry("fits")],
		"please write the release notes",
	)
	.await;

	assert!(!has_active_skill(&sid, "fits"));
	finish_judging_test(&sid);
}

#[tokio::test]
#[serial]
async fn a_plugin_that_picks_none_activates_no_skill() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("fits", "developer", &[]);

	let sid = "__skillauto_judge_none".to_string();
	let answer = skill_choice(crate::plugins::judgement::NONE, 0.99);
	activate_with_judging_plugin(
		&sid,
		answer,
		vec![rule_less_entry("fits")],
		"please write the release notes",
	)
	.await;

	assert!(!has_active_skill(&sid, "fits"));
	finish_judging_test(&sid);
}

#[tokio::test]
#[serial]
async fn a_rule_that_decided_keeps_the_plugin_from_being_asked() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("rust-helper", "developer", &["content(rust)"]);
	install_tap_skill("fits", "developer", &[]);

	let sid = "__skillauto_judge_rules".to_string();
	let mut ruled = rule_less_entry("rust-helper");
	ruled.rules = content_rule("rust");
	let entries = vec![ruled, rule_less_entry("fits")];
	let stub = activate_with_judging_plugin(
		&sid,
		skill_choice("fits", 0.9),
		entries,
		"please help with rust development",
	)
	.await;

	assert!(has_active_skill(&sid, "rust-helper"));
	assert!(!has_active_skill(&sid, "fits"));
	assert!(stub.sent("judge").is_empty());
	finish_judging_test(&sid);
}

#[tokio::test]
#[serial]
async fn without_a_plugin_that_judges_the_rules_abstaining_activates_nothing() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	install_tap_skill("fits", "developer", &[]);

	let sid = "__skillauto_judge_absent".to_string();
	set_pool(vec![rule_less_entry("fits")]);
	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please write the release notes", Path::new("/tmp"), &mut session).await;
	})
	.await;

	assert!(session.session.messages.is_empty());
	assert!(!has_active_skill(&sid, "fits"));
	finish_judging_test(&sid);
}

// ---------------------------------------------------------------------------
// Pool init + config override
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn init_pool_collects_domain_matching_skills() {
	let _guard = DataDirGuard::new();
	install_tap_skill("in-domain", "developer", &["content(rust)"]);
	// No rules → still in the pool, reachable only by the evaluation choice.
	install_tap_skill("no-rules", "developer", &[]);
	// Different domain → excluded.
	install_tap_skill("other-domain", "medical", &["content(rust)"]);

	init_pool("developer");

	{
		let pool = get_pool().read().unwrap();
		let pool = pool.get("__default__").expect("pool initialized");
		let names: Vec<&str> = pool.entries.iter().map(|e| e.name.as_str()).collect();
		assert!(names.contains(&"in-domain"));
		let no_rules = pool
			.entries
			.iter()
			.find(|e| e.name == "no-rules")
			.expect("rule-less skills join the pool");
		assert!(no_rules.rules.is_empty());
		assert!(!names.contains(&"other-domain"), "domain filter applies");
	}
	clear_pool();
}

#[tokio::test]
#[serial]
async fn skills_config_reads_session_override() {
	let sid = "__skillauto_cfg".to_string();
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml")).expect("parse default config template");
	config.skills.validation_timeout = 123;
	config.skills.max_retries = 7;
	set_session_config(&sid, &config);

	let cfg = with_session_id(sid.clone(), async { get_skills_config() }).await;
	assert_eq!(cfg.validation_timeout, 123);
	assert_eq!(cfg.max_retries, 7);

	cleanup_session(&sid);
}

// ---------------------------------------------------------------------------
// Wave-1 coverage additions: pool initialization edge cases (malformed tap
// entries, universal dirs), env-skill injection
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
	assert!(names.is_empty(), "malformed entries must be skipped: {names:?}");

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
	crate::mcp::workdir::set_session_working_directory(std::env::current_dir().expect("restore cwd"));
	match prev_home {
		Some(v) => std::env::set_var("HOME", v),
		None => std::env::remove_var("HOME"),
	}
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
	get_pool()
		.write()
		.expect("pool lock")
		.insert(sid.clone(), SkillPool { entries: Vec::new() });

	let mut session = ChatSession::for_tests(Vec::new());
	with_session_id(sid.clone(), async {
		run_activation("please help with rust development", Path::new("/tmp"), &mut session).await;
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
	session.session.session_file = Some(PathBuf::from("/nonexistent-octomind-test-dir/session.json"));

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

/// Install a skill owned by `owner` (`agents/<owner>/skills/<name>/`). Its frontmatter names
/// the domain `ignored-domain`, which is never read: the location binds the skill.
fn install_owned_skill(owner: &str, name: &str, rules: &[&str]) -> PathBuf {
	let skill_dir = default_tap_dir().join("agents").join(owner).join("skills").join(name);
	write_file(&skill_dir.join("SKILL.md"), &skill_md(name, "ignored-domain", rules));
	write_file(
		&default_tap_dir().join("agents").join(owner).join("default.toml"),
		"# Title: T\n# Description: D\n",
	);
	skill_dir
}

#[test]
#[serial]
fn an_owned_skill_enters_the_pool_only_for_its_owner() {
	let _guard = DataDirGuard::new();
	install_owned_skill("writer", "owned-skill", &["content(draft)"]);

	clear_pool();
	init_pool("writer");
	assert_eq!(pool_names("__default__"), vec!["owned-skill".to_string()]);

	clear_pool();
	init_pool("developer");
	assert!(pool_names("__default__").is_empty(), "another agent does not get it");

	clear_pool();
	init_pool("ignored-domain");
	assert!(
		pool_names("__default__").is_empty(),
		"the frontmatter `domains:` of an owned skill is not read"
	);
	clear_pool();
}

#[test]
#[serial]
fn an_owned_skill_binds_to_the_namespace_so_a_variant_session_gets_it() {
	let _guard = DataDirGuard::new();
	install_owned_skill("writer", "owned-skill", &["content(draft)"]);
	install_tap_skill("shared-ns", "writer", &["content(draft)"]);

	clear_pool();
	init_pool("writer:fiction");
	assert_eq!(
		pool_names("__default__"),
		vec!["shared-ns".to_string(), "owned-skill".to_string()],
		"a session of `writer:fiction` gets the skills of the namespace `writer`"
	);

	clear_pool();
	init_pool("writer-extra:fiction");
	assert!(pool_names("__default__").is_empty(), "another namespace does not");
	clear_pool();
}

#[test]
#[serial]
fn a_shared_skill_is_still_gated_by_its_domains() {
	let _guard = DataDirGuard::new();
	install_tap_skill("shared-two", "developer writer", &["content(rust)"]);
	install_tap_skill("shared-manual", "", &["content(rust)"]);

	clear_pool();
	init_pool("writer");
	assert_eq!(pool_names("__default__"), vec!["shared-two".to_string()]);

	clear_pool();
	init_pool("lawyer");
	assert!(pool_names("__default__").is_empty());
	clear_pool();
}

#[test]
#[serial]
fn shared_and_owned_skills_enter_the_pool_in_a_fixed_order() {
	let _guard = DataDirGuard::new();
	install_owned_skill("writer", "b-owned", &["content(x)"]);
	install_owned_skill("writer", "a-owned", &["content(x)"]);
	install_tap_skill("z-shared", "writer", &["content(x)"]);

	clear_pool();
	init_pool("writer");
	assert_eq!(
		pool_names("__default__"),
		vec!["z-shared".to_string(), "a-owned".to_string(), "b-owned".to_string()],
		"shared first, then the agent's skills by name"
	);
	clear_pool();
}

/// Write an executable validate script and return its path.
fn write_validate_script(skill_dir: &Path, body: &str) {
	write_file(&skill_dir.join("validate"), body);
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(skill_dir.join("validate"), std::fs::Permissions::from_mode(0o755))
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
	write_file(&fail_dir.join("SKILL.md"), &skill_md("val-fail", "developer", &[]));
	write_validate_script(&fail_dir, "#!/bin/sh\necho 'val-fail: broken' >&2\nexit 1\n");
	// Existing but non-executable validator: spawn fails.
	let noexec_dir = tap.join("skills").join("val-noexec");
	write_file(&noexec_dir.join("SKILL.md"), &skill_md("val-noexec", "developer", &[]));
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

		let failures = run_validators("assistant output", &std::env::temp_dir()).await;
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
async fn the_validator_of_a_shared_name_is_the_one_the_agent_loaded() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	// A shared `dup-lint` for developers and one owned by `doctor`, with different validators.
	let shared = tap.join("skills").join("dup-lint");
	write_file(&shared.join("SKILL.md"), &skill_md("dup-lint", "developer", &[]));
	write_validate_script(&shared, "#!/bin/sh\necho shared >&2\nexit 1\n");
	let owned = install_owned_skill("doctor", "dup-lint", &[]);
	write_validate_script(&owned, "#!/bin/sh\necho owned >&2\nexit 1\n");

	for (role, expected) in [("doctor:blood", "owned\n"), ("developer", "shared\n")] {
		let sid = format!("__skillauto_dup_{}", role.replace(':', "_"));
		crate::session::context::set_session_role(&sid, role);
		with_session_id(sid.clone(), async {
			add_active_skill(&sid, "dup-lint");
			let failures = run_validators("assistant output", &std::env::temp_dir()).await;
			assert_eq!(failures, vec![("dup-lint".to_string(), expected.to_string())], "{role}");
		})
		.await;
		cleanup_session(&sid);
		get_retry_tracker().write().expect("retry lock").clear();
	}
}

#[tokio::test]
#[serial]
async fn run_validators_skips_retry_capped_skills_and_broken_taps() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	let capped_dir = tap.join("skills").join("val-capped");
	write_file(&capped_dir.join("SKILL.md"), &skill_md("val-capped", "developer", &[]));
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
		let failures = run_validators("assistant output", &std::env::temp_dir()).await;
		assert!(failures.is_empty(), "capped validator must be skipped: {failures:?}");
		max
	})
	.await;
	assert!(max_retries > 0, "default config must cap retries");

	// Broken taps enumeration: no validators can be found, answer stays empty.
	let data = crate::directories::get_octomind_data_dir().expect("data dir");
	std::fs::write(data.join("taps.toml"), "not valid toml [[[").expect("broken taps.toml");
	with_session_id(sid.clone(), async {
		let failures = run_validators("assistant output", &std::env::temp_dir()).await;
		assert!(failures.is_empty(), "broken taps must degrade to no validators");
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
	write_file(&pass_dir.join("SKILL.md"), &skill_md("val-pass", "developer", &[]));
	write_validate_script(&pass_dir, "#!/bin/sh\nexit 0\n");

	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml")).expect("parse default config template");
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
		let failures = run_validators("assistant output", &std::env::temp_dir()).await;
		assert!(failures.is_empty(), "passing validator reports nothing: {failures:?}");
	})
	.await;

	// A passing validation resets the retry counter.
	assert!(
		!get_retry_tracker().read().expect("retry lock").contains_key("val-pass"),
		"pass must reset retries"
	);

	cleanup_session(&sid);
	get_retry_tracker().write().expect("retry lock").clear();
}

// ---------------------------------------------------------------------------
// Skill directories registered by plugins
// ---------------------------------------------------------------------------

use crate::plugins::protocol::event as plugin_event;
use crate::plugins::test_support::StubPlugin;
use crate::session::guardrails::plugin_tests::{stub_answering, PluginSession};

fn skill_dirs_answer(dirs: &[(&str, &PathBuf, &str)]) -> serde_json::Value {
	let dirs: Vec<_> = dirs
		.iter()
		.map(|(id, path, mode)| serde_json::json!({"id": id, "path": path.to_str().unwrap(), "mode": mode}))
		.collect();
	serde_json::json!({"register": {"skill_dirs": dirs}})
}

fn write_plugin_skill(root: &Path, name: &str) -> PathBuf {
	let dir = root.join(name);
	write_file(&dir.join("SKILL.md"), &skill_md(name, "developer", &["content(rust)"]));
	dir
}

/// Binding of the pool entry named `name` in the session's pool.
fn pool_binding(sid: &str, name: &str) -> SkillBinding {
	get_pool()
		.read()
		.unwrap()
		.get(sid)
		.and_then(|pool| pool.entries.iter().find(|entry| entry.name == name))
		.and_then(|entry| entry.evolution.clone())
		.unwrap_or_else(|| panic!("no plugin skill '{name}' in the pool"))
}

#[tokio::test]
#[serial]
async fn plugin_skill_dirs_join_the_pool_and_the_active_list_by_mode() {
	let root = tempfile::tempdir().unwrap();
	let live = write_plugin_skill(root.path(), "plugin-live");
	let watched = write_plugin_skill(root.path(), "plugin-watched");
	let answer = skill_dirs_answer(&[("live", &live, "apply"), ("watched", &watched, "observe")]);
	let session = PluginSession::start("skills-pool", stub_answering(answer)).await;
	get_pool()
		.write()
		.unwrap()
		.insert(session.sid.clone(), SkillPool { entries: Vec::new() });

	session.send_event().await;

	assert_eq!(pool_names(&session.sid), vec!["plugin-live", "plugin-watched"]);
	assert!(!pool_binding(&session.sid, "plugin-live").shadow);
	assert!(pool_binding(&session.sid, "plugin-watched").shadow);
	let active = with_session_id(session.sid.clone(), async { plugin_skill_dirs() }).await;
	assert_eq!(active, vec![live]);
}

#[tokio::test]
#[serial]
async fn a_second_registration_of_skill_dirs_replaces_the_first() {
	let root = tempfile::tempdir().unwrap();
	let one = write_plugin_skill(root.path(), "plugin-one");
	let two = write_plugin_skill(root.path(), "plugin-two");
	let stub = StubPlugin::new(&[plugin_event::REQUEST_BEFORE]).queue([
		skill_dirs_answer(&[("one", &one, "apply")]),
		skill_dirs_answer(&[("two", &two, "observe")]),
	]);
	let session = PluginSession::start("skills-replace", stub).await;
	get_pool()
		.write()
		.unwrap()
		.insert(session.sid.clone(), SkillPool { entries: Vec::new() });

	session.send_event().await;
	assert_eq!(pool_names(&session.sid), vec!["plugin-one"]);
	session.send_event().await;
	assert_eq!(pool_names(&session.sid), vec!["plugin-two"]);
	// An event that registers nothing changes nothing.
	session.send_event().await;
	assert_eq!(pool_names(&session.sid), vec!["plugin-two"]);
	let active = with_session_id(session.sid.clone(), async { plugin_skill_dirs() }).await;
	assert!(active.is_empty());
}

#[tokio::test]
#[serial]
async fn a_pool_built_after_the_registration_includes_the_skill_dirs() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	default_tap_dir();
	let root = tempfile::tempdir().unwrap();
	let live = write_plugin_skill(root.path(), "plugin-late");
	let answer = skill_dirs_answer(&[("late", &live, "apply")]);
	let session = PluginSession::start("skills-late", stub_answering(answer)).await;

	session.send_event().await;
	assert!(pool_names(&session.sid).is_empty());
	with_session_id(session.sid.clone(), async { init_pool("developer") }).await;

	assert_eq!(pool_names(&session.sid), vec!["plugin-late"]);
}

#[tokio::test]
#[serial]
async fn a_plugin_skill_match_is_reported_by_mode_and_observed_ones_stay_out() {
	let root = tempfile::tempdir().unwrap();
	let live = write_plugin_skill(root.path(), "plugin-live");
	let watched = write_plugin_skill(root.path(), "plugin-watched");
	let answer = skill_dirs_answer(&[("live", &live, "apply"), ("watched", &watched, "observe")]);
	let session = PluginSession::start("skills-match", stub_answering(answer)).await;
	get_pool()
		.write()
		.unwrap()
		.insert(session.sid.clone(), SkillPool { entries: Vec::new() });
	session.send_event().await;

	let live_binding = pool_binding(&session.sid, "plugin-live");
	let watched_binding = pool_binding(&session.sid, "plugin-watched");
	let (live_observed, watched_observed) = with_session_id(session.sid.clone(), async {
		(observed_only(&live_binding), observed_only(&watched_binding))
	})
	.await;

	assert!(!live_observed);
	assert!(watched_observed);
	assert_eq!(
		session.reported_matches().await,
		serde_json::json!([
			{"id": "live", "outcome": "applied"},
			{"id": "watched", "outcome": "observed"}
		])
	);
}

#[tokio::test]
#[serial]
async fn run_validators_runs_the_validate_script_of_an_owned_skill() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let skill_dir = install_owned_skill("writer", "val-owned", &[]);
	write_validate_script(&skill_dir, "#!/bin/sh\necho 'val-owned: broken' >&2\nexit 1\n");

	let sid = "__skillauto_val_owned".to_string();
	with_session_id(sid.clone(), async {
		add_active_skill(&sid, "val-owned");
		let failures = run_validators("assistant output", &std::env::temp_dir()).await;
		assert_eq!(
			failures,
			vec![("val-owned".to_string(), "val-owned: broken\n".to_string())]
		);
	})
	.await;

	cleanup_session(&sid);
	get_retry_tracker().write().expect("retry lock").clear();
}
