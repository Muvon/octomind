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

//! Wire-format and role-resolution tests complementing the inline unit
//! tests: TOML deserialization/round trips for every server variant, the
//! `auto_bind` exact-match contract, and agent-manifest server merging
//! (concat + dedup by name).

use super::*;
use crate::config::loading::merge_agent_toml;

fn bound(server: McpServerConfig, roles: &[&str]) -> McpServerConfig {
	server.with_auto_bind(Some(roles.iter().map(|r| r.to_string()).collect()))
}

fn template_config() -> crate::config::Config {
	toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("default template must deserialize")
}

#[test]
fn builtin_variant_deserializes_with_auto_bind_from_toml() {
	let server: McpServerConfig = toml::from_str(
		r#"
type = "builtin"
name = "core"
timeout_seconds = 45
tools = ["plan"]
auto_bind = ["developer:general"]
"#,
	)
	.expect("builtin config must deserialize");
	assert_eq!(server.name(), "core");
	assert_eq!(server.timeout_seconds(), 45);
	assert_eq!(server.tools(), ["plan"]);
	let expected: Vec<String> = vec!["developer:general".to_string()];
	assert_eq!(server.auto_bind_roles(), Some(expected.as_slice()));
}

#[test]
fn stdio_variant_deserializes_env_and_cwd_from_toml() {
	let server: McpServerConfig = toml::from_str(
		r#"
type = "stdio"
name = "local"
command = "node"
args = ["server.js"]
timeout_seconds = 30
tools = []
env = { TOKEN = "{{ENV:MY_TOKEN}}" }
cwd = "/opt/plugin"
"#,
	)
	.expect("stdio config must deserialize");
	assert_eq!(server.command(), Some("node"));
	assert_eq!(
		server
			.env()
			.and_then(|env| env.get("TOKEN"))
			.map(String::as_str),
		Some("{{ENV:MY_TOKEN}}")
	);
	match &server {
		McpServerConfig::Stdin { cwd, .. } => assert_eq!(cwd.as_deref(), Some("/opt/plugin")),
		_ => panic!("fixture must deserialize as Stdin"),
	}
}

#[test]
fn stdin_is_not_a_wire_type_the_tag_is_stdio() {
	let correct: Result<McpServerConfig, _> = toml::from_str(
		"type = \"stdio\"\nname = \"s\"\ncommand = \"c\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);
	assert!(correct.is_ok(), "the stdio tag must be \"stdio\"");

	let wrong: Result<McpServerConfig, _> = toml::from_str(
		"type = \"stdin\"\nname = \"s\"\ncommand = \"c\"\nargs = []\ntimeout_seconds = 30\ntools = []\n",
	);
	assert!(
		wrong.is_err(),
		"\"stdin\" must not be accepted as a server type"
	);
}

#[test]
fn unknown_server_types_are_rejected() {
	let parsed: Result<McpServerConfig, _> = toml::from_str(
		"type = \"websocket\"\nname = \"s\"\nurl = \"wss://x\"\ntimeout_seconds = 30\ntools = []\n",
	);
	assert!(parsed.is_err(), "only builtin/http/stdio are wire types");
}

#[test]
fn serde_roundtrips_every_variant() {
	let http = McpServerConfig::Http {
		name: "remote".to_string(),
		url: "https://x/mcp".to_string(),
		timeout_seconds: 60,
		tools: vec!["fetch".to_string()],
		headers: HashMap::from([("Authorization".to_string(), "Bearer x".to_string())]),
		auto_bind: Some(vec!["developer".to_string()]),
	};
	let stdio = McpServerConfig::Stdin {
		name: "local".to_string(),
		command: "node".to_string(),
		args: vec!["server.js".to_string()],
		timeout_seconds: 30,
		tools: vec![],
		env: HashMap::from([("TOKEN".to_string(), "x".to_string())]),
		cwd: Some("/opt".to_string()),
		auto_bind: None,
	};
	let builtin = McpServerConfig::builtin("core", 30, vec!["plan".to_string()]);
	for server in [http, stdio, builtin] {
		let text = toml::to_string(&server).expect("server must serialize");
		let back: McpServerConfig = toml::from_str(&text).expect("server must round-trip");
		assert_eq!(back, server);
	}
}

#[test]
fn auto_bind_is_skipped_in_serialization_when_absent() {
	let text = toml::to_string(&McpServerConfig::builtin("core", 30, vec![]))
		.expect("builtin must serialize");
	assert!(!text.contains("auto_bind"));
}

#[test]
fn auto_bind_matches_exactly_not_by_prefix() {
	let server = bound(
		McpServerConfig::builtin("dev-only", 30, vec![]),
		&["developer:general"],
	);
	assert!(!server.auto_binds_to("developer"));
	assert!(server.auto_binds_to("developer:general"));
	assert!(!server.auto_binds_to("general"));
}

#[test]
fn exact_match_auto_bind_gates_get_enabled_servers() {
	let registry = vec![bound(
		McpServerConfig::builtin("tagged", 30, vec![]),
		&["developer:general"],
	)];
	let role = RoleMcpConfig::default();
	let plain = role.get_enabled_servers(&registry, Some("developer"));
	assert!(
		plain.is_empty(),
		"\"developer\" must not match auto_bind \"developer:general\""
	);
	let tagged = role.get_enabled_servers(&registry, Some("developer:general"));
	assert_eq!(tagged.len(), 1, "the full tag must match");
}

#[test]
fn tools_mut_edits_the_filter_in_place() {
	let mut server = McpServerConfig::builtin("core", 30, vec!["plan".to_string()]);
	server.tools_mut().push("review".to_string());
	server.tools_mut().retain(|tool| tool != "plan");
	assert_eq!(server.tools(), ["review"]);
}

#[test]
fn env_and_headers_accessors_are_variant_specific() {
	let stdio = McpServerConfig::stdin("s", "c", vec![], 30, vec![]);
	let http = McpServerConfig::http("h", "https://x", 30, vec![]);
	let builtin = McpServerConfig::builtin("b", 30, vec![]);
	assert!(stdio.env().is_some());
	assert!(http.env().is_none());
	assert!(builtin.env().is_none());
	assert!(http.headers().is_some());
	assert!(stdio.headers().is_none());
	assert!(builtin.headers().is_none());
}

#[test]
fn mcp_config_deserializes_servers_and_allowed_tools() {
	let config: McpConfig = toml::from_str(
		r#"
allowed_tools = ["core:*"]

[[servers]]
type = "builtin"
name = "core"
timeout_seconds = 30
tools = ["plan"]
"#,
	)
	.expect("McpConfig must deserialize");
	assert_eq!(config.allowed_tools, ["core:*"]);
	assert_eq!(config.servers.len(), 1);
	assert_eq!(config.servers[0].name(), "core");
}

#[test]
fn unnamespaced_allowed_tools_narrow_every_referenced_server() {
	let registry = vec![
		McpServerConfig::builtin("core", 30, vec!["read".to_string(), "write".to_string()]),
		McpServerConfig::builtin("other", 30, vec!["fetch".to_string()]),
	];
	let role = RoleMcpConfig {
		server_refs: vec!["core".to_string(), "other".to_string()],
		allowed_tools: vec!["read".to_string()],
	};
	let enabled = role.get_enabled_servers(&registry, None);
	assert_eq!(enabled[0].tools(), ["read"]);
	assert_eq!(enabled[1].tools(), ["read"]);
}

#[test]
fn enabled_servers_preserve_ref_order_then_registry_order_for_auto_binds() {
	let registry = vec![
		bound(McpServerConfig::builtin("z", 30, vec![]), &["dev"]),
		McpServerConfig::builtin("b", 30, vec![]),
		McpServerConfig::builtin("a", 30, vec![]),
	];
	let role = RoleMcpConfig {
		server_refs: vec!["b".to_string(), "a".to_string()],
		allowed_tools: vec![],
	};
	let enabled = role.get_enabled_servers(&registry, Some("dev"));
	let names: Vec<&str> = enabled.iter().map(|server| server.name()).collect();
	assert_eq!(names, ["b", "a", "z"]);
}

#[test]
fn merge_agent_toml_appends_servers_and_skips_duplicate_names() {
	let base = template_config();
	let before = base.mcp.servers.len();
	let merged = merge_agent_toml(
		&base,
		r#"
[[mcp.servers]]
type = "stdio"
name = "agent-extra"
command = "agent-tool"
args = []
timeout_seconds = 30
tools = []

[[mcp.servers]]
type = "builtin"
name = "core"
timeout_seconds = 999
tools = []
"#,
	)
	.expect("agent manifest must merge");
	let core_servers: Vec<_> = merged
		.mcp
		.servers
		.iter()
		.filter(|server| server.name() == "core")
		.collect();
	assert_eq!(
		core_servers.len(),
		1,
		"duplicate server names must be skipped"
	);
	// The duplicate is dropped whole — the base "core" keeps its own timeout.
	assert_eq!(core_servers[0].timeout_seconds(), 30);
	assert!(merged
		.mcp
		.servers
		.iter()
		.any(|server| server.name() == "agent-extra"));
	assert_eq!(merged.mcp.servers.len(), before + 1);
}

#[test]
fn merge_agent_toml_appends_roles_and_overrides_scalars() {
	let base = template_config();
	let merged = merge_agent_toml(
		&base,
		r#"
model = "openai:agent-model"

[[roles]]
name = "agent-role"
temperature = 0.2
top_p = 0.9
top_k = 10
system = "Agent."
welcome = "Hi."
mcp = { server_refs = [], allowed_tools = [] }

[[roles]]
name = "assistant"
temperature = 0.1
top_p = 0.5
top_k = 5
system = "OVERRIDE"
welcome = "No."
mcp = { server_refs = [], allowed_tools = [] }
"#,
	)
	.expect("agent manifest must merge");
	assert_eq!(merged.model, "openai:agent-model");
	assert!(merged.roles.iter().any(|role| role.name == "agent-role"));
	let assistants: Vec<_> = merged
		.roles
		.iter()
		.filter(|role| role.name == "assistant")
		.collect();
	assert_eq!(assistants.len(), 1, "duplicate role names must be skipped");
	assert_ne!(assistants[0].config.system, "OVERRIDE", "base role wins");
}
