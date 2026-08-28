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
