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

//! Tests for the `capability` tool command surface: parameter validation,
//! unknown-name handling, and the idempotent disable path. Only the
//! deterministic arms — nothing here depends on which taps happen to be
//! installed on the machine (list is asserted as "answers", not contents).

use super::*;
use serial_test::serial;

fn cap_call(params: serde_json::Value) -> McpToolCall {
	McpToolCall {
		tool_name: "capability".to_string(),
		parameters: params,
		tool_id: "t-cap".to_string(),
	}
}

fn test_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
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

#[tokio::test]
#[serial]
async fn test_capability_action_validation() {
	let config = test_config();

	let result = execute_capability_command(&cap_call(serde_json::json!({})), &config)
		.await
		.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("action"));

	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "explode"})), &config)
			.await
			.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Unknown action"));
}

#[tokio::test]
#[serial]
async fn test_capability_enable_unknown_and_disable_idempotent() {
	let config = test_config();

	// enable without a name
	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "enable"})), &config)
			.await
			.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("name"));

	// enable a capability no tap provides
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "__captest_nonexistent"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("not found"));

	// disable of an inactive capability is an idempotent success
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "__captest_nonexistent"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
	assert!(text_of(&result).contains("not active"));
}

#[tokio::test]
#[serial]
async fn test_capability_list_answers() {
	let config = test_config();
	// Contents depend on installed taps — only the contract matters: the
	// command answers rather than erroring or hanging.
	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "list"})), &config)
			.await
			.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
	assert!(!text_of(&result).is_empty());
}

#[tokio::test]
#[serial]
async fn test_capability_discover_arms() {
	let config = test_config();

	// discover without intent → validation error
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "discover"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("intent"));

	// With an intent it must answer (match set depends on installed taps)
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "discover", "intent": "review some rust code"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "got: {}", text_of(&result));
}

#[tokio::test]
#[serial]
async fn test_load_env_capabilities_reports_failures_per_name() {
	let config = test_config();

	// Unset: early return, no events
	std::env::remove_var("OCTOMIND_CAPABILITIES");
	let events = std::sync::Mutex::new(Vec::new());
	let cb = |e: EnvCapabilityProgress| {
		events.lock().unwrap().push(e);
	};
	load_env_capabilities(&config, Some(&cb)).await;
	assert!(events.lock().unwrap().is_empty());

	// Two bogus names (plus junk whitespace): a Starting event with both,
	// then a failed Completed per name — never an abort.
	std::env::set_var(
		"OCTOMIND_CAPABILITIES",
		"__envcap_nonexistent, ,__envcap_other",
	);
	load_env_capabilities(&config, Some(&cb)).await;
	std::env::remove_var("OCTOMIND_CAPABILITIES");

	let events = events.into_inner().unwrap();
	let mut starting_names = Vec::new();
	let mut completions = Vec::new();
	for e in events {
		match e {
			EnvCapabilityProgress::Starting { capabilities } => starting_names = capabilities,
			EnvCapabilityProgress::Completed {
				capability,
				success,
			} => completions.push((capability, success)),
		}
	}
	assert_eq!(
		starting_names,
		vec![
			"__envcap_nonexistent".to_string(),
			"__envcap_other".to_string()
		]
	);
	assert_eq!(completions.len(), 2, "{completions:?}");
	assert!(
		completions.iter().all(|(_, success)| !success),
		"bogus capabilities must fail: {completions:?}"
	);
}

// ---------------------------------------------------------------------------
// Sandboxed tap fixtures — OCTOMIND_DATA_DIR points at a temp dir carrying
// a local tap, so enable/disable/list run against fixture capabilities only,
// never against whatever taps happen to be installed on this machine.
// ---------------------------------------------------------------------------

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
	fn new(keys: &[&'static str]) -> Self {
		Self(keys.iter().map(|k| (*k, std::env::var_os(k))).collect())
	}
}

impl Drop for EnvGuard {
	fn drop(&mut self) {
		for (key, saved) in &self.0 {
			match saved {
				Some(v) => std::env::set_var(key, v),
				None => std::env::remove_var(key),
			}
		}
	}
}

struct CapSandbox {
	_env: EnvGuard,
	dir: std::path::PathBuf,
}

impl CapSandbox {
	fn new(tag: &str) -> Self {
		let env = EnvGuard::new(&["OCTOMIND_DATA_DIR"]);
		let dir = std::env::temp_dir().join(format!("octomind-cap-{tag}-{}", std::process::id()));
		if dir.exists() {
			std::fs::remove_dir_all(&dir).expect("clear stale sandbox");
		}
		std::fs::create_dir_all(&dir).expect("create sandbox");
		std::env::set_var("OCTOMIND_DATA_DIR", &dir);
		let tap_root = dir.join("taps").join("captest").join("octomind-tap");
		std::fs::create_dir_all(tap_root.join("capabilities")).expect("tap capabilities dir");
		std::fs::write(
			dir.join("taps.toml"),
			format!(
				"[[taps]]\nname = \"captest/tap\"\nlocal_path = {}\n",
				toml::Value::String(tap_root.to_string_lossy().into_owned())
			),
		)
		.expect("write taps.toml");
		Self { _env: env, dir }
	}

	/// Install a fixture capability: `config.toml` (triggers/domains) plus a
	/// `default.toml` provider body (servers/deps/allowed_tools).
	fn cap(&self, name: &str, config_toml: &str, provider_toml: &str) {
		let cap_dir = self
			.dir
			.join("taps")
			.join("captest")
			.join("octomind-tap")
			.join("capabilities")
			.join(name);
		std::fs::create_dir_all(&cap_dir).expect("cap dir");
		std::fs::write(cap_dir.join("config.toml"), config_toml).expect("config.toml");
		std::fs::write(cap_dir.join("default.toml"), provider_toml).expect("provider.toml");
	}

	fn tap_root(&self) -> std::path::PathBuf {
		self.dir.join("taps").join("captest").join("octomind-tap")
	}
}

impl Drop for CapSandbox {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.dir);
	}
}

/// Fixture set. `captest-static`'s server must be pushed into the role config
/// by the test (static-server branch). `captest-dynamic` uses a builtin server
/// NOT in the config, so its enable fails deterministically — builtin servers
/// are rejected by `get_server_functions` without any connection attempt.
/// `captest-envgate`'s stdio command points at a nonexistent binary, so once
/// the env gate passes, enable still fails fast (spawn error, no network).
fn install_fixture_caps(sb: &CapSandbox) {
	sb.cap(
		"captest-static",
		"triggers = [\"use the static cap\", \"second trigger\", \"third trigger\", \"fourth trigger\"]\n",
		"[[mcp.servers]]\nname = \"captest-static-srv\"\ntype = \"builtin\"\ntimeout_seconds = 30\ntools = []\n\n[roles.mcp]\nallowed_tools = [\"captest-static-srv:tool_alpha\"]\n",
	);
	sb.cap(
		"captest-dynamic",
		"triggers = [\"use the dynamic cap\"]\n",
		"[[mcp.servers]]\nname = \"captest-dyn-srv\"\ntype = \"builtin\"\ntimeout_seconds = 30\ntools = []\n",
	);
	sb.cap(
		"captest-envgate",
		"triggers = [\"use the env cap\"]\n",
		"[[mcp.servers]]\nname = \"captest-env-srv\"\ntype = \"stdio\"\ncommand = \"captest-no-such-binary\"\nargs = []\ntimeout_seconds = 5\ntools = []\nenv = { API_KEY = \"{{ENV:CAPTEST_MISSING_KEY}}\" }\n",
	);
	sb.cap(
		"captest-domain",
		"triggers = [\"use the domain cap\"]\ndomains = [\"medical\"]\n",
		"[[mcp.servers]]\nname = \"captest-domain-srv\"\ntype = \"builtin\"\ntimeout_seconds = 30\ntools = []\n",
	);
	sb.cap("captest-empty", "triggers = [\"use the empty cap\"]\n", "");
	sb.cap(
		"captest-deps-ok",
		"triggers = [\"install the deps cap\"]\n",
		"[deps]\nrequire = [\"captest/tool\"]\n",
	);
	sb.cap(
		"captest-deps-missing",
		"triggers = [\"install the missing deps cap\"]\n",
		"[deps]\nrequire = [\"ghost/tool\"]\n",
	);
	let deps_dir = sb.tap_root().join("deps").join("captest");
	std::fs::create_dir_all(&deps_dir).expect("deps dir");
	std::fs::write(deps_dir.join("tool.sh"), "#!/bin/sh\nexit 0\n").expect("dep script");
}

fn seed_cap(name: &str, server: &str, tools: &[&str], age_secs: u64) {
	registry().write().unwrap().insert(
		name.to_string(),
		CapState {
			server_tools: vec![(
				server.to_string(),
				tools.iter().map(|t| t.to_string()).collect(),
			)],
			last_used: std::time::Instant::now() - std::time::Duration::from_secs(age_secs),
		},
	);
}

fn clear_seeded_caps(names: &[&str]) {
	let mut reg = registry().write().unwrap();
	for n in names {
		reg.remove(*n);
	}
}

/// The active-capability registry is process-global; a test that seeds or
/// activates caps must start from a clean slate or it inherits entries
/// leaked by whichever test ran before it (a failed assert skips cleanup).
/// Only OUR `captest-*` entries are dropped: other test modules share this
/// registry WITHOUT #[serial], so a full clear would race their entries.
fn reset_registry() {
	registry()
		.write()
		.unwrap()
		.retain(|name, _| !name.starts_with("captest-"));
}

/// `active_count()` counts foreign entries too: other test modules share this
/// registry WITHOUT #[serial] (see `reset_registry`), so a concurrent test can
/// add an entry mid-flight. Count only our `captest-*` namespace.
fn captest_active_count() -> usize {
	registry()
		.read()
		.unwrap()
		.keys()
		.filter(|name| name.starts_with("captest-"))
		.count()
}

#[tokio::test]
#[serial]
async fn test_capability_enable_static_server_extends_overlay() {
	let sb = CapSandbox::new("static");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let mut config = test_config();
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::builtin(
			"captest-static-srv",
			30,
			vec![],
		));

	// The enable path overlays capability tools onto the INITIALIZED tool
	// map; in a filtered run nothing initializes it first, so do it here.
	crate::mcp::tool_map::initialize_tool_map(&config)
		.await
		.expect("init tool map");

	// Enable: static branch registers the cap's bare tool names in the global
	// tool map and records an overlay entry.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-static"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "enable failed: {}", text_of(&result));
	let msg = text_of(&result);
	assert!(
		msg.contains("Activated 1 server(s): captest-static-srv"),
		"got: {msg}"
	);
	assert!(msg.contains("Tools available: tool_alpha"), "got: {msg}");
	assert!(is_active("captest-static"));
	assert!(crate::mcp::tool_map::get_server_for_tool("tool_alpha").is_some());

	// Second enable is an idempotent no-op.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-static"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(text_of(&result).contains("already active"));

	// Disable: the server is static-owned, so it is stripped (tool map +
	// overlay) but NOT shut down — refcount-style protection for role servers.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "captest-static"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "disable failed: {}", text_of(&result));
	assert!(text_of(&result).contains("Fully shut down 0 server(s)"));
	assert!(!is_active("captest-static"));
	assert!(crate::mcp::tool_map::get_server_for_tool("tool_alpha").is_none());

	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_enable_dynamic_builtin_fails_cleanly() {
	let sb = CapSandbox::new("dynamic");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let config = test_config();

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-dynamic"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(
		is_err(&result),
		"builtin dynamic enable must fail: {}",
		text_of(&result)
	);
	assert!(
		text_of(&result).contains("Failed to enable server 'captest-dyn-srv'"),
		"got: {}",
		text_of(&result)
	);
	assert!(!is_active("captest-dynamic"));

	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_enable_deps_paths() {
	let sb = CapSandbox::new("deps");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let config = test_config();

	// Deps-only cap with a working installer script: activation IS the install.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-deps-ok"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "deps enable failed: {}", text_of(&result));
	assert!(text_of(&result).contains("Installed deps: captest/tool"));
	assert!(is_active("captest-deps-ok"));

	// Deps-only cap pointing at a missing script: structured error, not active.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-deps-missing"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Failed to install deps"));
	assert!(!is_active("captest-deps-missing"));

	clear_seeded_caps(&["captest-deps-ok"]);
	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_env_gate_blocks_then_passes() {
	let sb = CapSandbox::new("envgate");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let config = test_config();
	let env = EnvGuard::new(&["CAPTEST_MISSING_KEY"]);
	std::env::remove_var("CAPTEST_MISSING_KEY");

	// Gate closed: activation refused before any server work.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-envgate"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(
		text_of(&result).contains("requires env vars: CAPTEST_MISSING_KEY"),
		"got: {}",
		text_of(&result)
	);
	assert!(!is_active("captest-envgate"));

	// Gate open: activation proceeds past the gate (and then fails on the
	// unconnectable server — a DIFFERENT error, proving the gate passed).
	std::env::set_var("CAPTEST_MISSING_KEY", "set-for-test");
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-envgate"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(
		text_of(&result).contains("Failed to enable server 'captest-env-srv'"),
		"got: {}",
		text_of(&result)
	);
	drop(env);

	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_domain_gate_blocks_foreign_domain() {
	let sb = CapSandbox::new("domain");
	install_fixture_caps(&sb);
	reset_registry();
	let config = test_config();

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-domain"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(
		text_of(&result).contains("bound to domains"),
		"got: {}",
		text_of(&result)
	);
	assert!(!is_active("captest-domain"));
}

#[tokio::test]
#[serial]
async fn test_capability_empty_cap_is_an_error() {
	let sb = CapSandbox::new("empty");
	install_fixture_caps(&sb);
	reset_registry();
	let config = test_config();

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-empty"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("nothing to activate"));
}

#[tokio::test]
#[serial]
async fn test_capability_disable_shared_server_refcounting() {
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let config = test_config();
	// Register the shared server in the dynamic registry so the kill path
	// finds it (a builtin config passes register validation without spawning).
	crate::mcp::runtime::dynamic::register_server(crate::config::McpServerConfig::builtin(
		"captest-shared-srv",
		30,
		vec![],
	))
	.expect("register shared server");

	// Two caps own disjoint tool sets on the same server.
	seed_cap("captest-share-a", "captest-shared-srv", &["tool_a"], 100);
	seed_cap("captest-share-b", "captest-shared-srv", &["tool_b"], 50);

	// Disable A: B still references the server → strip-only, no shutdown.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "captest-share-a"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "disable a failed: {}", text_of(&result));
	assert!(text_of(&result).contains("Fully shut down 0 server(s)"));
	assert!(!is_active("captest-share-a"));
	assert!(is_active("captest-share-b"));

	// Disable B: last reference, not static-owned → full shutdown.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "captest-share-b"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "disable b failed: {}", text_of(&result));
	assert!(text_of(&result).contains("Fully shut down 1 server(s): captest-shared-srv"));
	assert!(!is_active("captest-share-b"));
	// The server stays registered (disabled) — removal is `remove`, not disable.
	assert!(crate::mcp::runtime::dynamic::is_dynamic(
		"captest-shared-srv"
	));
	assert!(crate::mcp::runtime::dynamic::list_servers()
		.iter()
		.any(|(n, _, e)| n == "captest-shared-srv" && !e));

	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_disable_failure_reinserts_cap_for_retry() {
	// A mid-plan disable failure must re-insert the original cap state so
	// the user can retry; partially stripped servers are restored by
	// retrying enable (enable re-applies overlay + tools).
	let config = test_config();
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	seed_cap("captest-ghost", "captest-ghost-srv", &["tool_g"], 1);

	// The cap's server is not registered → the plan fails mid-loop.
	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "captest-ghost"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(is_err(&result));
	assert!(text_of(&result).contains("Failed to disable server 'captest-ghost-srv'"));
	assert!(
		is_active("captest-ghost"),
		"failed disable must keep the cap active for retry"
	);

	// Register the server so the retry takes the success path end-to-end.
	crate::mcp::runtime::dynamic::register_server(crate::config::McpServerConfig::builtin(
		"captest-ghost-srv",
		30,
		vec![],
	))
	.expect("register ghost server");

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "disable", "name": "captest-ghost"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(
		!is_err(&result),
		"retry disable failed: {}",
		text_of(&result)
	);
	assert!(!is_active("captest-ghost"));

	clear_seeded_caps(&["captest-ghost"]);
	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_lru_eviction_on_enable() {
	let sb = CapSandbox::new("lru");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let mut config = test_config();
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::builtin(
			"captest-static-srv",
			30,
			vec![],
		));

	// Fill the registry to the soft cap with staggered ages.
	seed_cap("captest-lru-a", "captest-lru-a-srv", &["t_a"], 400);
	seed_cap("captest-lru-b", "captest-lru-b-srv", &["t_b"], 300);
	seed_cap("captest-lru-c", "captest-lru-c-srv", &["t_c"], 200);
	seed_cap("captest-lru-d", "captest-lru-d-srv", &["t_d"], 100);

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-static"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "enable failed: {}", text_of(&result));

	// The oldest cap was evicted; the rest plus the newcomer remain.
	assert!(!is_active("captest-lru-a"), "LRU entry must be evicted");
	assert!(is_active("captest-lru-b"));
	assert!(is_active("captest-lru-c"));
	assert!(is_active("captest-lru-d"));
	assert!(is_active("captest-static"));
	assert_eq!(captest_active_count(), 4);

	clear_seeded_caps(&[
		"captest-lru-b",
		"captest-lru-c",
		"captest-lru-d",
		"captest-static",
	]);
	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_touch_capability_for_server_protects_from_eviction() {
	let sb = CapSandbox::new("touch");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let mut config = test_config();
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::builtin(
			"captest-static-srv",
			30,
			vec![],
		));

	seed_cap("captest-lru-a", "captest-lru-a-srv", &["t_a"], 400);
	seed_cap("captest-lru-b", "captest-lru-b-srv", &["t_b"], 300);
	seed_cap("captest-lru-c", "captest-lru-c-srv", &["t_c"], 200);
	seed_cap("captest-lru-d", "captest-lru-d-srv", &["t_d"], 100);

	// Touch the oldest cap's server: its last_used bumps to now, so the
	// second-oldest becomes the eviction victim.
	touch_capability_for_server("captest-lru-a-srv");

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "enable", "name": "captest-static"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result), "enable failed: {}", text_of(&result));

	assert!(
		is_active("captest-lru-a"),
		"touched cap must survive eviction"
	);
	assert!(
		!is_active("captest-lru-b"),
		"next-oldest must be evicted instead"
	);
	assert_eq!(captest_active_count(), 4);

	clear_seeded_caps(&[
		"captest-lru-a",
		"captest-lru-c",
		"captest-lru-d",
		"captest-static",
	]);
	crate::mcp::runtime::dynamic::clear_all();
}

#[tokio::test]
#[serial]
async fn test_capability_list_markers_and_triggers_preview() {
	let sb = CapSandbox::new("list");
	install_fixture_caps(&sb);
	let config = test_config();
	reset_registry();
	mark_active("captest-static", vec![]);

	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "list"})), &config)
			.await
			.expect("dispatch");
	assert!(!is_err(&result));
	let msg = text_of(&result);
	// The domain-bound fixture cap is filtered out (no session domain here),
	// so 6 of the 7 installed caps are listed.
	assert!(msg.contains("Installed capabilities (6)"), "got: {msg}");
	assert!(msg.contains("- [active] captest-static"), "got: {msg}");
	assert!(
		msg.contains("- [missing env] captest-envgate"),
		"got: {msg}"
	);
	assert!(
		msg.contains("(missing env: CAPTEST_MISSING_KEY)"),
		"got: {msg}"
	);
	// 4 triggers → preview shows the first 3 quoted plus the ellipsis suffix.
	assert!(
		msg.contains("\"use the static cap\", \"second trigger\", \"third trigger\", …"),
		"got: {msg}"
	);

	clear_seeded_caps(&["captest-static"]);
}

#[tokio::test]
#[serial]
async fn test_capability_list_and_discover_empty_tap_set() {
	let _sb = CapSandbox::new("nocaps");
	// No fixture caps installed — only the sandbox's (nonexistent) default tap.
	let config = test_config();

	let result =
		execute_capability_command(&cap_call(serde_json::json!({"action": "list"})), &config)
			.await
			.expect("dispatch");
	assert!(!is_err(&result));
	assert_eq!(text_of(&result), "No capabilities installed in any tap.");

	let result = execute_capability_command(
		&cap_call(serde_json::json!({"action": "discover", "intent": "anything"})),
		&config,
	)
	.await
	.expect("dispatch");
	assert!(!is_err(&result));
	assert!(text_of(&result).contains("No capabilities installed in any tap."));
}

#[test]
#[serial]
fn test_check_env_readiness_matrix() {
	let env = EnvGuard::new(&["CAPTEST_ENV_KEY"]);

	std::env::remove_var("CAPTEST_ENV_KEY");
	assert_eq!(
		check_env_readiness(&["CAPTEST_ENV_KEY".to_string()]),
		Err(vec!["CAPTEST_ENV_KEY".to_string()])
	);

	std::env::set_var("CAPTEST_ENV_KEY", "");
	assert_eq!(
		check_env_readiness(&["CAPTEST_ENV_KEY".to_string()]),
		Err(vec!["CAPTEST_ENV_KEY".to_string()])
	);

	std::env::set_var("CAPTEST_ENV_KEY", "value");
	assert_eq!(
		check_env_readiness(&["CAPTEST_ENV_KEY".to_string()]),
		Ok(())
	);
	assert_eq!(check_env_readiness(&[]), Ok(()));
	drop(env);
}

#[test]
#[serial]
fn test_domain_availability_rules() {
	// Universal caps (empty domains) are available everywhere, including
	// when no session domain is set.
	assert!(crate::agent::registry::cap_available_in_domain(
		&[],
		"medical"
	));
	assert!(crate::agent::registry::cap_available_in_domain(&[], ""));
	// Domain-bound caps require an exact domain match.
	assert!(crate::agent::registry::cap_available_in_domain(
		&["medical".to_string()],
		"medical"
	));
	assert!(!crate::agent::registry::cap_available_in_domain(
		&["medical".to_string()],
		"developer"
	));
	// Out-of-session (no domain context): only universal caps survive.
	assert!(!crate::agent::registry::cap_available_in_domain(
		&["medical".to_string()],
		""
	));
	// The private wrapper mirrors the rule against the session domain.
	assert!(cap_in_current_domain(&[]));
	assert!(!cap_in_current_domain(&["medical".to_string()]));
}

#[tokio::test]
#[serial]
async fn test_activate_capability_inline_mirrors_handle_enable() {
	let sb = CapSandbox::new("inline");
	install_fixture_caps(&sb);
	crate::mcp::runtime::dynamic::clear_all();
	reset_registry();
	let config = test_config();

	// Already active → idempotent empty result.
	mark_active("captest-dynamic", vec![]);
	let activated = activate_capability_inline("captest-dynamic", &config).await;
	assert_eq!(activated.unwrap(), Vec::<String>::new());

	// Env gate → anyhow error naming the missing var.
	let err = activate_capability_inline("captest-envgate", &config)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("requires env vars"));

	// Empty cap → anyhow error.
	let err = activate_capability_inline("captest-empty", &config)
		.await
		.unwrap_err();
	assert!(err.to_string().contains("no [[mcp.servers]] and no [deps]"));

	// Deps-only success → marked active, empty server list.
	let activated = activate_capability_inline("captest-deps-ok", &config).await;
	assert_eq!(activated.unwrap(), Vec::<String>::new());
	assert!(is_active("captest-deps-ok"));

	clear_seeded_caps(&["captest-dynamic", "captest-deps-ok"]);
	crate::mcp::runtime::dynamic::clear_all();
}

#[test]
#[serial]
fn test_list_active_names_is_sorted() {
	reset_registry();
	seed_cap("captest-names-c", "captest-names-srv", &["t"], 1);
	seed_cap("captest-names-a", "captest-names-srv", &["t"], 1);
	seed_cap("captest-names-b", "captest-names-srv", &["t"], 1);

	// Foreign entries can appear mid-flight (see `captest_active_count`): compare only our `captest-names-*` namespace.
	let names: Vec<String> = list_active_names()
		.into_iter()
		.filter(|n| n.starts_with("captest-names-"))
		.collect();
	assert_eq!(
		names,
		vec![
			"captest-names-a".to_string(),
			"captest-names-b".to_string(),
			"captest-names-c".to_string()
		]
	);

	clear_seeded_caps(&["captest-names-a", "captest-names-b", "captest-names-c"]);
}
