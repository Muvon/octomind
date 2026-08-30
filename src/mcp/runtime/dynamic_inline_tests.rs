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
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_list_empty() {
	clear_all();
	let servers = list_servers();
	assert!(servers.is_empty());
}

#[test]
fn test_mcp_function_definition() {
	let func = get_mcp_tool_function();
	assert_eq!(func.name, "mcp");
	assert!(func.parameters.get("properties").is_some());
}

#[tokio::test]
#[serial]
async fn test_persist_enabled_server_stores_auto_bind() {
	clear_all();

	// Set the current role (uses global RwLock, survives across threads)
	crate::config::set_thread_role("developer");

	// Register a stdio server
	let server = crate::config::McpServerConfig::stdin(
		"__test_persist_autobind",
		"echo",
		vec!["hello".to_string()],
		60,
		vec![],
	);
	register_server(server).unwrap();

	// Manually mark it as enabled (skip actual connection)
	{
		let manager = get_manager();
		let mut state = manager.write().unwrap();
		state
			.enabled
			.insert("__test_persist_autobind".to_string(), true);
	}

	// Persist — should include auto_bind = ["developer"]
	let result = persist_server("__test_persist_autobind", None).unwrap();

	// Verify the PersistResult
	assert_eq!(
		result.auto_bind,
		Some(vec!["developer".to_string()]),
		"auto_bind should be set to current role"
	);

	// Verify the actual file content
	let content = std::fs::read_to_string(&result.path).unwrap();
	assert!(
		content.contains("auto_bind"),
		"TOML file must contain auto_bind field, got:\n{}",
		content
	);
	assert!(
		content.contains("developer"),
		"TOML file must contain the role name, got:\n{}",
		content
	);

	// Cleanup
	let _ = std::fs::remove_file(&result.path);
	clear_all();
}

#[tokio::test]
#[serial]
async fn test_persist_disabled_server_clears_auto_bind() {
	clear_all();

	crate::config::set_thread_role("developer");

	// Register but do NOT enable
	let server = crate::config::McpServerConfig::stdin(
		"__test_persist_disabled",
		"echo",
		vec![],
		60,
		vec![],
	);
	register_server(server).unwrap();

	// Persist while disabled — auto_bind should be None
	let result = persist_server("__test_persist_disabled", None).unwrap();

	assert_eq!(
		result.auto_bind, None,
		"auto_bind should be None for disabled server"
	);

	// Verify the file does NOT contain auto_bind
	let content = std::fs::read_to_string(&result.path).unwrap();
	assert!(
		!content.contains("auto_bind"),
		"TOML file must NOT contain auto_bind for disabled server, got:\n{}",
		content
	);

	// Cleanup
	let _ = std::fs::remove_file(&result.path);
	clear_all();
}
/// Point `OCTOMIND_DATA_DIR` at a tempdir so persist/unpersist hit a sandbox
/// config dir, never the developer machine's real one. Tests using it must be
/// `#[serial]` (env is process-global).
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("tempdir");
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

fn template_config() -> crate::config::Config {
	toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template")
}

#[test]
#[serial]
fn test_register_server_validation() {
	clear_all();

	let err = register_server(crate::config::McpServerConfig::stdin(
		"",
		"echo",
		vec![],
		30,
		vec![],
	))
	.expect_err("blank name must bail");
	assert!(err.to_string().contains("name cannot be empty"));

	let err = register_server(crate::config::McpServerConfig::stdin(
		"__dyn_blank_cmd",
		"",
		vec![],
		30,
		vec![],
	))
	.expect_err("blank command must bail");
	assert!(err.to_string().contains("stdin server requires a command"));

	let http_blank = crate::config::McpServerConfig::Http {
		name: "__dyn_blank_url".to_string(),
		url: String::new(),
		timeout_seconds: 30,
		tools: vec![],
		headers: std::collections::HashMap::new(),
		auto_bind: None,
	};
	let err = register_server(http_blank).expect_err("blank url must bail");
	assert!(err.to_string().contains("http server requires a url"));
}

#[tokio::test]
#[serial]
async fn test_is_persisted_tracks_file_and_unpersist_missing_is_ok() {
	let _guard = DataDirGuard::new();
	clear_all();
	crate::config::set_thread_role("developer");

	assert!(!is_persisted("__dyn_persist_probe"));

	register_server(crate::config::McpServerConfig::stdin(
		"__dyn_persist_probe",
		"echo",
		vec![],
		30,
		vec![],
	))
	.unwrap();
	let result = persist_server("__dyn_persist_probe", None).expect("persist");
	assert!(is_persisted("__dyn_persist_probe"));

	unpersist_server("__dyn_persist_probe").expect("unpersist");
	assert!(!is_persisted("__dyn_persist_probe"));

	// Removing an already-missing file is a no-op success.
	unpersist_server("__dyn_persist_probe").expect("unpersist missing file");

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_persist_server_config_fallback_and_not_found() {
	let _guard = DataDirGuard::new();
	clear_all();
	crate::config::set_thread_role("developer");

	let mut config = template_config();
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::builtin(
			"__dyn_cfgpersist_srv",
			30,
			vec![],
		));

	// Not registered dynamically, but present in the config: treated as
	// enabled and persisted with the current role's auto_bind.
	let result =
		persist_server("__dyn_cfgpersist_srv", Some(&config)).expect("persist config server");
	assert_eq!(result.auto_bind, Some(vec!["developer".to_string()]));
	assert!(is_persisted("__dyn_cfgpersist_srv"));
	let _ = std::fs::remove_file(&result.path);

	// Unknown everywhere: hard error.
	let err = persist_server("__dyn_nope", Some(&config))
		.err()
		.expect("must bail");
	assert!(err.to_string().contains("not found"));

	clear_all();
}

#[tokio::test]
#[serial]
async fn test_disable_server_tools_config_fallback_and_not_found() {
	clear_all();

	let mut config = template_config();
	config
		.mcp
		.servers
		.push(crate::config::McpServerConfig::builtin(
			"__dyn_cfgdisable_srv",
			30,
			vec![],
		));

	// Config-loaded server disabled in CLI mode: registered into the global
	// singleton as a disabled shadow entry.
	disable_server("__dyn_cfgdisable_srv", Some(&config)).expect("disable config server");
	{
		let manager = get_manager();
		let state = manager.read().unwrap();
		assert!(state.servers.contains_key("__dyn_cfgdisable_srv"));
		assert!(!state
			.enabled
			.get("__dyn_cfgdisable_srv")
			.copied()
			.unwrap_or(true));
	}

	// kill_server=false in CLI mode only strips tools — always Ok.
	disable_server_tools("__dyn_cfgdisable_srv", &[], false, None).expect("tool-only disable");

	// Unknown everywhere: hard error.
	let err = disable_server("__dyn_nope", Some(&config)).expect_err("must bail");
	assert!(err.to_string().contains("not found"));

	// Removing an unknown server is a None, not an error.
	assert!(remove_server("__dyn_nope").is_none());

	clear_all();
}
