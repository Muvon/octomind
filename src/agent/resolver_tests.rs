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
fn test_inject_role_name_overrides_first_role() {
	let manifest = "[[roles]]\nname = \"old\"\nsystem = \"do things\"\n";
	let injected = inject_role_name(manifest, "doctor:blood").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert_eq!(value["roles"][0]["name"].as_str(), Some("doctor:blood"));
	// Other fields survive the roundtrip
	assert_eq!(value["roles"][0]["system"].as_str(), Some("do things"));
}

#[test]
fn test_inject_role_name_without_roles_is_noop() {
	let injected = inject_role_name("version = 1\n", "tag").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert!(value.get("roles").is_none());
	assert_eq!(value["version"].as_integer(), Some(1));
}

#[test]
fn test_inject_role_name_invalid_toml_errors() {
	assert!(inject_role_name("not = = toml", "tag").is_err());
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
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
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
		"[[roles]]\nname = \"ztest:zz\"\nsystem = \"You are the {{ENV:OCTOMIND_RESOLVER_PROBE}} agent.\"\nwelcome = \"Hi.\"\ntemperature = 0.1\ntop_p = 0.9\ntop_k = 40\n\n[deps]\nrequire = [\"ztest/probe\"]\n",
	);

	let had_probe = std::env::var_os("OCTOMIND_RESOLVER_PROBE");
	std::env::set_var("OCTOMIND_RESOLVER_PROBE", "resolver");

	let statuses: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
	let (merged, role) = resolve_config_and_role(
		Some("ztest:zz"),
		&base_config(),
		Some(&|status| {
			statuses
				.lock()
				.expect("statuses lock")
				.push(status.to_string())
		}),
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
		guard
			.path()
			.join("agents")
			.join("ztest")
			.join("zz.toml")
			.exists(),
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
	config
		.taps
		.insert("ztest:zz".to_string(), "openai:gpt-4o".to_string());

	let (merged, role) = resolve_config_and_role(Some("ztest:zz"), &config, None)
		.await
		.expect("tag resolves");
	assert_eq!(role, "ztest:zz");
	assert_eq!(
		merged.model, "openai:gpt-4o",
		"[taps] model override must win for this tag"
	);
	assert_eq!(
		merged.model_profile.temperature,
		config.model_profile.temperature
	);
	assert_eq!(merged.model_profile.top_p, config.model_profile.top_p);
	assert_eq!(merged.model_profile.top_k, config.model_profile.top_k);
	assert_eq!(
		merged.model_profile.max_tokens,
		config.model_profile.max_tokens
	);
	assert_eq!(
		merged.model_profile.max_retries,
		config.model_profile.max_retries
	);
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
		err.to_string()
			.contains("must define at least one new [[roles]] entry"),
		"got: {err:#}"
	);
}
