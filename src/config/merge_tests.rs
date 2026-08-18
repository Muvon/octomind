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

/// Test fixture: default template + a controlled server/role set.
/// `testsrv` auto-binds to `binder`; `restricted` grants itself only
/// `testsrv:alpha`; `excluded` references testsrv but grants it nothing.
const TEST_FIXTURE: &str = r#"

[[mcp.servers]]
name = "testsrv"
type = "builtin"
timeout_seconds = 30
tools = ["alpha", "beta"]
auto_bind = ["binder"]

[[roles]]
name = "binder"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "Binder role."
welcome = "hi"
mcp = { server_refs = [], allowed_tools = [] }

[[roles]]
name = "restricted"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "Restricted role."
welcome = "hi"
mcp = { server_refs = ["testsrv"], allowed_tools = ["testsrv:alpha"] }

[[roles]]
name = "excluded"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "Excluded role."
welcome = "hi"
mcp = { server_refs = ["core", "testsrv"], allowed_tools = ["core:*"] }
"#;

fn test_config() -> Config {
	let mut toml_src = include_str!("../../config-templates/default.toml").to_string();
	toml_src.push_str(TEST_FIXTURE);
	let mut config: Config = toml::from_str(&toml_src).expect("parse test config");
	config.build_role_map();
	config
}

fn server_names(config: &Config) -> Vec<&str> {
	config.mcp.servers.iter().map(|s| s.name()).collect()
}

#[test]
fn test_get_role_config_known_role() {
	let config = test_config();
	let (_, mcp, _, _, system) = config.get_role_config("restricted");
	assert_eq!(system, "Restricted role.");
	assert_eq!(mcp.server_refs, vec!["testsrv"]);
}

#[test]
fn test_get_role_config_unknown_falls_back_without_panic() {
	let config = test_config();
	let (_, _, _, _, system) = config.get_role_config("no-such-role");
	// Falls back to SOME defined role instead of tearing the session down
	assert!(config
		.roles
		.iter()
		.any(|role| role.config.system == *system));
}

#[test]
#[should_panic(expected = "role_map is empty")]
fn test_get_role_config_empty_role_map_panics() {
	let mut config = test_config();
	config.roles.clear();
	config.build_role_map();
	let _ = config.get_role_config("anything");
}

#[test]
fn test_merged_config_auto_bind_server() {
	let config = test_config();
	let merged = config.get_merged_config_for_role("binder");

	// Auto-bound server appears even though server_refs is empty
	assert!(server_names(&merged).contains(&"testsrv"));
	// role_map entry is patched so downstream readers see the same truth
	let role_entry = &merged.role_map["binder"];
	assert!(role_entry.mcp.server_refs.contains(&"testsrv".to_string()));
	// Unrestricted role (empty allowed_tools) stays unrestricted — no wildcard
	assert!(role_entry.mcp.allowed_tools.is_empty());
	assert!(merged.mcp.allowed_tools.is_empty());
	// System prompt comes from the role
	assert_eq!(merged.system.as_deref(), Some("Binder role."));
}

#[test]
fn test_merged_config_filters_tools_by_allowed_list() {
	let config = test_config();
	let merged = config.get_merged_config_for_role("restricted");

	let testsrv = merged
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "testsrv")
		.expect("testsrv enabled via explicit ref");
	// Concrete allow-list narrows the exposed tools
	assert_eq!(testsrv.tools().to_vec(), vec!["alpha".to_string()]);
	assert_eq!(merged.mcp.allowed_tools, vec!["testsrv:alpha"]);
}

#[test]
fn test_merged_config_drops_server_with_no_matching_grant() {
	let config = test_config();
	let merged = config.get_merged_config_for_role("excluded");

	let names = server_names(&merged);
	// core:* matches core; nothing matches testsrv → dropped entirely
	// (inverted-default fix: an unmatched server must not expose all tools)
	assert!(names.contains(&"core"));
	assert!(!names.contains(&"testsrv"));
}

#[test]
fn test_interactive_role_synthesizes_orchestration_tools() {
	let config = test_config();
	let merged = config.get_merged_config_for_interactive_role("binder");

	let orchestration = merged
		.mcp
		.servers
		.iter()
		.find(|s| s.name() == "orchestration")
		.expect("interactive session always gets orchestration");
	let mut tools = orchestration.tools().to_vec();
	tools.sort();
	assert_eq!(tools, vec!["monitor".to_string(), "schedule".to_string()]);

	// role_map entry gains the server ref for re-merging (e.g. /role)
	assert!(merged.role_map["binder"]
		.mcp
		.server_refs
		.contains(&"orchestration".to_string()));
}

#[test]
fn test_interactive_role_extends_restricted_grants() {
	let config = test_config();
	let merged = config.get_merged_config_for_interactive_role("restricted");

	// Restricted roles get explicit grants for the two session-flow tools
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"orchestration:schedule".to_string()));
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"orchestration:monitor".to_string()));
	// The original grant survives
	assert!(merged
		.mcp
		.allowed_tools
		.contains(&"testsrv:alpha".to_string()));
}
