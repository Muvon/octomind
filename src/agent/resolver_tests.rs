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
	let injected = prepare_role_manifest(manifest, "doctor").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert_eq!(value["roles"][0]["name"].as_str(), Some("doctor"));
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

/// Create the default tap tree with one agent manifest named `zz` and a passing dep
/// script, so `fetch_manifest`/`run_dep_entries` work fully offline.
fn install_default_tap_agent(data_dir: &std::path::Path, manifest: &str) {
	let agent_dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join("zz");
	std::fs::create_dir_all(&agent_dir).expect("create agent dir");
	std::fs::write(agent_dir.join("default.toml"), manifest).expect("write manifest");

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
		toml::from_str(&crate::config::template_with_assistant_role::template_with_assistant_role())
			.expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
#[serial]
async fn resolve_config_and_role_merges_tap_manifest_end_to_end() {
	let guard = ResolverDataDir::new();
	install_default_tap_agent(
		guard.path(),
		"[[roles]]\nname = \"ignored\"\nsystem = \"You are the {{ENV:OCTOMIND_RESOLVER_PROBE}} agent.\"\nwelcome = \"Hi.\"\n\n[deps]\nrequire = [\"ztest/probe\"]\n",
	);

	let had_probe = std::env::var_os("OCTOMIND_RESOLVER_PROBE");
	std::env::set_var("OCTOMIND_RESOLVER_PROBE", "resolver");

	let statuses: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
	let (merged, role) = resolve_config_and_role(
		Some("zz"),
		&base_config(),
		Some(&|status| statuses.lock().expect("statuses lock").push(status.to_string())),
	)
	.await
	.expect("tag resolves end to end");

	match had_probe {
		Some(value) => std::env::set_var("OCTOMIND_RESOLVER_PROBE", value),
		None => std::env::remove_var("OCTOMIND_RESOLVER_PROBE"),
	}

	assert_eq!(role, "zz", "the tag itself is the role identity");
	let resolved_role = merged
		.roles
		.iter()
		.find(|r| r.name == "zz")
		.expect("merged config carries the tap role");
	assert_eq!(
		resolved_role.config.system, "You are the resolver agent.",
		"ENV placeholder substituted through the full path: {}",
		resolved_role.config.system
	);
	let statuses = statuses.into_inner().expect("statuses lock");
	assert!(
		statuses.contains(&"Fetching agent: zz".to_string()),
		"status callback saw the fetch: {statuses:?}"
	);
	assert!(
		statuses.contains(&"Checking dep: ztest/probe".to_string()),
		"status callback saw the dep check: {statuses:?}"
	);
	// The manifest is cached for future runs, in a file keyed by the name and its tap and ref.
	let cached = std::fs::read_dir(guard.path().join("agents"))
		.expect("cache dir exists")
		.map(|entry| entry.expect("cache entry").file_name().to_string_lossy().into_owned())
		.any(|file| file.starts_with("zz.") && file.ends_with(".toml"));
	assert!(cached, "manifest cached under <data>/agents/<name>.<tap and ref>.toml");
}

#[tokio::test]
#[serial]
async fn resolve_config_and_role_applies_tap_model_override_keyed_by_the_agent_name() {
	let guard = ResolverDataDir::new();
	install_default_tap_agent(
		guard.path(),
		"[[roles]]\nname = \"zz\"\nsystem = \"probe\"\nwelcome = \"Hi.\"\n\n[deps]\nrequire = [\"ztest/probe\"]\n",
	);

	let mut config = base_config();
	config.taps.insert("zz".to_string(), "openai:gpt-4o".to_string());

	let (merged, role) = resolve_config_and_role(Some("zz"), &config, None)
		.await
		.expect("tag resolves");
	assert_eq!(role, "zz");
	assert_eq!(
		merged.model, "openai:gpt-4o",
		"[taps] model override must win for this agent"
	);
	assert_eq!(merged.model_profile.max_tokens, config.model_profile.max_tokens);
	assert_eq!(merged.model_profile.max_retries, config.model_profile.max_retries);
}

#[tokio::test]
#[serial]
async fn resolve_config_and_role_rejects_manifest_without_new_role() {
	let guard = ResolverDataDir::new();
	install_default_tap_agent(guard.path(), "version = 1\n");

	let err = resolve_config_and_role(Some("zz"), &base_config(), None)
		.await
		.expect_err("manifest without [[roles]] must fail");
	assert!(
		err.to_string().contains("must define at least one new [[roles]] entry"),
		"got: {err:#}"
	);
}

// --- flat tag resolution ------------------------------------------------------

const MANIFEST_BODY: &str = "\n[[roles]]\nsystem = \"probe\"\nwelcome = \"Hi.\"\n";

/// Write a manifest with the given header comment lines into the default tap.
fn install_default_tap_manifest(data_dir: &std::path::Path, name: &str, header: &str) {
	let dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join(name);
	std::fs::create_dir_all(&dir).expect("create agent dir");
	std::fs::write(dir.join("default.toml"), format!("{header}{MANIFEST_BODY}")).expect("write manifest");
}

/// Write a variant manifest `agents/<namespace>/<variant>.toml` into the default tap.
fn install_default_tap_variant(data_dir: &std::path::Path, namespace: &str, variant: &str, header: &str) {
	let dir = data_dir
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join(namespace);
	std::fs::create_dir_all(&dir).expect("create namespace dir");
	std::fs::write(dir.join(format!("{variant}.toml")), format!("{header}{MANIFEST_BODY}")).expect("write manifest");
}

const MANIFEST_HEADER: &str = "# Title: T\n# Description: D\n";

#[tokio::test]
#[serial]
async fn flat_name_resolves_to_its_tap_agent() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "other", MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some("zz"), &base_config(), None)
		.await
		.expect("agent resolves");

	assert_eq!(role, "zz");
	assert!(merged.roles.iter().any(|r| r.name == "zz"));
	assert!(
		!merged.roles.iter().any(|r| r.name == "other"),
		"only the named agent is merged"
	);
}

#[tokio::test]
#[serial]
async fn the_default_variant_is_the_base_agent_for_the_local_role_check_and_the_taps_key() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);

	// A local role named like the base agent wins over the tap agent, however it is spelled.
	let mut config = base_config();
	config.roles.push(config.roles[0].clone());
	config.roles.last_mut().unwrap().name = "ztest".to_string();
	let (_, role) = resolve_config_and_role(Some("ztest:default"), &config, None)
		.await
		.expect("local role resolves");
	assert_eq!(role, "ztest");
	assert_eq!(config.roles.iter().filter(|r| r.name == "ztest").count(), 1);

	// A `[taps]` key under the base name applies to `<ns>:default`.
	let mut config = base_config();
	config.taps.insert("ztest".to_string(), "ollama:base-model".to_string());
	let (merged, role) = resolve_config_and_role(Some("ztest:default"), &config, None)
		.await
		.expect("agent resolves");
	assert_eq!(role, "ztest");
	assert_eq!(merged.model_profile.model, "ollama:base-model");
}

#[tokio::test]
#[serial]
async fn a_tag_that_is_not_valid_is_refused_before_any_path_is_built_from_it() {
	let _guard = ResolverDataDir::new();
	for bad in ["../../x", "a/b", "dev:../x", "dev@../..:x", "Dev"] {
		let err = resolve_config_and_role(Some(bad), &base_config(), None)
			.await
			.expect_err(bad)
			.to_string();
		assert!(err.contains("Invalid agent tag"), "{bad}: {err}");
		assert!(!err.contains("agents/"), "{bad} leaked a path: {err}");
	}
}

#[tokio::test]
#[serial]
async fn a_variant_tag_resolves_to_its_own_manifest_and_role() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "ztest", "rust", MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some("ztest:rust"), &base_config(), None)
		.await
		.expect("variant resolves");

	assert_eq!(role, "ztest:rust");
	assert!(merged.roles.iter().any(|r| r.name == "ztest:rust"));
	assert!(
		!merged.roles.iter().any(|r| r.name == "ztest"),
		"only the variant is merged"
	);
}

#[tokio::test]
#[serial]
async fn the_default_variant_is_the_base_agent() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some("ztest:default"), &base_config(), None)
		.await
		.expect("ztest:default resolves");

	assert_eq!(role, "ztest", "the canonical form is the namespace");
	assert!(merged.roles.iter().any(|r| r.name == "ztest"));
	assert!(!merged.roles.iter().any(|r| r.name == "ztest:default"));
}

#[tokio::test]
#[serial]
async fn a_namespace_with_variants_and_no_base_resolves_its_variants_only() {
	let guard = ResolverDataDir::new();
	install_default_tap_variant(guard.path(), "private", "reviewer", MANIFEST_HEADER);

	let (_, role) = resolve_config_and_role(Some("private:reviewer"), &base_config(), None)
		.await
		.expect("variant of a namespace without a base");
	assert_eq!(role, "private:reviewer");

	let err = resolve_config_and_role(Some("private"), &base_config(), None)
		.await
		.expect_err("the base agent does not exist");
	let message = format!("{err:#}");
	assert!(message.contains("namespace 'private'"), "{message}");
	assert!(message.contains("private:reviewer"), "{message}");
}

#[tokio::test]
#[serial]
async fn an_unknown_variant_names_the_namespace_and_lists_the_agents_it_has() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "ztest", "rust", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "ztest", "go", MANIFEST_HEADER);

	let err = resolve_config_and_role(Some("ztest:cobol"), &base_config(), None)
		.await
		.expect_err("no such variant");
	let message = format!("{err:#}");
	assert!(message.contains("There is no agent 'ztest:cobol'"), "{message}");
	assert!(message.contains("namespace 'ztest'"), "{message}");
	assert!(message.contains("ztest, ztest:go, ztest:rust"), "{message}");
}

#[tokio::test]
#[serial]
async fn a_tag_with_a_colon_that_no_tap_has_is_an_error_and_a_plain_name_is_not() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);

	let err = resolve_config_and_role(Some("nowhere:x"), &base_config(), None)
		.await
		.expect_err("a variant of no namespace");
	assert!(format!("{err:#}").contains("not in any installed tap"));

	let (_, role) = resolve_config_and_role(Some("nowhere"), &base_config(), None)
		.await
		.expect("a plain name stays a plain role");
	assert_eq!(role, "nowhere");
}

#[tokio::test]
#[serial]
async fn a_local_role_with_a_colon_wins_over_the_tap() {
	let guard = ResolverDataDir::new();
	install_default_tap_variant(guard.path(), "acme", "reviewer", MANIFEST_HEADER);
	let mut config = base_config();
	config.roles.push(config.roles[0].clone());
	config.roles.last_mut().unwrap().name = "acme:reviewer".to_string();
	config.build_role_map();

	let (resolved, role) = resolve_config_and_role(Some("acme:reviewer"), &config, None)
		.await
		.expect("the local role resolves");
	assert_eq!(role, "acme:reviewer");
	assert_eq!(resolved.roles.len(), config.roles.len(), "nothing merged from the tap");
}

#[tokio::test]
#[serial]
async fn the_taps_model_override_of_a_variant_falls_back_to_its_namespace() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "ztest", "rust", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "ztest", "go", MANIFEST_HEADER);
	let mut config = base_config();
	config.taps.insert("ztest".to_string(), "ollama:ns-model".to_string());
	config
		.taps
		.insert("ztest:go".to_string(), "ollama:go-model".to_string());

	let (rust, _) = resolve_config_and_role(Some("ztest:rust"), &config, None)
		.await
		.expect("variant resolves");
	assert_eq!(rust.model_profile.model, "ollama:ns-model");
	let (go, _) = resolve_config_and_role(Some("ztest:go"), &config, None)
		.await
		.expect("variant resolves");
	assert_eq!(go.model_profile.model, "ollama:go-model", "an exact tag wins");
}

// Symlinks need elevated rights on Windows, so the alias rule is checked on unix only.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn an_alias_tag_resolves_to_its_target_and_keeps_its_own_model_key() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);
	// The tap renamed `ztest:chief` to the base agent `ztest` and kept the old name as a symlink.
	let namespace_dir = guard
		.path()
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("agents")
		.join("ztest");
	std::os::unix::fs::symlink("default.toml", namespace_dir.join("chief.toml")).expect("create alias");
	let mut config = base_config();
	config
		.taps
		.insert("ztest:chief".to_string(), "ollama:alias-model".to_string());

	let (merged, role) = resolve_config_and_role(Some("ztest:chief"), &config, None)
		.await
		.expect("alias resolves");

	assert_eq!(role, "ztest");
	assert!(merged.roles.iter().any(|r| r.name == "ztest"));
	assert!(!merged.roles.iter().any(|r| r.name == "ztest:chief"));
	assert_eq!(
		merged.model_profile.model, "ollama:alias-model",
		"a key under the old name applies"
	);
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

/// Serve the built-in tap at `git_ref`, then go back to the template default when dropped.
struct BuiltInRefGuard;

impl BuiltInRefGuard {
	fn set(git_ref: &str) -> Self {
		crate::agent::taps::set_built_in_tap(&crate::config::registry::RegistryConfig {
			git_ref: git_ref.to_string(),
			..Default::default()
		});
		Self
	}
}

impl Drop for BuiltInRefGuard {
	fn drop(&mut self) {
		crate::agent::taps::set_built_in_tap(&crate::config::registry::RegistryConfig::default());
	}
}

/// Resolution has to bring in the built-in tap itself: a new user's first run may name one of
/// its agents. The remote is a local repository that git reaches through `insteadOf`.
#[test]
#[serial]
fn agent_lookup_clones_the_built_in_tap_at_its_ref_when_it_is_not_cloned_yet() {
	let _guard = ResolverDataDir::new();
	let _ref = BuiltInRefGuard::set("flat-agents");
	let remote_root = tempfile::tempdir().expect("remote root");
	let remote = remote_root.path().join("muvon").join("octomind-tap");
	let agent_dir = remote.join("agents").join("ztest");
	std::fs::create_dir_all(&agent_dir).expect("create remote");
	std::fs::write(
		agent_dir.join("default.toml"),
		format!("{MANIFEST_HEADER}{MANIFEST_BODY}"),
	)
	.expect("write manifest");
	for args in [
		vec!["init"],
		vec!["checkout", "-b", "flat-agents"],
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

	assert!(
		tap_has_agent("ztest").expect("lookup"),
		"the agent is on the configured ref"
	);
	assert!(!tap_has_agent("nowhere").expect("lookup"));
}

/// A default tap clone that is due for its refresh and has `zz` only, plus a `git` first
/// on PATH whose "pull" adds `late`: a clone on the first run after an upgrade that
/// published an agent. The returned guards must live as long as the test.
#[cfg(unix)]
fn stale_default_tap_whose_refresh_adds_an_agent(data_dir: &std::path::Path) -> (tempfile::TempDir, EnvVarGuard) {
	use std::os::unix::fs::PermissionsExt;

	install_default_tap_manifest(data_dir, "zz", MANIFEST_HEADER);
	let tap_dir = data_dir.join("taps").join("muvon").join("octomind-tap");
	std::fs::create_dir_all(tap_dir.join(".git")).expect("create .git");

	let fake_git_dir = tempfile::tempdir().expect("fake git dir");
	let late = fake_git_dir.path().join("late.toml");
	std::fs::write(&late, format!("{MANIFEST_HEADER}{MANIFEST_BODY}")).expect("write late manifest");
	let git = fake_git_dir.path().join("git");
	// The pause lets the first lookup read the clone before the pull lands.
	std::fs::write(
		&git,
		format!(
			"#!/bin/sh\nsleep 0.3\nmkdir -p agents/late\ncp '{}' agents/late/default.toml\n",
			late.display()
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
fn agent_lookup_waits_for_the_refresh_that_brings_the_agent() {
	let guard = ResolverDataDir::new();
	let _fake_git = stale_default_tap_whose_refresh_adds_an_agent(guard.path());

	assert!(
		tap_has_agent("late").expect("lookup"),
		"the lookup retries once the refresh has ended"
	);
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn tag_missing_from_a_stale_clone_is_fetched_after_the_refresh() {
	let guard = ResolverDataDir::new();
	let _fake_git = stale_default_tap_whose_refresh_adds_an_agent(guard.path());

	let (merged, role) = resolve_config_and_role(Some("late"), &base_config(), None)
		.await
		.expect("tag resolves once the refresh has ended");

	assert_eq!(role, "late");
	assert!(merged.roles.iter().any(|r| r.name == "late"));
}

#[tokio::test]
#[serial]
async fn local_config_role_wins_over_a_tap_agent_of_the_same_name() {
	let guard = ResolverDataDir::new();
	let config = base_config();
	let local_role = config.roles[0].name.clone();
	install_default_tap_manifest(guard.path(), &local_role, MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some(&local_role), &config, None)
		.await
		.expect("plain role resolves");

	assert_eq!(role, local_role);
	assert_eq!(merged.roles.len(), config.roles.len(), "no tap manifest merged");
	assert!(!resolved_from_tap(&local_role), "a local role is not a tap agent");
}

#[tokio::test]
#[serial]
async fn a_versioned_tag_names_the_role_without_the_version() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zver", MANIFEST_HEADER);

	let (merged, role) = resolve_config_and_role(Some("zver@1.2"), &base_config(), None)
		.await
		.expect("versioned tag resolves");

	assert_eq!(role, "zver");
	assert!(merged.roles.iter().any(|r| r.name == "zver"));
	assert!(resolved_from_tap("zver"));
}

#[tokio::test]
#[serial]
async fn plain_name_that_is_not_an_agent_stays_a_plain_role() {
	let guard = ResolverDataDir::new();
	// The built-in tap must exist, or the resolver would try to clone it.
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);

	let (_, role) = resolve_config_and_role(Some("nowhere"), &base_config(), None)
		.await
		.expect("plain role");

	assert_eq!(role, "nowhere");
}

#[tokio::test]
#[serial]
async fn a_flat_manifest_file_fails_resolution_naming_the_tap_the_file_and_its_new_path() {
	let guard = ResolverDataDir::new();
	// The built-in tap must exist, or the resolver would try to clone it.
	install_default_tap_manifest(guard.path(), "ztest", MANIFEST_HEADER);
	let agents = guard.path().join("taps/muvon/octomind-tap/agents");
	std::fs::write(agents.join("old.toml"), format!("{MANIFEST_HEADER}{MANIFEST_BODY}")).expect("write flat manifest");

	let err = resolve_config_and_role(Some("old"), &base_config(), None)
		.await
		.expect_err("the flat manifest is not read");
	let message = format!("{err:#}");
	assert!(message.contains("muvon/tap"), "{message}");
	assert!(
		message.contains(&agents.join("old.toml").display().to_string()),
		"{message}"
	);
	assert!(
		message.contains(&agents.join("old").join("default.toml").display().to_string()),
		"{message}"
	);
}

#[tokio::test]
#[serial]
async fn a_directory_manifest_wins_over_a_flat_file_of_the_same_name() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	let agents = guard.path().join("taps/muvon/octomind-tap/agents");
	std::fs::write(agents.join("zz.toml"), "not read").expect("write flat manifest");

	let (_, role) = resolve_config_and_role(Some("zz"), &base_config(), None)
		.await
		.expect("the directory form resolves");
	assert_eq!(role, "zz");
}

// --- which agents a caller can hand a task to ----------------------------------
/// The `# Uses:` error loading `tag` would fail with, if any.
fn reach_error(tag: &str) -> Option<String> {
	let agents = crate::agent::taps::list_agent_headers().expect("agent headers");
	reachable_agents(tag, true, &agents)
		.err()
		.map(|error| error.to_string())
}

fn reachable_tags(tag: &str) -> Vec<String> {
	reachable_tags_as(tag, true)
}

fn reachable_tags_as(tag: &str, caller_is_tap_agent: bool) -> Vec<String> {
	let agents = crate::agent::taps::list_agent_headers().expect("agent headers");
	reachable_agents(tag, caller_is_tap_agent, &agents)
		.expect("reachable agents")
		.into_iter()
		.map(|agent| agent.tag.clone())
		.collect()
}

#[test]
#[serial]
fn tap_agent_without_uses_reaches_nobody() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "mm", MANIFEST_HEADER);

	assert!(reachable_tags("zz").is_empty());
}

#[test]
#[serial]
fn caller_outside_the_taps_reaches_every_agent() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "yother", MANIFEST_HEADER);

	assert_eq!(reachable_tags_as("local-role", false), ["yother", "zz"]);
}

#[test]
#[serial]
fn a_local_role_named_like_a_tap_agent_reaches_the_others_not_that_agents_uses() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "yother", MANIFEST_HEADER);

	// As the tap agent `zz` has no `# Uses:`, it reaches nobody; as a local role it reaches the rest.
	assert!(reachable_tags_as("zz", true).is_empty());
	assert_eq!(reachable_tags_as("zz", false), ["yother"]);
}

#[test]
#[serial]
fn a_base_agent_reaches_the_variants_of_its_namespace_without_a_header() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "zz", "rust", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "zz", "go", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "other", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "other", "x", MANIFEST_HEADER);

	assert_eq!(reachable_tags("zz"), ["zz:go", "zz:rust"]);
}

#[test]
#[serial]
fn a_variant_reaches_nobody_and_its_uses_header_is_not_read() {
	let guard = ResolverDataDir::new();
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	install_default_tap_variant(guard.path(), "zz", "rust", "# Description: D\n# Uses: missing-agent\n");
	install_default_tap_variant(guard.path(), "zz", "go", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "other", MANIFEST_HEADER);

	assert!(reach_error("zz:rust").is_none(), "a variant's header is not validated");
	assert!(reachable_tags("zz:rust").is_empty(), "not even its siblings");
}

#[test]
#[serial]
fn a_uses_entry_names_a_namespace_or_one_variant() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: content:blog, seo:default\n");
	install_default_tap_manifest(guard.path(), "content", "# Description: Runs content\n");
	install_default_tap_variant(guard.path(), "content", "blog", "# Description: Writes blogs\n");
	install_default_tap_variant(guard.path(), "content", "news", "# Description: Writes news\n");
	install_default_tap_manifest(guard.path(), "seo", "# Description: Audits pages\n");

	assert_eq!(reachable_tags("mkt"), ["content:blog", "seo"]);
}

#[test]
#[serial]
fn uses_star_includes_the_variants_of_other_namespaces() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *\n");
	install_default_tap_manifest(guard.path(), "content", "# Description: Runs content\n");
	install_default_tap_variant(guard.path(), "content", "blog", "# Description: Writes blogs\n");
	install_default_tap_variant(guard.path(), "mkt", "email", "# Description: Writes email\n");

	assert_eq!(reachable_tags("mkt"), ["content", "content:blog", "mkt:email"]);
}

// --- `# Uses:` header ----------------------------------------------------------
fn install_uses_agent(data_dir: &std::path::Path, uses: &str) {
	install_default_tap_manifest(data_dir, "mkt", &format!("# Description: Runs mkt\n{uses}"));
}

#[test]
#[serial]
fn uses_reaches_the_named_agents_sorted() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: seo-audit, content\n");
	install_default_tap_manifest(guard.path(), "email", MANIFEST_HEADER);
	install_default_tap_manifest(guard.path(), "content", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "seo-audit", "# Description: Audits pages\n");

	assert_eq!(reachable_tags("mkt"), ["content", "seo-audit"]);
}

#[test]
#[serial]
fn uses_star_reaches_every_other_agent() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *\n");
	install_default_tap_manifest(guard.path(), "x-y", "# Description: Runs x-y\n");
	install_default_tap_manifest(guard.path(), "x", "# Description: Runs x\n");
	install_default_tap_manifest(guard.path(), "content", "# Description: Runs content\n");

	assert_eq!(reachable_tags("mkt"), ["content", "x", "x-y"]);
}

#[test]
#[serial]
fn uses_star_combined_with_exact_entries_has_no_duplicates() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: *, content, seo-audit\n");
	install_default_tap_manifest(guard.path(), "content", "# Description: Runs content\n");
	install_default_tap_manifest(guard.path(), "seo-audit", "# Description: Audits pages\n");

	assert_eq!(reachable_tags("mkt"), ["content", "seo-audit"]);
}

#[test]
#[serial]
fn uses_missing_agent_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: seo-audit\n");

	let err = reach_error("mkt").expect("bad header");
	assert_eq!(err, "`mkt` declares `# Uses: seo-audit`, but no such agent exists");
}

#[test]
#[serial]
fn uses_empty_entry_is_an_error_only_for_its_declaring_agent() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: content,,seo-audit\n");

	let err = reach_error("mkt").expect("bad header");
	assert_eq!(
		err,
		"`mkt` declares `# Uses: content, , seo-audit`, which has an empty entry"
	);
	// Another agent's prompt is unaffected by the malformed header.
	install_default_tap_manifest(guard.path(), "zz", MANIFEST_HEADER);
	assert!(reach_error("zz").is_none());
}

#[test]
#[serial]
fn uses_own_name_is_an_error() {
	let guard = ResolverDataDir::new();
	install_uses_agent(guard.path(), "# Uses: mkt\n");

	let err = reach_error("mkt").expect("bad header");
	assert_eq!(err, "`mkt` declares `# Uses: mkt`, which is its own name");
}
