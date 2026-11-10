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
fn test_prepare_role_manifest_overrides_first_role() {
	let manifest = "[[roles]]\nname = \"old\"\nsystem = \"do things\"\n";
	let injected = prepare_role_manifest(manifest, "doctor:blood").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert_eq!(value["roles"][0]["name"].as_str(), Some("doctor:blood"));
	// Other fields survive the roundtrip
	assert_eq!(value["roles"][0]["system"].as_str(), Some("do things"));
}

#[test]
fn test_prepare_role_manifest_without_roles_is_noop() {
	let injected = prepare_role_manifest("version = 1\n", "tag").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert!(value.get("roles").is_none());
	assert_eq!(value["version"].as_integer(), Some(1));
}

#[test]
fn test_prepare_role_manifest_invalid_toml_errors() {
	assert!(prepare_role_manifest("not = = toml", "tag").is_err());
}

// --- full tag resolution through a fabricated tap ---------------------------

use serial_test::serial;

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test.
/// Tests using it must be `#[serial]` (env is process-global).
struct ResolverDataDir {
	previous: Option<std::ffi::OsString>,
	dir: tempfile::TempDir,
}

impl ResolverDataDir {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("create temp data dir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self { previous, dir }
	}

	fn path(&self) -> &std::path::Path {
		self.dir.path()
	}
}

impl Drop for ResolverDataDir {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(old) => std::env::set_var("OCTOMIND_DATA_DIR", old),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Create the default tap tree with one agent manifest and a passing dep
/// script, so `fetch_manifest`/`run_dep_entries` work fully offline.
fn install_default_tap_agent(data_dir: &std::path::Path, manifest: &str) {
	let agents_dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join("ztest");
	std::fs::create_dir_all(&agents_dir).expect("create agent dir");
	std::fs::write(agents_dir.join("zz.toml"), manifest).expect("write manifest");

	let deps_dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("deps")
		.join("ztest");
	std::fs::create_dir_all(&deps_dir).expect("create deps dir");
	std::fs::write(deps_dir.join("probe.sh"), "exit 0\n").expect("write dep script");
}

fn base_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
#[serial]
async fn resolve_config_and_role_merges_tap_manifest_end_to_end() {
	let guard = ResolverDataDir::new();
	install_default_tap_agent(
		guard.path(),
		"[[roles]]\nname = \"ztest:zz\"\nsystem = \"You are the {{ENV:OCTOMIND_RESOLVER_PROBE}} agent.\"\nwelcome = \"Hi.\"\ntemperature = 0.1\ntop_p = 0.9\ntop_k = 40\n\n[deps]\nrequire = [\"ztest/probe\"]\n",
	);

	let had_probe = std::env::var_os("OCTOMIND_RESOLVER_PROBE");
	std::env::set_var("OCTOMIND_RESOLVER_PROBE", "resolver");

	let statuses: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
	let (merged, role) = resolve_config_and_role(
		Some("ztest:zz"),
		&base_config(),
		Some(&|status| statuses.lock().expect("statuses lock").push(status.to_string())),
	)
	.await
	.expect("tag resolves end to end");

	match had_probe {
		Some(value) => std::env::set_var("OCTOMIND_RESOLVER_PROBE", value),
		None => std::env::remove_var("OCTOMIND_RESOLVER_PROBE"),
	}

	assert_eq!(role, "ztest:zz", "the tag itself is the role identity");
	let resolved_role = merged
		.roles
		.iter()
		.find(|r| r.name == "ztest:zz")
		.expect("merged config carries the tap role");
	assert_eq!(
		resolved_role.config.system, "You are the resolver agent.",
		"ENV placeholder substituted through the full path: {}",
		resolved_role.config.system
	);
	let statuses = statuses.into_inner().expect("statuses lock");
	assert!(
		statuses.contains(&"Fetching agent: ztest:zz".to_string()),
		"status callback saw the fetch: {statuses:?}"
	);
	assert!(
		statuses.contains(&"Checking dep: ztest/probe".to_string()),
		"status callback saw the dep check: {statuses:?}"
	);
	// The manifest is cached for future runs.
	assert!(
		guard.path().join("agents").join("ztest").join("zz.toml").exists(),
		"manifest cached under <data>/agents"
	);
}

#[tokio::test]
#[serial]
async fn resolve_config_and_role_applies_tap_model_override() {
	let guard = ResolverDataDir::new();
	install_default_tap_agent(
		guard.path(),
		"[[roles]]\nname = \"ztest:zz\"\nsystem = \"probe\"\nwelcome = \"Hi.\"\ntemperature = 0.1\ntop_p = 0.9\ntop_k = 40\n\n[deps]\nrequire = [\"ztest/probe\"]\n",
	);

	let mut config = base_config();
	config.taps.insert("ztest:zz".to_string(), "openai:gpt-4o".to_string());

	let (merged, role) = resolve_config_and_role(Some("ztest:zz"), &config, None)
		.await
		.expect("tag resolves");
	assert_eq!(role, "ztest:zz");
	assert_eq!(
		merged.model, "openai:gpt-4o",
		"[taps] model override must win for this tag"
	);
	assert_eq!(merged.model_profile.temperature, config.model_profile.temperature);
	assert_eq!(merged.model_profile.top_p, config.model_profile.top_p);
	assert_eq!(merged.model_profile.top_k, config.model_profile.top_k);
	assert_eq!(merged.model_profile.max_tokens, config.model_profile.max_tokens);
	assert_eq!(merged.model_profile.max_retries, config.model_profile.max_retries);
}

#[tokio::test]
#[serial]
async fn resolve_config_and_role_rejects_manifest_without_new_role() {
	let guard = ResolverDataDir::new();
	install_default_tap_agent(guard.path(), "version = 1\n");

	let err = resolve_config_and_role(Some("ztest:zz"), &base_config(), None)
		.await
		.expect_err("manifest without [[roles]] must fail");
	assert!(
		err.to_string().contains("must define at least one new [[roles]] entry"),
		"got: {err:#}"
	);
}

// --- bare domain resolution --------------------------------------------------

const MANIFEST_BODY: &str =
	"\n[[roles]]\nsystem = \"probe\"\nwelcome = \"Hi.\"\ntemperature = 0.1\ntop_p = 0.9\ntop_k = 40\n";

/// Write a manifest with the given header comment lines into the default tap.
fn install_default_tap_manifest(data_dir: &std::path::Path, category: &str, variant: &str, header: &str) {
	let dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join(category);
	std::fs::create_dir_all(&dir).expect("create agent dir");
	std::fs::write(dir.join(format!("{variant}.toml")), format!("{header}{MANIFEST_BODY}")).expect("write manifest");
}

const MANIFEST_HEADER: &str = "# Title: T\n# Description: D\n";

#[tokio::test]
#[serial]
async fn bare_domain_resolves_to_its_chief() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "chief", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some("ztest"), &base_config(), None)
		.await
		.expect("domain resolves");

	assert_eq!(role, "ztest:chief");
	assert!(merged.roles.iter().any(|r| r.name == "ztest:chief"));
}

/// Set an environment variable for the duration of a test, restoring the previous value on drop.
struct EnvVarGuard {
	name: &'static str,
	previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
	fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
		let previous = std::env::var_os(name);
		std::env::set_var(name, value);
		Self { name, previous }
	}
}

impl Drop for EnvVarGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var(self.name, value),
			None => std::env::remove_var(self.name),
		}
	}
}

/// `/role` and the `tap` tool call `expand_domain_role` directly, so the expansion itself has to
/// bring in the default tap. The remote is a local repository that git reaches through `insteadOf`.
#[test]
#[serial]
fn bare_domain_expands_when_the_default_tap_is_not_cloned_yet() {
	let _guard = ResolverDataDir::new();
	let remote_root = tempfile::tempdir().expect("remote root");
	let remote = remote_root.path().join("muvon").join("octomind-tap");
	let agents_dir = remote.join("agents").join("ztest");
	std::fs::create_dir_all(&agents_dir).expect("create remote");
	std::fs::write(
		agents_dir.join("chief.toml"),
		format!("{MANIFEST_HEADER}{MANIFEST_BODY}"),
	)
	.expect("write manifest");
	for args in [
		vec!["init"],
		vec!["add", "."],
		vec![
			"-c",
			"user.name=test",
			"-c",
			"user.email=test@example.com",
			"-c",
			"commit.gpgsign=false",
			"commit",
			"-m",
			"init",
		],
	] {
		let status = std::process::Command::new("git")
			.args(&args)
			.current_dir(&remote)
			.stdout(std::process::Stdio::null())
			.stderr(std::process::Stdio::null())
			.status()
			.expect("run git");
		assert!(status.success(), "git {args:?} failed");
	}
	let _prompt = EnvVarGuard::set("GIT_TERMINAL_PROMPT", "0");
	let _count = EnvVarGuard::set("GIT_CONFIG_COUNT", "1");
	let _key = EnvVarGuard::set(
		"GIT_CONFIG_KEY_0",
		format!("url.file://{}/.insteadOf", remote_root.path().display()),
	);
	let _value = EnvVarGuard::set("GIT_CONFIG_VALUE_0", "https://github.com/");

	let role = expand_domain_role("ztest", &base_config()).expect("domain expands");

	assert_eq!(role, "ztest:chief");
}

#[tokio::test]
#[serial]
async fn local_config_role_wins_over_a_domain_of_the_same_name() {
	let guard = ResolverDataDir::new();
	let config = base_config();
	let local_role = config.roles[0].name.clone();
	install_default_tap_manifest(guard.path(), &local_role, "chief", MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some(&local_role), &config, None)
		.await
		.expect("plain role resolves");

	assert_eq!(role, local_role);
	assert_eq!(merged.roles.len(), config.roles.len(), "no tap manifest merged");
}

#[tokio::test]
#[serial]
async fn plain_name_that_is_not_a_domain_stays_a_plain_role() {
	let guard = ResolverDataDir::new();
	// The built-in tap must exist, or the resolver would try to clone it.
	install_default_tap_manifest(guard.path(), "ztest", "chief", MANIFEST_HEADER);

	let (_, role) = resolve_config_and_role(Some("nowhere"), &base_config(), None)
		.await
		.expect("plain role");

	assert_eq!(role, "nowhere");
}

#[tokio::test]
#[serial]
async fn domain_without_chief_fails_with_the_available_tags() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);

	let err = resolve_config_and_role(Some("ztest"), &base_config(), None)
		.await
		.expect_err("no orchestrator");

	let message = format!("{err:#}");
	assert!(
		message.contains("no orchestrator") && message.contains("ztest:zz"),
		"{message}"
	);
}

// --- model override per domain -----------------------------------------------

#[tokio::test]
#[serial]
async fn domain_key_sets_the_model_when_no_exact_tag_key_exists() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "chief", MANIFEST_HEADER);
	let mut config = base_config();
	config.taps.insert("ztest".to_string(), "openai:gpt-4o".to_string());

	let (merged, _) = resolve_config_and_role(Some("ztest"), &config, None)
		.await
		.expect("resolves");

	assert_eq!(merged.model, "openai:gpt-4o");
}

#[tokio::test]
#[serial]
async fn exact_tag_key_beats_the_domain_key() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);
	let mut config = base_config();
	config.taps.insert("ztest".to_string(), "openai:gpt-4o".to_string());
	config
		.taps
		.insert("ztest:zz".to_string(), "openai:gpt-4o-mini".to_string());

	let (merged, _) = resolve_config_and_role(Some("ztest:zz"), &config, None)
		.await
		.expect("resolves");

	assert_eq!(merged.model, "openai:gpt-4o-mini");
}

#[tokio::test]
#[serial]
async fn renamed_tag_resolves_to_the_new_agent_and_keeps_its_old_model_key() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "developer", "chief", MANIFEST_HEADER);
	let mut config = base_config();
	config.taps.insert("developer".to_string(), "openai:gpt-4o".to_string());
	config
		.taps
		.insert("developer:cto".to_string(), "openai:gpt-4o-mini".to_string());

	let (merged, role) = resolve_config_and_role(Some("developer:cto"), &config, None)
		.await
		.expect("resolves");

	assert_eq!(role, "developer:chief");
	assert_eq!(merged.model, "openai:gpt-4o-mini");
}

// --- roster placeholders -------------------------------------------------------

fn roster_system(manifest: &str, tag: &str) -> String {
	let prepared = prepare_role_manifest(manifest, tag).expect("prepare");
	let value: toml::Value = toml::from_str(&prepared).expect("valid toml");
	value["roles"][0]["system"].as_str().expect("system string").to_string()
}

const ROSTER_MANIFEST: &str = "[[roles]]\nsystem = \"Team:\\n{{SPECIALISTS}}\"\n";

#[test]
#[serial]
fn roster_lists_other_agents_of_the_domain_sorted() {
	let guard = ResolverDataDir::new();
	// Written out of order on purpose; the roster must come out sorted.
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "ztest", "mm", "# Description: He said \"hi\" \\ there\n");
	install_default_tap_manifest(guard.path(), "ztest", "aa", "# Title: Only a title\n");
	// Agents of other domains never appear in `{{SPECIALISTS}}`.
	install_default_tap_manifest(guard.path(), "yother", "chief", "# Description: Runs yother\n");

	let system = roster_system(ROSTER_MANIFEST, "ztest:zz");

	assert_eq!(system, "Team:\n- ztest:aa\n- ztest:mm — He said \"hi\" \\ there");
}

#[test]
#[serial]
fn roster_with_nothing_to_list_says_none() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);

	assert_eq!(roster_system(ROSTER_MANIFEST, "ztest:zz"), "Team:\n(none)");
}

#[test]
#[serial]
fn manifest_without_placeholders_is_left_alone() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "other", MANIFEST_HEADER);

	assert_eq!(
		roster_system("[[roles]]\nsystem = \"plain {{CWD}} prompt\"\n", "ztest:zz"),
		"plain {{CWD}} prompt"
	);
}

// --- {{USES}} ------------------------------------------------------------------

const USES_MANIFEST: &str = "[[roles]]\nsystem = \"Can call:\\n{{USES}}\"\n";

fn install_uses_agent(data_dir: &std::path::Path, uses: &str) {
	install_default_tap_manifest(data_dir, "mkt", "chief", &format!("# Description: Runs mkt\n{uses}"));
}

#[test]
#[serial]
fn uses_lists_domain_chief_and_tag_entries_sorted() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: seo:audit, content\n");
	install_default_tap_manifest(guard.path(), "content", "chief", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "content", "writer", "# Description: Not listed\n");
	install_default_tap_manifest(guard.path(), "seo", "audit", "# Description: Audits pages\n");

	assert_eq!(
		roster_system(USES_MANIFEST, "mkt:chief"),
		"Can call:\n- content — Runs content\n- seo:audit — Audits pages"
	);
}

#[test]
#[serial]
fn uses_star_lists_every_other_domain_with_a_chief() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *\n");
	// Written out of order on purpose; `x` sorts before `x-y` as a domain, not as a tag.
	install_default_tap_manifest(guard.path(), "x-y", "chief", "# Description: Runs x-y\n");
	install_default_tap_manifest(guard.path(), "x", "chief", "# Description: Runs x\n");
	install_default_tap_manifest(guard.path(), "content", "chief", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "content", "writer", "# Description: Not listed\n");
	// A domain without a chief is skipped, not an error.
	install_default_tap_manifest(guard.path(), "nochief", "x", "# Description: Has no chief\n");

	// The agent's own domain (`mkt`) is left out.
	assert_eq!(
		roster_system(USES_MANIFEST, "mkt:chief"),
		"Can call:\n- content — Runs content\n- x — Runs x\n- x-y — Runs x-y"
	);
}

#[test]
#[serial]
fn uses_star_combined_with_exact_entries_has_no_duplicates() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *, content, seo:audit\n");
	install_default_tap_manifest(guard.path(), "content", "chief", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "seo", "chief", "# Description: Runs seo\n");
	install_default_tap_manifest(guard.path(), "seo", "audit", "# Description: Audits pages\n");

	assert_eq!(
		roster_system(USES_MANIFEST, "mkt:chief"),
		"Can call:\n- content — Runs content\n- seo — Runs seo\n- seo:audit — Audits pages"
	);
}

#[test]
#[serial]
fn uses_star_with_no_other_domain_chief_says_none() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *\n");
	install_default_tap_manifest(guard.path(), "nochief", "x", MANIFEST_HEADER);

	assert_eq!(roster_system(USES_MANIFEST, "mkt:chief"), "Can call:\n(none)");
}

#[test]
#[serial]
fn uses_without_header_says_none() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "");

	assert_eq!(roster_system(USES_MANIFEST, "mkt:chief"), "Can call:\n(none)");
}

#[test]
fn escaped_uses_placeholder_stays_literal() {
	let manifest = "[[roles]]\nsystem = \"Write {{{{USES}}}} verbatim\"\n";

	assert_eq!(roster_system(manifest, "mkt:chief"), "Write {{USES}} verbatim");
}

#[test]
#[serial]
fn uses_domain_without_chief_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: content\n");
	install_default_tap_manifest(guard.path(), "content", "writer", MANIFEST_HEADER);

	let err = prepare_role_manifest(USES_MANIFEST, "mkt:chief")
		.unwrap_err()
		.to_string();
	assert_eq!(
		err,
		"`mkt:chief` declares `# Uses: content`, but domain `content` has no `chief`"
	);
}

#[test]
#[serial]
fn uses_missing_tag_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: seo:audit\n");

	let err = prepare_role_manifest(USES_MANIFEST, "mkt:chief")
		.unwrap_err()
		.to_string();
	assert_eq!(
		err,
		"`mkt:chief` declares `# Uses: seo:audit`, but no such agent exists"
	);
}

#[test]
#[serial]
fn uses_empty_entry_is_an_error_only_for_its_declaring_agent() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: content,,seo:audit\n");

	let err = prepare_role_manifest(USES_MANIFEST, "mkt:chief")
		.unwrap_err()
		.to_string();
	assert_eq!(
		err,
		"`mkt:chief` declares `# Uses: content, , seo:audit`, which has an empty entry"
	);
	// Another agent's roster is unaffected by the malformed header.
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);
	assert_eq!(roster_system(USES_MANIFEST, "ztest:zz"), "Can call:\n(none)");
}

#[test]
#[serial]
fn uses_own_domain_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: mkt\n");

	let err = prepare_role_manifest(USES_MANIFEST, "mkt:chief")
		.unwrap_err()
		.to_string();
	assert_eq!(
		err,
		"`mkt:chief` declares `# Uses: mkt`, but `mkt` is its own domain: use `{{SPECIALISTS}}` for your own domain"
	);
}
