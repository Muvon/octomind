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

/// A default tap clone that is due for its refresh and has `zlate:zz` only, plus a `git` first
/// on PATH whose "pull" adds `zlate:chief`: a clone on the first run after an upgrade that
/// renamed an agent. The returned guards must live as long as the test.
#[cfg(unix)]
fn stale_default_tap_whose_refresh_adds_a_chief(data_dir: &std::path::Path) -> (tempfile::TempDir, EnvVarGuard) {
	use std::os::unix::fs::PermissionsExt;

	install_default_tap_manifest(data_dir, "zlate", "zz", MANIFEST_HEADER);
	let tap_dir = data_dir.join("taps").join("muvon").join("octomind-tap");
	std::fs::create_dir_all(tap_dir.join(".git")).expect("create .git");

	let fake_git_dir = tempfile::tempdir().expect("fake git dir");
	let chief = fake_git_dir.path().join("chief.toml");
	std::fs::write(&chief, format!("{MANIFEST_HEADER}{MANIFEST_BODY}")).expect("write late manifest");
	let git = fake_git_dir.path().join("git");
	// The pause lets the first lookup read the clone before the pull lands.
	std::fs::write(
		&git,
		format!(
			"#!/bin/sh\nsleep 0.3\ncp '{}' agents/zlate/chief.toml\n",
			chief.display()
		),
	)
	.expect("write fake git");
	std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).expect("make fake git executable");
	let search_path = format!(
		"{}:{}",
		fake_git_dir.path().display(),
		std::env::var("PATH").unwrap_or_default()
	);
	(fake_git_dir, EnvVarGuard::set("PATH", search_path))
}

#[cfg(unix)]
#[test]
#[serial]
fn bare_domain_waits_for_the_refresh_that_brings_its_chief() {
	let guard = ResolverDataDir::new();
	let _fake_git = stale_default_tap_whose_refresh_adds_a_chief(guard.path());

	let role = expand_domain_role("zlate", &base_config()).expect("domain expands once the refresh has ended");

	assert_eq!(role, "zlate:chief");
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn tag_missing_from_a_stale_clone_is_fetched_after_the_refresh() {
	let guard = ResolverDataDir::new();
	let _fake_git = stale_default_tap_whose_refresh_adds_a_chief(guard.path());

	let (merged, role) = resolve_config_and_role(Some("zlate:chief"), &base_config(), None)
		.await
		.expect("tag resolves once the refresh has ended");

	assert_eq!(role, "zlate:chief");
	assert!(merged.roles.iter().any(|r| r.name == "zlate:chief"));
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

// Symlinks need elevated rights on Windows, so the alias rule is checked on unix only.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn alias_tag_resolves_to_its_target_and_keeps_its_own_model_key() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "chief", MANIFEST_HEADER);
	// The tap renamed `ztest:old` to `ztest:chief` and kept the old name as a symlink.
	let domain_dir = guard
		.path()
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join("ztest");
	std::os::unix::fs::symlink("chief.toml", domain_dir.join("old.toml")).expect("create alias");
	let mut config = base_config();
	config.taps.insert("ztest".to_string(), "openai:gpt-4o".to_string());
	config
		.taps
		.insert("ztest:old".to_string(), "openai:gpt-4o-mini".to_string());

	let (merged, role) = resolve_config_and_role(Some("ztest:old"), &config, None)
		.await
		.expect("resolves");

	assert_eq!(role, "ztest:chief");
	assert_eq!(merged.model, "openai:gpt-4o-mini");
}

// --- which agents a caller can hand a task to ----------------------------------
/// The `# Uses:` error loading `tag` would fail with, if any.
fn reach_error(tag: &str) -> Option<String> {
	let agents = crate::agent::taps::list_agent_headers().expect("agent headers");
	reachable_specialists(tag, &agents).err().map(|error| error.to_string())
}

fn reachable_tags(tag: &str) -> Vec<String> {
	let agents = crate::agent::taps::list_agent_headers().expect("agent headers");
	reachable_specialists(tag, &agents)
		.expect("reachable specialists")
		.into_iter()
		.map(|agent| agent.tag.clone())
		.collect()
}

#[test]
#[serial]
fn tap_agent_reaches_other_agents_of_its_domain_sorted() {
	let guard = ResolverDataDir::new();
	// Written out of order on purpose; the result must come out sorted.
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "ztest", "mm", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "ztest", "aa", "# Title: Only a title\n");
	// Agents of other domains are out of reach without a `# Uses:` header.
	install_default_tap_manifest(guard.path(), "yother", "chief", "# Description: Runs yother\n");

	assert_eq!(reachable_tags("ztest:zz"), ["ztest:aa", "ztest:mm"]);
}

#[test]
#[serial]
fn caller_outside_the_taps_reaches_every_agent() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "yother", "chief", MANIFEST_HEADER);

	assert_eq!(reachable_tags("local-role"), ["yother:chief", "ztest:zz"]);
}

// --- `# Uses:` header ----------------------------------------------------------
fn install_uses_agent(data_dir: &std::path::Path, uses: &str) {
	install_default_tap_manifest(data_dir, "mkt", "chief", &format!("# Description: Runs mkt\n{uses}"));
}

#[test]
#[serial]
fn uses_reaches_domain_chief_and_tag_entries() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: seo:audit, content\n");
	install_default_tap_manifest(guard.path(), "mkt", "email", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "content", "chief", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "content", "writer", "# Description: Out of reach\n");
	install_default_tap_manifest(guard.path(), "seo", "audit", "# Description: Audits pages\n");

	assert_eq!(reachable_tags("mkt:chief"), ["content:chief", "mkt:email", "seo:audit"]);
}

#[test]
#[serial]
fn uses_star_reaches_every_other_domain_with_a_chief() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *\n");
	install_default_tap_manifest(guard.path(), "x-y", "chief", "# Description: Runs x-y\n");
	install_default_tap_manifest(guard.path(), "x", "chief", "# Description: Runs x\n");
	install_default_tap_manifest(guard.path(), "content", "chief", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "content", "writer", "# Description: Out of reach\n");
	// A domain without a chief is skipped, not an error.
	install_default_tap_manifest(guard.path(), "nochief", "x", "# Description: Has no chief\n");

	assert_eq!(reachable_tags("mkt:chief"), ["content:chief", "x-y:chief", "x:chief"]);
}

#[test]
#[serial]
fn uses_star_combined_with_exact_entries_has_no_duplicates() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *, content, seo:audit\n");
	install_default_tap_manifest(guard.path(), "content", "chief", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "seo", "chief", "# Description: Runs seo\n");
	install_default_tap_manifest(guard.path(), "seo", "audit", "# Description: Audits pages\n");

	assert_eq!(reachable_tags("mkt:chief"), ["content:chief", "seo:audit", "seo:chief"]);
}

#[test]
#[serial]
fn tap_agent_alone_in_its_domain_without_uses_reaches_nothing() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "");
	install_default_tap_manifest(guard.path(), "nochief", "x", MANIFEST_HEADER);

	assert!(reachable_tags("mkt:chief").is_empty());
}

#[test]
#[serial]
fn uses_domain_without_chief_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: content\n");
	install_default_tap_manifest(guard.path(), "content", "writer", MANIFEST_HEADER);

	let err = reach_error("mkt:chief").expect("bad header");
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

	let err = reach_error("mkt:chief").expect("bad header");
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

	let err = reach_error("mkt:chief").expect("bad header");
	assert_eq!(
		err,
		"`mkt:chief` declares `# Uses: content, , seo:audit`, which has an empty entry"
	);
	// Another agent's prompt is unaffected by the malformed header.
	install_default_tap_manifest(guard.path(), "ztest", "zz", MANIFEST_HEADER);
	assert!(reach_error("ztest:zz").is_none());
}

#[test]
#[serial]
fn uses_own_domain_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: mkt\n");

	let err = reach_error("mkt:chief").expect("bad header");
	assert_eq!(
		err,
		"`mkt:chief` declares `# Uses: mkt`, but `mkt` is its own domain: its agents are reachable without it"
	);
}
