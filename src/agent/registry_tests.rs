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

//! Registry client tests: tag parsing, cache staleness, manifest fetch
//! against an isolated data dir, tap enumeration, and capability
//! resolution/merging. Complements the inline `meta_tests` (header-comment
//! parsing) and `resolver_tests.rs` (role-name injection).

use super::*;
use serial_test::serial;

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

/// The default tap's on-disk directory inside the current data dir.
/// Pre-created so `load_taps()`'s `ensure_default_tap()` takes the
/// already-cloned branch (git pull fails silently on a non-repo — no network).
fn default_tap_dir() -> PathBuf {
	let dir = crate::directories::get_octomind_data_dir()
		.expect("data dir")
		.join("taps")
		.join("muvon")
		.join("octomind-tap");
	fs::create_dir_all(&dir).expect("create default tap dir");
	dir
}

fn write_file(path: &Path, content: &str) {
	fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
	fs::write(path, content).expect("write file");
}

// ---------------------------------------------------------------------------
// Tag parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_tag_splits_category_variant_and_optional_version() {
	let (c, v, ver) = parse_tag("developer:general").expect("valid tag");
	assert_eq!(c, "developer");
	assert_eq!(v, "general");
	assert_eq!(ver, None);

	let (c, v, ver) = parse_tag("developer:general@1.2").expect("valid versioned tag");
	assert_eq!(c, "developer");
	assert_eq!(v, "general");
	assert_eq!(ver.as_deref(), Some("1.2"));
}

#[test]
fn parse_tag_rejects_invalid_agent_names() {
	let err = parse_tag("developer").expect_err("missing colon must fail");
	assert!(err.to_string().contains("expected 'category:variant'"));

	// Version split happens first — still needs category:variant afterwards.
	assert!(parse_tag("developer@1.0").is_err());
	assert!(parse_tag(":general").is_err());
	assert!(parse_tag("developer:").is_err());
	assert!(parse_tag("").is_err());
}

// ---------------------------------------------------------------------------
// Cache staleness + path layout
// ---------------------------------------------------------------------------

#[test]
fn is_stale_true_for_missing_file() {
	assert!(is_stale(
		&PathBuf::from("/nonexistent/registry-test.toml"),
		24
	));
}

#[test]
fn is_stale_false_for_fresh_file() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("manifest.toml");
	fs::write(&path, "x").expect("write");
	assert!(!is_stale(&path, 24));
}

#[test]
fn is_stale_true_when_older_than_ttl() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("manifest.toml");
	fs::write(&path, "x").expect("write");
	let file = std::fs::File::options()
		.write(true)
		.open(&path)
		.expect("open");
	file.set_modified(SystemTime::now() - Duration::from_secs(25 * 3600))
		.expect("backdate mtime");
	assert!(is_stale(&path, 24));
}

#[test]
#[serial]
fn cache_path_lives_under_agents_dir_and_creates_it() {
	let _guard = DataDirGuard::new();
	let path = cache_path("devtool", "helper").expect("cache path");
	let data = crate::directories::get_octomind_data_dir().expect("data dir");
	assert_eq!(
		path,
		data.join("agents").join("devtool").join("helper.toml")
	);
	assert!(path.parent().expect("parent").is_dir(), "cache dir created");
}

// ---------------------------------------------------------------------------
// Manifest fetch
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn fetch_manifest_reads_from_tap_and_populates_cache() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	let manifest =
		"# Title: Helper\n# Description: Helps.\n\n[[roles]]\nname = \"devtool:helper\"\n";
	write_file(
		&tap.join("agents").join("devtool").join("helper.toml"),
		manifest,
	);

	let (toml, root) = fetch_manifest("devtool:helper", &RegistryConfig::default())
		.await
		.expect("fetch");
	assert_eq!(toml, manifest);
	assert_eq!(root, tap, "tap root points at the providing tap");

	let cache = cache_path("devtool", "helper").expect("cache path");
	assert_eq!(fs::read_to_string(&cache).expect("read cache"), manifest);
}

#[tokio::test]
#[serial]
async fn fetch_manifest_serves_fresh_cache_without_tap_hit() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	default_tap_dir(); // exists but provides nothing

	let cache = cache_path("devtool", "helper").expect("cache path");
	fs::write(&cache, "CACHED-CONTENT").expect("seed cache");

	let (toml, _) = fetch_manifest("devtool:helper", &RegistryConfig::default())
		.await
		.expect("fresh cache serves without tap");
	assert_eq!(toml, "CACHED-CONTENT");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_errors_when_no_tap_provides_it() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	default_tap_dir(); // empty tap set, no cache

	let err = fetch_manifest("devtool:missing", &RegistryConfig::default())
		.await
		.expect_err("must fail");
	assert!(err.to_string().contains("Failed to fetch agent manifest"));
}

#[tokio::test]
#[serial]
async fn fetch_manifest_rejects_invalid_tag_before_any_io() {
	let _env = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let _guard = DataDirGuard::new();
	let err = fetch_manifest("nope", &RegistryConfig::default())
		.await
		.expect_err("must fail");
	assert!(err.to_string().contains("Invalid agent tag"));
}

// ---------------------------------------------------------------------------
// Tap enumeration
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn list_all_tap_agents_enumerates_sorted_and_skips_non_toml() {
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	write_file(
		&tap.join("agents").join("b-cat").join("zeta.toml"),
		"# Title: Zeta\n# Description: z\n",
	);
	write_file(
		&tap.join("agents").join("a-cat").join("alpha.toml"),
		"# Title: Alpha\n# Description: a\n",
	);
	write_file(
		&tap.join("agents").join("a-cat").join("notes.txt"),
		"not a manifest",
	);

	let agents = list_all_tap_agents().expect("list");
	let roles: Vec<&str> = agents.iter().map(|a| a.role.as_str()).collect();
	assert_eq!(roles, vec!["a-cat:alpha", "b-cat:zeta"]);
	assert_eq!(agents[0].meta.title, "Alpha");
	assert_eq!(agents[0].source_tap, crate::agent::taps::DEFAULT_TAP);
}

#[test]
#[serial]
fn list_all_tap_agents_fails_on_manifest_missing_headers() {
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	write_file(
		&tap.join("agents").join("x").join("bad.toml"),
		"# no headers\n",
	);
	assert!(list_all_tap_agents().is_err());
}

#[test]
#[serial]
fn list_all_tap_workflows_reads_descriptions_and_skips_invalid() {
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	write_file(
		&tap.join("workflows").join("deploy.toml"),
		"description = \"Deploy things\"\nsteps = []\n",
	);
	write_file(&tap.join("workflows").join("broken.toml"), "not = = toml");
	write_file(&tap.join("workflows").join("readme.md"), "ignore me");
	write_file(&tap.join("workflows").join("bare.toml"), "steps = []\n");

	let flows = list_all_tap_workflows().expect("list");
	let names: Vec<&str> = flows.iter().map(|w| w.name.as_str()).collect();
	assert_eq!(names, vec!["bare", "deploy"]);
	assert_eq!(flows[0].description, "");
	assert_eq!(flows[1].description, "Deploy things");
}

// ---------------------------------------------------------------------------
// Capability resolution
// ---------------------------------------------------------------------------

#[test]
fn cap_available_in_domain_empty_means_universal() {
	assert!(cap_available_in_domain(&[], "developer"));
	assert!(cap_available_in_domain(&[], "medical"));
}

#[test]
fn cap_available_in_domain_requires_exact_match() {
	let domains: Vec<String> = vec!["developer".to_string(), "devops".to_string()];
	assert!(cap_available_in_domain(&domains, "developer"));
	assert!(!cap_available_in_domain(&domains, "medical"));
	assert!(!cap_available_in_domain(&domains, "developer:general"));
}

#[test]
fn read_capability_config_requires_config_file() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let err = read_capability_config(tmp.path(), "cap").expect_err("must fail");
	assert!(err.to_string().contains("missing `config.toml`"));
}

#[test]
fn read_capability_config_requires_non_empty_triggers() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(&tmp.path().join("config.toml"), "triggers = []\n");
	let err = read_capability_config(tmp.path(), "cap").expect_err("must fail");
	assert!(err.to_string().contains("no `triggers"));
}

#[test]
fn read_capability_config_trims_and_drops_empty_entries() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(
		&tmp.path().join("config.toml"),
		"triggers = [\"  deploy  \", \"\", \"ship\"]\ndomains = [\" developer \", \"\"]\n",
	);
	let (triggers, domains) = read_capability_config(tmp.path(), "cap").expect("parse");
	assert_eq!(triggers, vec!["deploy".to_string(), "ship".to_string()]);
	assert_eq!(domains, vec!["developer".to_string()]);
}

#[test]
fn read_capability_config_domains_default_empty() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_file(&tmp.path().join("config.toml"), "triggers = [\"deploy\"]\n");
	let (triggers, domains) = read_capability_config(tmp.path(), "cap").expect("parse");
	assert_eq!(triggers, vec!["deploy".to_string()]);
	assert!(domains.is_empty());
}

#[test]
#[serial]
fn parse_capability_toml_errors_when_not_in_any_tap() {
	let _guard = DataDirGuard::new();
	default_tap_dir();
	let err = parse_capability_toml("no-such-cap", &HashMap::new()).expect_err("must fail");
	assert!(err.to_string().contains("not found"));
}

#[test]
#[serial]
fn parse_capability_toml_resolves_default_provider_and_fields() {
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	let cap_dir = tap.join("capabilities").join("deploy-helper");
	write_file(
		&cap_dir.join("config.toml"),
		"triggers = [\"deploy the app\"]\ndomains = [\"developer\"]\n",
	);
	write_file(
		&cap_dir.join("default.toml"),
		"[deps]\nrequire = [\"kubectl\"]\n\n[roles.mcp]\nserver_refs = [\"k8s\"]\nallowed_tools = [\"shell\"]\n\n[[mcp.servers]]\nname = \"deploy-srv\"\ntype = \"stdio\"\ncommand = \"deployer\"\nargs = []\ntimeout_seconds = 30\ntools = []\nenv = { TOKEN = \"{{ENV:DEPLOY_TOKEN}}\" }\n",
	);

	let cap = parse_capability_toml("deploy-helper", &HashMap::new()).expect("resolve");
	assert_eq!(cap.name, "deploy-helper");
	assert_eq!(cap.triggers, vec!["deploy the app".to_string()]);
	assert_eq!(cap.domains, vec!["developer".to_string()]);
	assert_eq!(cap.deps, vec!["kubectl".to_string()]);
	assert_eq!(cap.server_refs, vec!["k8s".to_string()]);
	assert_eq!(cap.allowed_tools, vec!["shell".to_string()]);
	assert_eq!(cap.mcp_servers.len(), 1);
	assert_eq!(cap.mcp_servers[0].name(), "deploy-srv");
	assert_eq!(cap.required_env_keys, vec!["DEPLOY_TOKEN".to_string()]);
	assert_eq!(cap.tap_root, tap);
}

#[test]
#[serial]
fn parse_capability_toml_provider_override_selects_file() {
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	let cap_dir = tap.join("capabilities").join("deploy-helper");
	write_file(&cap_dir.join("config.toml"), "triggers = [\"deploy\"]\n");
	write_file(
		&cap_dir.join("custom.toml"),
		"[deps]\nrequire = [\"helm\"]\n",
	);

	// Without an override only `default.toml` is a provider — not found.
	assert!(parse_capability_toml("deploy-helper", &HashMap::new()).is_err());

	let mut overrides = HashMap::new();
	overrides.insert("deploy-helper".to_string(), "custom".to_string());
	let cap = parse_capability_toml("deploy-helper", &overrides).expect("resolve via override");
	assert_eq!(cap.deps, vec!["helm".to_string()]);
}

#[test]
#[serial]
fn list_all_capabilities_lists_installed_sorted() {
	let _guard = DataDirGuard::new();
	let tap = default_tap_dir();
	for name in ["z-cap", "a-cap"] {
		let cap_dir = tap.join("capabilities").join(name);
		write_file(&cap_dir.join("config.toml"), "triggers = [\"go\"]\n");
		write_file(&cap_dir.join("default.toml"), "[deps]\nrequire = []\n");
	}

	let caps = list_all_capabilities(&HashMap::new()).expect("list");
	let names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
	assert_eq!(names, vec!["a-cap", "z-cap"]);
}

// ---------------------------------------------------------------------------
// Capability merge into agent manifests
// ---------------------------------------------------------------------------

#[test]
fn resolve_capabilities_passthrough_when_none_declared() {
	let raw = "[[roles]]\nname = \"x\"\n";
	let out =
		resolve_capabilities(raw, Path::new("/nonexistent-tap"), &HashMap::new()).expect("resolve");
	assert_eq!(out, raw);
}

#[test]
fn resolve_capabilities_errors_on_missing_capability_file() {
	let err = resolve_capabilities(
		"capabilities = [\"ghost\"]\n",
		Path::new("/nonexistent"),
		&HashMap::new(),
	)
	.expect_err("must fail");
	assert!(err.to_string().contains("Capability file not found"));
}

#[test]
fn resolve_capabilities_merges_and_strips_capabilities() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let cap_dir = tmp.path().join("capabilities").join("dep-cap");
	write_file(
		&cap_dir.join("default.toml"),
		"[deps]\nrequire = [\"kubectl\"]\n\n[roles.mcp]\nserver_refs = [\"k8s\"]\nallowed_tools = [\"shell\"]\n\n[[mcp.servers]]\nname = \"cap-srv\"\ncommand = \"anything\"\n",
	);

	let raw = "capabilities = [\"dep-cap\"]\n\n[[roles]]\nname = \"devtool:helper\"\n\n[roles.mcp]\nserver_refs = [\"existing\"]\nallowed_tools = []\n\n[[mcp.servers]]\nname = \"agent-srv\"\n\n[deps]\nrequire = [\"cargo\"]\n";
	let out = resolve_capabilities(raw, tmp.path(), &HashMap::new()).expect("resolve");
	let value: toml::Value = toml::from_str(&out).expect("output is valid toml");

	assert!(
		value.get("capabilities").is_none(),
		"capabilities key stripped"
	);
	let deps: Vec<&str> = value["deps"]["require"]
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|v| v.as_str())
		.collect();
	assert_eq!(deps, vec!["cargo", "kubectl"]);

	let role_mcp = &value["roles"][0]["mcp"];
	let refs: Vec<&str> = role_mcp["server_refs"]
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|v| v.as_str())
		.collect();
	assert_eq!(refs, vec!["existing", "k8s"]);
	let tools: Vec<&str> = role_mcp["allowed_tools"]
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|v| v.as_str())
		.collect();
	assert_eq!(tools, vec!["shell"]);

	let servers: Vec<&str> = value["mcp"]["servers"]
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|s| s.get("name").and_then(|n| n.as_str()))
		.collect();
	assert_eq!(servers, vec!["agent-srv", "cap-srv"]);
}

#[test]
fn resolve_capabilities_dedupes_servers_by_name() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let cap_dir = tmp.path().join("capabilities").join("dep-cap");
	write_file(
		&cap_dir.join("default.toml"),
		"[[mcp.servers]]\nname = \"agent-srv\"\n",
	);

	let raw = "capabilities = [\"dep-cap\"]\n\n[[mcp.servers]]\nname = \"agent-srv\"\n";
	let out = resolve_capabilities(raw, tmp.path(), &HashMap::new()).expect("resolve");
	let value: toml::Value = toml::from_str(&out).expect("output is valid toml");
	let servers = value["mcp"]["servers"].as_array().expect("servers");
	assert_eq!(
		servers.len(),
		1,
		"same-name capability server not duplicated"
	);
}

#[test]
fn merge_string_array_dedupes_and_appends() {
	let mut table = toml::map::Map::new();
	table.insert(
		"key".to_string(),
		toml::Value::Array(vec!["a".into(), "b".into()]),
	);
	merge_string_array(&mut table, "key", &["b".to_string(), "c".to_string()]);
	let items: Vec<&str> = table
		.get("key")
		.and_then(|v| v.as_array())
		.expect("key present")
		.iter()
		.filter_map(|v| v.as_str())
		.collect();
	assert_eq!(items, vec!["a", "b", "c"]);
}

#[test]
fn merge_string_array_creates_missing_key() {
	let mut table = toml::map::Map::new();
	merge_string_array(&mut table, "fresh", &["x".to_string()]);
	let items: Vec<&str> = table
		.get("fresh")
		.and_then(|v| v.as_array())
		.expect("fresh present")
		.iter()
		.filter_map(|v| v.as_str())
		.collect();
	assert_eq!(items, vec!["x"]);
}

// --- fetch_manifest cache & multi-tap precedence ----------------------------

/// A local tap directory layout: `<data>/taps/<user>/octomind-<repo>`.
fn user_tap_dir(data_dir: &Path, name: &str) -> PathBuf {
	let (user, repo) = name.split_once('/').expect("tap name is user/repo");
	data_dir.join("taps").join(user).join(format!("octomind-{repo}"))
}

/// Write a taps.toml listing user taps (local_path kept so load_taps never
/// attempts git operations on them).
fn install_taps_file(data_dir: &Path, taps: &[(&str, Option<&Path>)]) {
	let mut content = String::new();
	for (name, local_path) in taps {
		content.push_str(&format!("[[taps]]\nname = \"{name}\"\n"));
		if let Some(path) = local_path {
			content.push_str(&format!("local_path = \"{}\"\n", path.display()));
		}
	}
	std::fs::write(data_dir.join("taps.toml"), content).expect("write taps.toml");
}

fn write_agent_manifest(tap_root: &Path, category: &str, variant: &str, body: &str) {
	let dir = tap_root.join("agents").join(category);
	std::fs::create_dir_all(&dir).expect("create category dir");
	std::fs::write(dir.join(format!("{variant}.toml")), body).expect("write manifest");
}

fn registry_config() -> crate::config::RegistryConfig {
	crate::config::RegistryConfig::default()
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) {
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
		.expect("set permissions");
}

/// Make a file older than any plausible cache TTL so `is_stale` flips true.
#[cfg(unix)]
fn age_beyond_ttl(path: &Path) {
	let file = std::fs::File::options()
		.write(true)
		.open(path)
		.expect("open cache file for mtime update");
	file.set_times(
		std::fs::FileTimes::new()
			.set_accessed(std::time::UNIX_EPOCH)
			.set_modified(std::time::UNIX_EPOCH),
	)
	.expect("set mtime beyond ttl");
}

#[tokio::test]
#[serial]
async fn fetch_manifest_errors_when_cache_dir_cannot_be_created() {
	// The data dir exists, but `agents` inside it is a file, so creating the
	// cache dir underneath fails after the data dir itself resolves fine.
	let tmp = tempfile::tempdir().expect("tempdir");
	std::fs::write(tmp.path().join("agents"), "not a directory").expect("write blocker");
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	std::env::set_var("OCTOMIND_DATA_DIR", tmp.path());

	let err = fetch_manifest("cat:var", &registry_config())
		.await
		.expect_err("unwritable data dir must fail");
	assert!(
		err.to_string().contains("Failed to create agent cache dir"),
		"got: {err:#}"
	);

	match previous {
		Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
		None => std::env::remove_var("OCTOMIND_DATA_DIR"),
	}
}

#[tokio::test]
#[serial]
async fn fetch_manifest_first_tap_wins_on_duplicate_manifests() {
	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	default_tap_dir(); // load_taps() requires the default tap dir to exist
	let first = user_tap_dir(&data_dir, "probe/first");
	let second = user_tap_dir(&data_dir, "probe/second");
	write_agent_manifest(&first, "dup", "var", "# first tap manifest\n");
	write_agent_manifest(&second, "dup", "var", "# second tap manifest\n");
	install_taps_file(
		&data_dir,
		&[("probe/first", Some(&first)), ("probe/second", Some(&second))],
	);

	let (raw, tap_root) = fetch_manifest("dup:var", &registry_config())
		.await
		.expect("duplicate manifests resolve");
	assert_eq!(raw, "# first tap manifest\n", "first tap wins");
	assert_eq!(tap_root, first, "tap root is the first providing tap");
}

#[tokio::test]
#[serial]
#[cfg(unix)]
async fn fetch_manifest_errors_on_unreadable_fresh_cache() {
	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	let cache = data_dir.join("agents").join("locked");
	std::fs::create_dir_all(&cache).expect("create cache dir");
	let cache_file = cache.join("var.toml");
	std::fs::write(&cache_file, "cached manifest").expect("write cache");
	chmod(&cache_file, 0o000);

	let err = fetch_manifest("locked:var", &registry_config())
		.await
		.expect_err("unreadable cache must fail");
	assert!(
		err.to_string().contains("Failed to read cached manifest"),
		"got: {err:#}"
	);
	chmod(&cache_file, 0o644);
}

#[tokio::test]
#[serial]
#[cfg(unix)]
async fn fetch_manifest_serves_stale_cache_and_refreshes_in_background() {
	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	let default_tap = default_tap_dir();
	write_agent_manifest(&default_tap, "stale", "var", "# fresh tap manifest\n");

	let cache_dir = data_dir.join("agents").join("stale");
	std::fs::create_dir_all(&cache_dir).expect("create cache dir");
	let cache_file = cache_dir.join("var.toml");
	std::fs::write(&cache_file, "# stale cached manifest\n").expect("write stale cache");
	age_beyond_ttl(&cache_file);

	// The stale cache is served synchronously…
	let (raw, _) = fetch_manifest("stale:var", &registry_config())
		.await
		.expect("stale cache still resolves");
	assert_eq!(raw, "# stale cached manifest\n");

	// …while the background refresh eventually replaces it from the tap.
	let refreshed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
		loop {
			if let Ok(content) = std::fs::read_to_string(&cache_file) {
				if content == "# fresh tap manifest\n" {
					return;
				}
			}
			tokio::time::sleep(std::time::Duration::from_millis(25)).await;
		}
	})
	.await;
	assert!(
		refreshed.is_ok(),
		"background refresh did not rewrite the cache"
	);
}

// --- list_all_tap_agents / list_all_tap_workflows edge branches -------------

#[test]
#[serial]
#[cfg(unix)]
fn list_all_tap_agents_skips_broken_unreadable_and_duplicate_entries() {
	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	let default_tap = default_tap_dir();

	// A user tap providing a duplicate role — first tap wins.
	let user_tap = user_tap_dir(&data_dir, "probe/dup");
	write_agent_manifest(
		&user_tap,
		"dup",
		"both",
		"# Title: Dup\n# Description: From the user tap.\n",
	);
	// A second user tap whose directory does not exist (agents_dir() errors).
	install_taps_file(&data_dir, &[("probe/dup", Some(&user_tap)), ("probe/ghost", None)]);

	// Default tap: one good agent, a duplicate of the user tap's role, a stray
	// non-directory entry, an unreadable category, and a directory named *.toml.
	write_agent_manifest(
		&default_tap,
		"dup",
		"both",
		"# Title: Dup\n# Description: From the default tap.\n",
	);
	write_agent_manifest(
		&default_tap,
		"good",
		"var",
		"# Title: Good\n# Description: The good agent.\n",
	);
	std::fs::write(default_tap.join("agents").join("stray.md"), "not a category")
		.expect("write stray file");
	let locked = default_tap.join("agents").join("locked");
	std::fs::create_dir_all(&locked).expect("create locked category");
	chmod(&locked, 0o000);
	let toml_dir = default_tap.join("agents").join("good").join("dirnamed.toml");
	std::fs::create_dir_all(&toml_dir).expect("create dir named like a manifest");

	let agents = list_all_tap_agents().expect("enumeration succeeds");
	let roles: Vec<&str> = agents.iter().map(|a| a.role.as_str()).collect();
	assert_eq!(roles, vec!["dup:both", "good:var"], "skipped entries");
	let dup = agents.iter().find(|a| a.role == "dup:both").expect("dup listed");
	assert_eq!(
		dup.source_tap, "probe/dup",
		"first tap wins over the default tap"
	);

	chmod(&locked, 0o755);
}

#[test]
#[serial]
#[cfg(unix)]
fn list_all_tap_workflows_skips_unreadable_and_non_utf8_entries() {
	use std::os::unix::ffi::OsStringExt;

	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	let default_tap = default_tap_dir();
	let workflows = default_tap.join("workflows");
	std::fs::create_dir_all(&workflows).expect("create workflows dir");
	std::fs::write(
		workflows.join("good.toml"),
		"description = \"the good workflow\"\n",
	)
	.expect("write good workflow");

	// A user tap whose workflows dir cannot be read is skipped entirely.
	let user_tap = user_tap_dir(&data_dir, "probe/locked");
	let locked_workflows = user_tap.join("workflows");
	std::fs::create_dir_all(&locked_workflows).expect("create locked workflows");
	std::fs::write(locked_workflows.join("hidden.toml"), "description = \"x\"\n")
		.expect("write hidden workflow");
	chmod(&locked_workflows, 0o000);
	install_taps_file(&data_dir, &[("probe/locked", Some(&user_tap))]);

	// Non-UTF-8 file stem and an unreadable workflow file in the default tap.
	let non_utf8 = workflows.join(std::ffi::OsString::from_vec(vec![0xff, 0xfe, b'.', b't', b'o', b'm', b'l']));
	std::fs::write(&non_utf8, "description = \"x\"\n").expect("write non-utf8 workflow");
	let unreadable = workflows.join("unreadable.toml");
	std::fs::write(&unreadable, "description = \"x\"\n").expect("write unreadable workflow");
	chmod(&unreadable, 0o000);

	let workflows_list = list_all_tap_workflows().expect("enumeration succeeds");
	let names: Vec<&str> = workflows_list.iter().map(|w| w.name.as_str()).collect();
	assert_eq!(names, vec!["good"], "only the readable UTF-8 workflow is listed");

	chmod(&locked_workflows, 0o755);
	chmod(&unreadable, 0o644);
}

// --- capability resolution edge branches ------------------------------------

#[test]
#[serial]
fn parse_capability_toml_skips_taps_with_missing_directories() {
	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	let default_tap = default_tap_dir();
	let cap_dir = default_tap.join("capabilities").join("probe-cap");
	std::fs::create_dir_all(&cap_dir).expect("create capability dir");
	std::fs::write(
		cap_dir.join("config.toml"),
		"triggers = [\"probe thing\"]\n",
	)
	.expect("write config");
	std::fs::write(cap_dir.join("default.toml"), "[deps]\n").expect("write provider");

	// A user tap whose directory does not exist must be skipped, not fatal.
	install_taps_file(&data_dir, &[("probe/ghost", None)]);

	let resolved =
		parse_capability_toml("probe-cap", &HashMap::new()).expect("capability resolves");
	assert_eq!(resolved.name, "probe-cap");
	assert_eq!(resolved.triggers, vec!["probe thing".to_string()]);
}

#[test]
#[serial]
#[cfg(unix)]
fn parse_capability_toml_surfaces_provider_file_failures() {
	let _guard = DataDirGuard::new();
	let default_tap = default_tap_dir();

	let unreadable = default_tap.join("capabilities").join("unreadable-cap");
	std::fs::create_dir_all(&unreadable).expect("create capability dir");
	std::fs::write(unreadable.join("config.toml"), "triggers = [\"t\"]\n").expect("config");
	std::fs::write(unreadable.join("default.toml"), "[deps]\n").expect("provider");
	chmod(&unreadable.join("default.toml"), 0o000);
	let err = parse_capability_toml("unreadable-cap", &HashMap::new())
		.expect_err("unreadable provider must fail");
	assert!(
		err.to_string().contains("Failed to read provider file"),
		"got: {err:#}"
	);
	chmod(&unreadable.join("default.toml"), 0o644);

	let malformed = default_tap.join("capabilities").join("malformed-cap");
	std::fs::create_dir_all(&malformed).expect("create capability dir");
	std::fs::write(malformed.join("config.toml"), "triggers = [\"t\"]\n").expect("config");
	std::fs::write(malformed.join("default.toml"), "not = = toml").expect("provider");
	let err = parse_capability_toml("malformed-cap", &HashMap::new())
		.expect_err("malformed provider must fail");
	assert!(
		err.to_string().contains("Failed to parse provider file"),
		"got: {err:#}"
	);
}

#[test]
#[serial]
fn parse_capability_toml_collects_env_keys_and_skips_malformed_servers() {
	let _guard = DataDirGuard::new();
	let default_tap = default_tap_dir();
	let cap_dir = default_tap.join("capabilities").join("wired-cap");
	std::fs::create_dir_all(&cap_dir).expect("create capability dir");
	std::fs::write(cap_dir.join("config.toml"), "triggers = [\"t\"]\n").expect("config");
	std::fs::write(
		cap_dir.join("default.toml"),
		r#"
[[mcp.servers]]
name = "stdio-srv"
type = "stdio"
command = "probe-server"
args = []
timeout_seconds = 5
tools = []
env = { TOKEN = "{{ENV:STDIN_TOKEN}}" }

[[mcp.servers]]
name = "http-srv"
type = "http"
url = "https://probe.test"
timeout_seconds = 5
tools = []
headers = { Authorization = "{{ENV:HDR_TOKEN}}" }

[[mcp.servers]]
type = "http"
"#,
	)
	.expect("write provider");

	let resolved =
		parse_capability_toml("wired-cap", &HashMap::new()).expect("capability resolves");
	let mut keys = resolved.required_env_keys.clone();
	keys.sort();
	assert_eq!(
		keys,
		vec!["HDR_TOKEN".to_string(), "STDIN_TOKEN".to_string()],
		"env and header placeholders both gate activation"
	);
	let server_names: Vec<&str> = resolved
		.mcp_servers
		.iter()
		.map(|s| s.name())
		.collect();
	assert_eq!(
		server_names,
		vec!["stdio-srv", "http-srv"],
		"malformed server block is skipped, not fatal"
	);
}

#[test]
#[serial]
#[cfg(unix)]
fn list_all_capabilities_skips_broken_unreadable_and_duplicate_entries() {
	let _guard = DataDirGuard::new();
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	let default_tap = default_tap_dir();

	// A user tap providing a duplicate capability name — first tap wins.
	let user_tap = user_tap_dir(&data_dir, "probe/dup");
	let dup_dir = user_tap.join("capabilities").join("dup-cap");
	std::fs::create_dir_all(&dup_dir).expect("create dup capability");
	std::fs::write(dup_dir.join("config.toml"), "triggers = [\"t\"]\n").expect("config");
	std::fs::write(dup_dir.join("default.toml"), "[deps]\n").expect("provider");
	install_taps_file(&data_dir, &[("probe/dup", Some(&user_tap)), ("probe/ghost", None)]);

	// Default tap: duplicate of the user capability, one good capability, a
	// stray file, and an unreadable capability directory.
	let dup_default = default_tap.join("capabilities").join("dup-cap");
	std::fs::create_dir_all(&dup_default).expect("create dup capability");
	std::fs::write(dup_default.join("config.toml"), "triggers = [\"t\"]\n").expect("config");
	std::fs::write(dup_default.join("default.toml"), "[deps]\n").expect("provider");

	let good = default_tap.join("capabilities").join("good-cap");
	std::fs::create_dir_all(&good).expect("create good capability");
	std::fs::write(good.join("config.toml"), "triggers = [\"good thing\"]\n").expect("config");
	std::fs::write(good.join("default.toml"), "[deps]\n").expect("provider");

	std::fs::write(
		default_tap.join("capabilities").join("stray.md"),
		"not a capability",
	)
	.expect("write stray file");
	let locked = default_tap.join("capabilities").join("locked-cap");
	std::fs::create_dir_all(&locked).expect("create locked capability");
	chmod(&locked, 0o000);

	let caps = list_all_capabilities(&HashMap::new()).expect("enumeration succeeds");
	let mut names: Vec<&str> = caps.iter().map(|c| c.name.as_str()).collect();
	names.sort();
	assert_eq!(names, vec!["dup-cap", "good-cap"], "skipped entries");

	chmod(&locked, 0o755);
}
