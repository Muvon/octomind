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

// Helper function to load and modify the default config template for testing
fn get_test_config_with_custom_role() -> String {
	// Load the default config template
	let template_content = include_str!("../../config-templates/default.toml");

	// Add a custom "tester" role to the template for testing
	let mut config = template_content.to_string();

	// Add test roles (developer, assistant, tester) — self-contained, not relying on template
	config.push_str(
		r#"

# Test roles for unit testing
[[roles]]
name = "developer"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "You are a developer assistant."
welcome = "Hello! Developer role."
mcp = { server_refs = [], allowed_tools = [] }

[[roles]]
name = "assistant"
temperature = 0.5
top_p = 0.9
top_k = 40
system = "You are a general assistant."
welcome = "Hello! Assistant role."
mcp = { server_refs = [], allowed_tools = [] }

[[roles]]
name = "tester"
temperature = 0.7
top_p = 0.9
top_k = 50
system = "You are a test assistant."
welcome = "Hello! Test tester role."
mcp = { server_refs = ["test_server", "clt"], allowed_tools = [] }

# Additional test MCP servers for tester role
[[mcp.servers]]
name = "test_server"
type = "stdio"
command = "test_command"
args = ["mcp"]
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "clt"
type = "stdio"
command = "clt"
args = ["mcp"]
timeout_seconds = 30
tools = []
"#,
	);

	config
}

#[test]
fn test_role_parsing() {
	let test_config = get_test_config_with_custom_role();

	// Parse the config
	let mut config: Config = toml::from_str(&test_config).expect("Failed to parse test config");
	config.build_role_map();

	// Verify roles were parsed (template has 4 roles + 3 appended by test = 7 in vec; assistant deduped in map → 6)
	assert_eq!(config.roles.len(), 7);
	assert_eq!(config.role_map.len(), 6);
	assert!(config.role_map.contains_key("tester"));

	let tester_role = config.role_map.get("tester").unwrap();
	assert_eq!(tester_role.mcp.server_refs, vec!["test_server", "clt"]);

	// Test get_role_config for custom role
	let (role_config, mcp_config, _, _, _) = config.get_role_config("tester");
	// Verify role config structure is valid
	assert_eq!(role_config.temperature, 0.7);
	assert_eq!(mcp_config.server_refs, vec!["test_server", "clt"]);

	// Test get_merged_config_for_mode for custom role
	let merged_config = config.get_merged_config_for_role("tester");
	// The merged config should only include servers that are referenced by the tester role
	let server_names: Vec<&str> = merged_config.mcp.servers.iter().map(|s| s.name()).collect();
	assert!(server_names.contains(&"test_server"));
	assert!(server_names.contains(&"clt"));
	// Should not contain servers not referenced by the tester role
	assert!(!server_names.contains(&"core"));
	assert!(!server_names.contains(&"filesystem"));
}

#[test]
fn test_role_merged_config() {
	let test_config = get_test_config_with_custom_role();

	// Parse the config
	let mut config: Config = toml::from_str(&test_config).expect("Failed to parse test config");
	config.build_role_map();

	// Test that the merged config for tester role only includes the specified servers
	let merged_config = config.get_merged_config_for_role("tester");
	// The merged config should only have servers that are in the tester role's server_refs
	let server_names: Vec<&str> = merged_config.mcp.servers.iter().map(|s| s.name()).collect();
	assert!(server_names.contains(&"test_server"));
	assert!(server_names.contains(&"clt"));
	assert!(!server_names.contains(&"core")); // Should not be included
	assert!(!server_names.contains(&"filesystem")); // Should not be included
}

#[test]
fn interactive_role_adds_only_schedule_and_monitor() {
	let mut config: Config =
		toml::from_str(&get_test_config_with_custom_role()).expect("parse test config");
	config.build_role_map();

	let regular = config.get_merged_config_for_role("tester");
	assert!(
		regular
			.mcp
			.servers
			.iter()
			.all(|server| server.name() != "orchestration"),
		"non-interactive role merge must remain unchanged"
	);

	let interactive = config.get_merged_config_for_interactive_role("tester");
	let orchestration = interactive
		.mcp
		.servers
		.iter()
		.find(|server| server.name() == "orchestration")
		.expect("interactive role must include orchestration");
	assert_eq!(orchestration.tools(), ["schedule", "monitor"]);
	assert!(interactive
		.role_map
		.get("tester")
		.expect("tester role")
		.mcp
		.server_refs
		.contains(&"orchestration".to_string()));

	// MCP initialization re-merges its input. The compatibility fields must
	// preserve the same narrow grant across that second merge.
	let remerged = interactive.get_merged_config_for_role("tester");
	let orchestration = remerged
		.mcp
		.servers
		.iter()
		.find(|server| server.name() == "orchestration")
		.expect("interactive tools must survive re-merge");
	assert_eq!(orchestration.tools(), ["schedule", "monitor"]);
}

#[test]
fn interactive_role_synthesizes_missing_orchestration_builtin() {
	let mut config: Config =
		toml::from_str(&get_test_config_with_custom_role()).expect("parse test config");
	config
		.mcp
		.servers
		.retain(|server| server.name() != "orchestration");
	config.build_role_map();

	let interactive = config.get_merged_config_for_interactive_role("tester");
	let orchestration = interactive
		.mcp
		.servers
		.iter()
		.find(|server| server.name() == "orchestration")
		.expect("orchestration builtin must be synthesized when absent from registry");
	assert_eq!(orchestration.tools(), ["schedule", "monitor"]);
}

#[test]
fn interactive_role_preserves_existing_orchestration_grants() {
	let mut config: Config =
		toml::from_str(&get_test_config_with_custom_role()).expect("parse test config");
	config.build_role_map();
	let tester = config.role_map.get_mut("tester").expect("tester role");
	tester.mcp.server_refs.push("orchestration".to_string());
	tester
		.mcp
		.allowed_tools
		.push("orchestration:tap".to_string());

	let interactive = config.get_merged_config_for_interactive_role("tester");
	let orchestration = interactive
		.mcp
		.servers
		.iter()
		.find(|server| server.name() == "orchestration")
		.expect("orchestration server");
	assert_eq!(orchestration.tools(), ["tap", "schedule", "monitor"]);
	assert!(interactive
		.mcp
		.allowed_tools
		.contains(&"orchestration:tap".to_string()));
	assert!(interactive
		.mcp
		.allowed_tools
		.contains(&"orchestration:schedule".to_string()));
	assert!(interactive
		.mcp
		.allowed_tools
		.contains(&"orchestration:monitor".to_string()));
}

/// Config with auto_bind servers for testing auto-bind behavior.
/// - `auto_bound` binds to the `developer` role via `auto_bind`
/// - `other_bound` binds to `assistant` only (should NOT appear for developer)
fn get_test_config_with_auto_bind() -> String {
	let mut config = include_str!("../../config-templates/default.toml").to_string();
	config.push_str(
		r#"

[[roles]]
name = "developer"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "Developer."
welcome = "Hi."
mcp = { server_refs = ["explicit"], allowed_tools = ["explicit:*"] }

[[roles]]
name = "assistant"
temperature = 0.5
top_p = 0.9
top_k = 40
system = "Assistant."
welcome = "Hi."
mcp = { server_refs = [], allowed_tools = [] }

[[mcp.servers]]
name = "explicit"
type = "stdio"
command = "explicit"
args = []
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "auto_bound"
type = "stdio"
command = "auto_bound"
args = []
timeout_seconds = 30
tools = []
auto_bind = ["developer"]

[[mcp.servers]]
name = "other_bound"
type = "stdio"
command = "other"
args = []
timeout_seconds = 30
tools = []
auto_bind = ["assistant"]
"#,
	);
	config
}

#[test]
fn test_auto_bind_server_appears_in_merged_servers() {
	let mut config: Config = toml::from_str(&get_test_config_with_auto_bind()).expect("parse");
	config.build_role_map();

	let merged = config.get_merged_config_for_role("developer");
	let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name()).collect();

	assert!(
		names.contains(&"explicit"),
		"explicit server missing: {names:?}"
	);
	assert!(
		names.contains(&"auto_bound"),
		"auto_bound server missing: {names:?}"
	);
	assert!(
		!names.contains(&"other_bound"),
		"other_bound should NOT auto-bind to developer: {names:?}"
	);
}

#[test]
fn test_auto_bind_patches_server_refs_in_role_map() {
	let mut config: Config = toml::from_str(&get_test_config_with_auto_bind()).expect("parse");
	config.build_role_map();

	let merged = config.get_merged_config_for_role("developer");
	let role_entry = merged
		.role_map
		.get("developer")
		.expect("developer role must exist");

	assert!(
		role_entry
			.mcp
			.server_refs
			.contains(&"auto_bound".to_string()),
		"auto_bound must be added to role_map server_refs, got: {:?}",
		role_entry.mcp.server_refs
	);
	assert!(
		role_entry.mcp.server_refs.contains(&"explicit".to_string()),
		"explicit server_ref must survive: {:?}",
		role_entry.mcp.server_refs
	);
}

#[test]
fn test_auto_bind_patches_allowed_tools_wildcard() {
	let mut config: Config = toml::from_str(&get_test_config_with_auto_bind()).expect("parse");
	config.build_role_map();

	let merged = config.get_merged_config_for_role("developer");

	// allowed_tools is non-empty (`explicit:*`) so patching must add `auto_bound:*`
	assert!(
		merged
			.mcp
			.allowed_tools
			.contains(&"auto_bound:*".to_string()),
		"auto_bound:* must be appended to allowed_tools, got: {:?}",
		merged.mcp.allowed_tools
	);
	assert!(
		merged.mcp.allowed_tools.contains(&"explicit:*".to_string()),
		"explicit:* must survive: {:?}",
		merged.mcp.allowed_tools
	);

	// Role map must mirror the merged allowed_tools.
	let role_entry = merged.role_map.get("developer").unwrap();
	assert_eq!(
		role_entry.mcp.allowed_tools, merged.mcp.allowed_tools,
		"role_map allowed_tools must match merged.mcp.allowed_tools"
	);
}

#[test]
fn test_auto_bind_empty_allowed_tools_stays_empty() {
	// When allowed_tools is empty = unrestricted → nothing to patch
	let mut config_str = get_test_config_with_auto_bind();
	// swap developer role to have empty allowed_tools
	config_str = config_str.replace(
		r#"mcp = { server_refs = ["explicit"], allowed_tools = ["explicit:*"] }"#,
		r#"mcp = { server_refs = ["explicit"], allowed_tools = [] }"#,
	);

	let mut config: Config = toml::from_str(&config_str).expect("parse");
	config.build_role_map();

	let merged = config.get_merged_config_for_role("developer");
	assert!(
		merged.mcp.allowed_tools.is_empty(),
		"empty allowed_tools must remain empty (unrestricted mode), got: {:?}",
		merged.mcp.allowed_tools
	);
	// server_refs still patched even when allowed_tools is empty
	let role_entry = merged.role_map.get("developer").unwrap();
	assert!(
		role_entry
			.mcp
			.server_refs
			.contains(&"auto_bound".to_string()),
		"auto_bound must still be in server_refs even when allowed_tools is empty"
	);
}

#[test]
fn test_auto_bind_does_not_leak_across_roles() {
	let mut config: Config = toml::from_str(&get_test_config_with_auto_bind()).expect("parse");
	config.build_role_map();

	let merged = config.get_merged_config_for_role("assistant");
	let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name()).collect();

	assert!(
		names.contains(&"other_bound"),
		"other_bound must bind to assistant: {names:?}"
	);
	assert!(
		!names.contains(&"auto_bound"),
		"auto_bound (developer-only) must NOT leak to assistant: {names:?}"
	);
}

#[test]
fn test_max_tokens_inheritance() {
	let test_config = get_test_config_with_custom_role();

	// Parse the config
	let mut config: Config = toml::from_str(&test_config).expect("Failed to parse test config");
	config.build_role_map();

	// Test that all roles use the root level max_tokens (32768 from test config)
	assert_eq!(config.get_max_tokens("developer"), 32768);
	assert_eq!(config.get_max_tokens("assistant"), 32768);
	assert_eq!(config.get_max_tokens("tester"), 32768);
	assert_eq!(config.get_max_tokens("nonexistent_role"), 32768); // Should still return root level
															   // Test get_effective_max_tokens directly
	assert_eq!(config.get_effective_max_tokens(), 32768);

	// Verify that RoleConfig no longer has max_tokens field by checking the role config struct
	let (role_config, _, _, _, _) = config.get_role_config("tester");
	// This test verifies the refactoring where max_tokens was moved from RoleConfig to system-wide
	// We verify role config is valid by checking its temperature field
	assert_eq!(role_config.temperature, 0.7);
	// Verify developer role exists in config
	assert!(config.role_map.contains_key("developer"));
}
