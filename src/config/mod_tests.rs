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

//! External tests for `src/config/mod.rs` — enum helpers, MCP config helpers,
//! Config getters, role-map resolution, and the role-merge chain implemented in
//! `merge.rs` (`get_role_config` / `get_merged_config_for_role` /
//! `get_merged_config_for_interactive_role`).

use super::*;
use crate::session::output::OutputMode;
use serial_test::serial;
use std::collections::HashMap;
use std::path::PathBuf;

fn base_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

fn make_role(name: &str, server_refs: Vec<String>, allowed_tools: Vec<String>) -> Role {
	Role {
		name: name.to_string(),
		config: RoleConfig {
			model: None,
			system: format!("system prompt for {name}"),
			welcome: format!("welcome for {name}"),
			temperature: 0.3,
			top_p: 0.7,
			top_k: 20,
		},
		mcp: RoleMcpConfig::with_server_refs_and_tools(server_refs, allowed_tools),
	}
}

// --- LogLevel -----------------------------------------------------------------

#[test]
fn log_level_serde_roundtrip_and_helpers() {
	assert_eq!(
		serde_json::from_str::<LogLevel>("\"none\"").unwrap(),
		LogLevel::None
	);
	assert_eq!(
		serde_json::from_str::<LogLevel>("\"info\"").unwrap(),
		LogLevel::Info
	);
	assert_eq!(
		serde_json::from_str::<LogLevel>("\"debug\"").unwrap(),
		LogLevel::Debug
	);
	assert!(serde_json::from_str::<LogLevel>("\"verbose\"").is_err());

	assert!(!LogLevel::None.is_info_enabled());
	assert!(LogLevel::Info.is_info_enabled());
	assert!(LogLevel::Debug.is_info_enabled());
	assert!(!LogLevel::None.is_debug_enabled());
	assert!(!LogLevel::Info.is_debug_enabled());
	assert!(LogLevel::Debug.is_debug_enabled());

	assert_eq!(LogLevel::None.as_str(), "off");
	assert_eq!(LogLevel::Info.as_str(), "info");
	assert_eq!(LogLevel::Debug.as_str(), "debug");
}

// --- ReasoningEffortConfig ------------------------------------------------------

#[test]
fn reasoning_effort_parse_accepts_aliases_and_is_case_insensitive() {
	assert_eq!(
		ReasoningEffortConfig::parse("low"),
		Some(ReasoningEffortConfig::Low)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("  MEDIUM "),
		Some(ReasoningEffortConfig::Medium)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("med"),
		Some(ReasoningEffortConfig::Medium)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("High"),
		Some(ReasoningEffortConfig::High)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("xhigh"),
		Some(ReasoningEffortConfig::XHigh)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("x-high"),
		Some(ReasoningEffortConfig::XHigh)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("extra-high"),
		Some(ReasoningEffortConfig::XHigh)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("max"),
		Some(ReasoningEffortConfig::Max)
	);
	assert_eq!(
		ReasoningEffortConfig::parse("maximum"),
		Some(ReasoningEffortConfig::Max)
	);
	assert_eq!(ReasoningEffortConfig::parse("bogus"), None);
	assert_eq!(ReasoningEffortConfig::parse(""), None);
}

#[test]
fn reasoning_effort_as_str_and_octolib_mapping() {
	assert_eq!(ReasoningEffortConfig::Low.as_str(), "low");
	assert_eq!(ReasoningEffortConfig::Medium.as_str(), "medium");
	assert_eq!(ReasoningEffortConfig::High.as_str(), "high");
	assert_eq!(ReasoningEffortConfig::XHigh.as_str(), "xhigh");
	assert_eq!(ReasoningEffortConfig::Max.as_str(), "max");
	assert!(matches!(
		ReasoningEffortConfig::Low.to_octolib(),
		octolib::llm::ReasoningEffort::Low
	));
	assert!(matches!(
		ReasoningEffortConfig::Max.to_octolib(),
		octolib::llm::ReasoningEffort::Max
	));
}

// --- compression attention defaults ---------------------------------------------

#[test]
fn compression_attention_defaults() {
	let governance = CompressionAttentionGovernanceConfig::default();
	assert!(governance.enabled);
	assert!(governance.verify_hash);

	let attention = CompressionAttentionConfig::default();
	assert!(!attention.enabled);
	assert!(attention.validator);
	assert!(attention.telemetry);
	assert!(attention.governance.enabled);
	assert!(attention.governance.verify_hash);

	// serde(default) on the struct: an empty table deserializes to defaults.
	let parsed: CompressionAttentionConfig = toml::from_str("").unwrap();
	assert!(!parsed.enabled);
	assert!(parsed.validator);
	assert!(parsed.telemetry);
}

// --- McpConfig helpers -------------------------------------------------------------

#[test]
fn mcp_config_serialization_default_check() {
	assert!(McpConfig {
		servers: vec![],
		allowed_tools: vec![]
	}
	.is_default_for_serialization());
	assert!(!McpConfig {
		servers: vec![McpServerConfig::builtin("s", 1, vec![])],
		allowed_tools: vec![]
	}
	.is_default_for_serialization());
	assert!(!McpConfig {
		servers: vec![],
		allowed_tools: vec!["s:*".to_string()]
	}
	.is_default_for_serialization());
}

#[test]
fn mcp_config_get_all_servers_clones_registry() {
	let config = base_config();
	let servers = config.mcp.get_all_servers();
	assert_eq!(servers.len(), config.mcp.servers.len());
	assert!(servers.iter().any(|s| s.name() == "core"));
	assert!(servers.iter().any(|s| s.name() == "orchestration"));
}

#[test]
fn mcp_config_with_servers_renames_entries_to_map_keys() {
	let mut servers = HashMap::new();
	servers.insert(
		"b".to_string(),
		McpServerConfig::builtin("wrong-b", 5, vec!["t".to_string()]),
	);
	servers.insert(
		"h".to_string(),
		McpServerConfig::http("wrong-h", "http://localhost", 5, vec![]),
	);
	servers.insert(
		"s".to_string(),
		McpServerConfig::stdin("wrong-s", "cmd", vec!["--flag".to_string()], 5, vec![]),
	);

	let mcp = McpConfig::with_servers(servers, Some(vec!["b:*".to_string()]));
	let names: Vec<&str> = mcp.servers.iter().map(|s| s.name()).collect();
	assert_eq!(names.len(), 3);
	for expected in ["b", "h", "s"] {
		assert!(
			names.contains(&expected),
			"missing renamed server {expected}"
		);
	}
	assert_eq!(mcp.allowed_tools, vec!["b:*".to_string()]);

	let mcp = McpConfig::with_servers(HashMap::new(), None);
	assert!(mcp.servers.is_empty());
	assert!(mcp.allowed_tools.is_empty());
}

// --- Config getters ------------------------------------------------------------------

#[test]
fn config_effective_model_and_tokens_come_from_root() {
	let config = base_config();
	assert_eq!(config.get_effective_model(), config.model);
	assert_eq!(config.get_effective_max_tokens(), config.max_tokens);
	assert_eq!(config.get_model("assistant"), config.model);
	assert_eq!(config.get_max_tokens("whatever"), config.max_tokens);
	assert_eq!(config.get_log_level(), config.log_level);
	assert_eq!(config.version, CURRENT_CONFIG_VERSION);
}

#[test]
fn config_server_and_hook_lookups() {
	let config = base_config();
	let core = config
		.get_server_config("core")
		.expect("core server in template");
	assert_eq!(core.name(), "core");
	assert_eq!(core.connection_type(), McpConnectionType::Builtin);
	assert!(config.get_server_config("no-such-server").is_none());
	assert!(config.get_hook_by_name("no-such-hook").is_none());
}

#[test]
fn output_mode_maps_runtime_modes() {
	let mut config = base_config();
	// No runtime mode set → "plain" → NonInteractive.
	assert_eq!(config.output_mode(), OutputMode::NonInteractive);
	config.runtime_output_mode = Some("interactive".to_string());
	assert_eq!(config.output_mode(), OutputMode::Interactive);
	config.runtime_output_mode = Some("jsonl".to_string());
	assert_eq!(config.output_mode(), OutputMode::Jsonl);
	assert!(config.output_mode().should_suppress_cli_output());
	config.runtime_output_mode = Some("websocket".to_string());
	assert_eq!(config.output_mode(), OutputMode::WebSocket);
	config.runtime_output_mode = Some("gibberish".to_string());
	assert_eq!(config.output_mode(), OutputMode::NonInteractive);
}

#[test]
fn working_directory_defaults_to_cwd_and_can_be_overridden() {
	let mut config = base_config();
	assert_eq!(
		config.get_working_directory(),
		std::env::current_dir().unwrap()
	);
	let marker = PathBuf::from("/tmp/octomind-config-test-cwd");
	config.set_working_directory(marker.clone());
	assert_eq!(config.get_working_directory(), marker);
}

// --- role map / get_role_config -----------------------------------------------------

#[test]
fn build_role_map_indexes_all_roles() {
	let config = base_config();
	assert!(config.has_role("assistant"));
	assert!(config.has_role("task_refiner"));
	assert!(!config.has_role("nonexistent-role"));
	assert_eq!(config.role_map.len(), config.roles.len());
}

#[test]
fn get_role_config_returns_exact_role_and_falls_back() {
	let mut config = base_config();
	config.roles = vec![
		make_role("alpha", vec![], vec![]),
		make_role("beta", vec![], vec![]),
	];
	config.build_role_map();

	let (role_config, mcp, _layers, _commands, system) = config.get_role_config("alpha");
	assert_eq!(system, "system prompt for alpha");
	assert_eq!(role_config.system, "system prompt for alpha");
	assert!(mcp.server_refs.is_empty());
	assert_eq!(
		config.get_role_config_struct("beta").system,
		"system prompt for beta"
	);

	// Unknown role falls back to some configured role instead of panicking.
	let (_rc, _m, _l, _c, fallback_system) = config.get_role_config("ghost");
	assert!(
		fallback_system.starts_with("system prompt for "),
		"unexpected fallback system: {fallback_system}"
	);
}

#[test]
#[should_panic(expected = "role_map is empty")]
fn get_role_config_panics_on_empty_role_map() {
	let mut config = base_config();
	config.roles.clear();
	config.build_role_map();
	let _ = config.get_role_config("anything");
}

#[test]
fn get_enabled_servers_for_role_delegates_to_role_mcp_config() {
	let config = base_config();
	let role = config.role_map.get("assistant").unwrap().clone();
	let servers = config.get_enabled_servers_for_role(&role.mcp, Some("assistant"));
	assert!(!servers.is_empty());
	let empty = config.get_enabled_servers_for_role(&RoleMcpConfig::default(), None);
	assert!(empty.is_empty());
}

// --- get_merged_config_for_role --------------------------------------------------------

#[test]
fn merged_config_for_role_filters_servers_and_patches_refs() {
	let mut config = base_config();
	config.roles = vec![
		make_role(
			"alpha",
			vec!["core".to_string()],
			vec!["core:plan".to_string()],
		),
		make_role("beta", vec![], vec![]),
	];
	config.build_role_map();
	config.mcp.servers = vec![
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::builtin("unused", 30, vec![]),
		McpServerConfig::builtin("auto-srv", 30, vec![])
			.with_auto_bind(Some(vec!["beta".to_string()])),
	];

	let merged = config.get_merged_config_for_role("alpha");
	let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name()).collect();
	assert_eq!(names, vec!["core"]);
	assert!(merged.mcp.servers[0].tools().iter().any(|t| t == "plan"));
	assert_eq!(merged.system.as_deref(), Some("system prompt for alpha"));
	let alpha = merged.role_map.get("alpha").unwrap();
	assert_eq!(alpha.mcp.server_refs, vec!["core"]);

	// beta has no explicit refs, but auto-srv binds to it: the server is added
	// to both the merged registry and the patched server_refs.
	let merged = config.get_merged_config_for_role("beta");
	let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name()).collect();
	assert_eq!(names, vec!["auto-srv"]);
	let beta = merged.role_map.get("beta").unwrap();
	assert_eq!(beta.mcp.server_refs, vec!["auto-srv"]);
	assert!(beta.mcp.allowed_tools.is_empty());
}

#[test]
fn merged_config_for_role_appends_wildcard_for_auto_bind_when_filtered() {
	let mut config = base_config();
	config.roles = vec![make_role(
		"gamma",
		vec!["core".to_string()],
		vec!["core:plan".to_string()],
	)];
	config.build_role_map();
	config.mcp.servers = vec![
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::builtin("auto-srv", 30, vec![])
			.with_auto_bind(Some(vec!["gamma".to_string()])),
	];

	let merged = config.get_merged_config_for_role("gamma");
	assert!(merged.mcp.allowed_tools.contains(&"core:plan".to_string()));
	assert!(
		merged.mcp.allowed_tools.contains(&"auto-srv:*".to_string()),
		"auto-bind server must get a wildcard grant in restricted mode"
	);
	let gamma = merged.role_map.get("gamma").unwrap();
	assert!(gamma.mcp.server_refs.contains(&"auto-srv".to_string()));
}

#[test]
fn merged_config_for_default_template_roles() {
	let config = base_config();
	let merged = config.get_merged_config_for_role("assistant");
	let names: Vec<String> = merged
		.mcp
		.servers
		.iter()
		.map(|s| s.name().to_string())
		.collect();
	for expected in ["core", "orchestration", "runtime", "agent"] {
		assert!(
			names.contains(&expected.to_string()),
			"assistant must see {expected}"
		);
	}
	// "filesystem" is referenced by the role but absent from the template
	// registry — it is skipped, not synthesized.
	assert!(!names.contains(&"filesystem".to_string()));
	assert_eq!(
		merged.system.as_deref(),
		Some(
			config
				.role_map
				.get("assistant")
				.unwrap()
				.config
				.system
				.as_str()
		)
	);

	// task_refiner has no servers: its merged registry is empty.
	let merged = config.get_merged_config_for_role("task_refiner");
	assert!(merged.mcp.servers.is_empty());
}

// --- get_merged_config_for_interactive_role ----------------------------------------------

#[test]
fn interactive_role_merge_synthesizes_orchestration() {
	let mut config = base_config();
	config.roles = vec![make_role("solo", vec![], vec![])];
	config.build_role_map();
	config.mcp.servers = vec![McpServerConfig::builtin("core", 30, vec![])];

	let merged = config.get_merged_config_for_interactive_role("solo");
	let orchestration = merged
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "orchestration")
		.expect("orchestration synthesized for interactive session");
	assert_eq!(
		orchestration.tools(),
		&["schedule".to_string(), "monitor".to_string()]
	);
	let solo = merged.role_map.get("solo").unwrap();
	assert!(solo.mcp.server_refs.iter().any(|n| n == "orchestration"));
	// Unrestricted role: no allowed_tools grants to append.
	assert!(merged.mcp.allowed_tools.is_empty());
}

#[test]
fn interactive_role_merge_unions_concrete_tool_grants() {
	let mut config = base_config();
	config.roles = vec![make_role(
		"dev",
		vec!["orchestration".to_string()],
		vec!["orchestration:tap".to_string()],
	)];
	config.build_role_map();
	config.mcp.servers = vec![McpServerConfig::builtin("orchestration", 30, vec![])];

	let merged = config.get_merged_config_for_interactive_role("dev");
	let orchestration = merged
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "orchestration")
		.expect("orchestration enabled for dev");
	for tool in ["tap", "schedule", "monitor"] {
		assert!(
			orchestration.tools().iter().any(|t| t == tool),
			"missing interactive tool {tool}"
		);
	}
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"orchestration:schedule".to_string()));
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"orchestration:monitor".to_string()));
	let dev = merged.role_map.get("dev").unwrap();
	assert!(dev
		.mcp
		.allowed_tools
		.contains(&"orchestration:schedule".to_string()));
}

#[test]
fn interactive_role_merge_keeps_empty_tool_list_as_full_grant() {
	// assistant-style: orchestration enabled with a wildcard filter → the
	// server's tool list stays empty (= all tools); the overlay must not
	// narrow it, only append the allowed_tools grants.
	let config = base_config();
	let merged = config.get_merged_config_for_interactive_role("assistant");
	let orchestration = merged
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "orchestration")
		.expect("orchestration enabled for assistant");
	assert!(orchestration.tools().is_empty());
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"orchestration:schedule".to_string()));
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"orchestration:monitor".to_string()));
}

// --- thread-local / process-global config plumbing ----------------------------------------

#[test]
fn thread_config_roundtrip_via_thread_local() {
	// Each test runs on its own thread: no config has been set here yet.
	assert!(with_thread_config(|_| ()).is_none());
	let config = base_config();
	set_thread_config(&config);
	let seen = with_thread_config(|c| c.model.clone());
	assert_eq!(seen, Some(config.model.clone()));
}

#[serial]
#[test]
fn thread_role_roundtrip_via_process_global() {
	set_thread_role("config-mod-test-role");
	assert_eq!(get_thread_role().as_deref(), Some("config-mod-test-role"));
}
