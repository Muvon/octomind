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

fn role(refs: &[&str], allowed: &[&str]) -> RoleMcpConfig {
	RoleMcpConfig {
		server_refs: refs.iter().map(|s| s.to_string()).collect(),
		allowed_tools: allowed.iter().map(|s| s.to_string()).collect(),
	}
}

fn tools(names: &[&str]) -> Vec<String> {
	names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn accessors_work_across_variants() {
	let builtin = McpServerConfig::builtin("core", 30, tools(&["read"]));
	let http = McpServerConfig::http("remote", "https://x/mcp", 60, vec![]);
	let stdio = McpServerConfig::stdin("local", "node", tools(&["server.js"]), 10, vec![]);

	assert_eq!(builtin.name(), "core");
	assert_eq!(builtin.connection_type(), McpConnectionType::Builtin);
	assert_eq!(builtin.timeout_seconds(), 30);
	assert_eq!(builtin.tools(), tools(&["read"]));
	assert_eq!(builtin.url(), None);
	assert_eq!(builtin.command(), None);
	assert!(builtin.args().is_empty());

	assert_eq!(http.connection_type(), McpConnectionType::Http);
	assert_eq!(http.url(), Some("https://x/mcp"));
	assert_eq!(http.command(), None);

	assert_eq!(stdio.connection_type(), McpConnectionType::Stdin);
	assert_eq!(stdio.command(), Some("node"));
	assert_eq!(stdio.args(), tools(&["server.js"]));
	assert_eq!(stdio.url(), None);
}

#[test]
fn connection_type_wire_strings_are_stable() {
	assert_eq!(McpConnectionType::Builtin.as_str(), "builtin");
	assert_eq!(McpConnectionType::Stdin.as_str(), "stdin");
	assert_eq!(McpConnectionType::Http.as_str(), "http");
}

#[test]
fn http_headers_deserialize_from_inline_table() {
	let server: McpServerConfig = toml::from_str(
		r#"
type = "http"
name = "remote"
url = "https://example.com/mcp"
timeout_seconds = 30
tools = []
headers = { Authorization = "Bearer {{ENV:MY_MCP_TOKEN}}", X_Client = "octomind" }
"#,
	)
	.expect("HTTP config with inline headers must deserialize");
	let headers = server.headers().expect("HTTP config must expose headers");
	assert_eq!(
		headers.get("Authorization").map(String::as_str),
		Some("Bearer {{ENV:MY_MCP_TOKEN}}")
	);
	assert_eq!(
		headers.get("X_Client").map(String::as_str),
		Some("octomind")
	);
}

#[test]
fn absent_http_headers_deserialize_as_empty() {
	let server: McpServerConfig = toml::from_str(
		r#"
type = "http"
name = "remote"
url = "https://example.com/mcp"
timeout_seconds = 30
tools = []
"#,
	)
	.expect("HTTP config without headers must deserialize");
	assert!(server
		.headers()
		.expect("HTTP config must expose headers")
		.is_empty());
}

#[test]
fn validate_rejects_empty_identity_fields() {
	assert!(McpServerConfig::builtin("core", 30, vec![])
		.validate()
		.is_ok());
	assert!(McpServerConfig::builtin("", 30, vec![]).validate().is_err());
	assert!(McpServerConfig::http("h", "", 30, vec![])
		.validate()
		.is_err());
	assert!(McpServerConfig::http("", "u", 30, vec![])
		.validate()
		.is_err());
	assert!(McpServerConfig::stdin("s", "", vec![], 30, vec![])
		.validate()
		.is_err());
	assert!(McpServerConfig::stdin("", "cmd", vec![], 30, vec![])
		.validate()
		.is_err());
}

#[test]
fn with_auto_bind_preserves_the_rest_of_the_config() {
	let stdio = McpServerConfig::stdin("local", "node", tools(&["a.js"]), 10, tools(&["t"]));
	let bound = stdio.with_auto_bind(Some(vec!["developer".to_string()]));
	assert!(bound.auto_binds_to("developer"));
	assert!(!bound.auto_binds_to("assistant"));
	assert_eq!(bound.command(), Some("node"));
	assert_eq!(bound.args(), tools(&["a.js"]));
	assert_eq!(bound.tools(), tools(&["t"]));
	// The original is untouched.
	assert!(!stdio.auto_binds_to("developer"));
}

#[test]
fn is_enabled_tracks_server_refs() {
	assert!(!RoleMcpConfig::default().is_enabled());
	assert!(role(&["core"], &[]).is_enabled());
}

#[test]
fn star_pattern_means_all_tools() {
	// `None` = unrestricted, which downstream renders as an empty tool list.
	assert_eq!(
		role(&[], &["core:*"]).expand_patterns_for_server("core"),
		None
	);
}

#[test]
fn unmatched_patterns_grant_nothing_not_everything() {
	// The critical inversion: a role that filters only `other:*` must not
	// end up with full access to `core`.
	assert_eq!(
		role(&[], &["other:*"]).expand_patterns_for_server("core"),
		Some(vec![])
	);
}

#[test]
fn patterns_expand_to_exact_names_and_prefixes() {
	let r = role(&[], &["core:read", "core:text_*", "other:write"]);
	assert_eq!(
		r.expand_patterns_for_server("core"),
		Some(tools(&["read", "text_*"]))
	);
	assert_eq!(
		r.expand_patterns_for_server("other"),
		Some(tools(&["write"]))
	);
}

#[test]
fn unnamespaced_patterns_apply_to_every_server() {
	let r = role(&[], &["shell"]);
	assert_eq!(
		r.expand_patterns_for_server("core"),
		Some(tools(&["shell"]))
	);
	assert_eq!(
		r.expand_patterns_for_server("other"),
		Some(tools(&["shell"]))
	);
}

#[test]
fn enabled_servers_resolve_refs_and_ignore_unknown_names() {
	let registry = vec![
		McpServerConfig::builtin("core", 30, tools(&["read", "write"])),
		McpServerConfig::builtin("extra", 30, vec![]),
	];
	let enabled = role(&["core", "ghost"], &[]).get_enabled_servers(&registry, None);
	assert_eq!(enabled.len(), 1);
	assert_eq!(enabled[0].name(), "core");
	// No role filter → the server keeps its own tool list.
	assert_eq!(enabled[0].tools(), tools(&["read", "write"]));
}

#[test]
fn enabled_servers_apply_the_role_tool_filter() {
	let registry = vec![
		McpServerConfig::builtin("core", 30, tools(&["read", "write", "shell"])),
		McpServerConfig::http("remote", "https://x", 30, tools(&["fetch"])),
	];
	let r = role(&["core", "remote"], &["core:read", "remote:*"]);
	let enabled = r.get_enabled_servers(&registry, None);
	assert_eq!(enabled.len(), 2);
	// Concrete allow-list for `core`.
	assert_eq!(enabled[0].tools(), tools(&["read"]));
	// `remote:*` collapses to the empty "expose all" list.
	assert!(enabled[1].tools().is_empty());
	// Non-tool fields survive the rewrite.
	assert_eq!(enabled[1].url(), Some("https://x"));
}

#[test]
fn a_referenced_server_with_no_matching_pattern_is_dropped() {
	let registry = vec![
		McpServerConfig::builtin("core", 30, tools(&["read"])),
		McpServerConfig::builtin("secret", 30, tools(&["exfiltrate"])),
	];
	let enabled = role(&["core", "secret"], &["core:read"]).get_enabled_servers(&registry, None);
	assert_eq!(enabled.len(), 1, "restricted role must not keep 'secret'");
	assert_eq!(enabled[0].name(), "core");
}

#[test]
fn auto_bound_servers_are_added_once_and_only_for_their_role() {
	let registry = vec![
		McpServerConfig::builtin("core", 30, vec![]),
		McpServerConfig::builtin("dev-only", 30, vec![])
			.with_auto_bind(Some(vec!["developer".to_string()])),
	];

	let enabled = role(&["core"], &[]).get_enabled_servers(&registry, Some("developer"));
	let names: Vec<&str> = enabled.iter().map(|s| s.name()).collect();
	assert_eq!(names, ["core", "dev-only"]);

	// Another role does not get it.
	let other = role(&["core"], &[]).get_enabled_servers(&registry, Some("assistant"));
	assert_eq!(other.len(), 1);

	// An explicitly referenced auto-bind server is not duplicated.
	let both = role(&["dev-only"], &[]).get_enabled_servers(&registry, Some("developer"));
	assert_eq!(both.len(), 1);
}

#[test]
fn no_refs_and_no_role_means_no_servers() {
	let registry = vec![McpServerConfig::builtin("core", 30, vec![])
		.with_auto_bind(Some(vec!["developer".to_string()]))];
	assert!(RoleMcpConfig::default()
		.get_enabled_servers(&registry, None)
		.is_empty());
	// With a role, auto-bind still applies even without explicit refs.
	assert_eq!(
		RoleMcpConfig::default()
			.get_enabled_servers(&registry, Some("developer"))
			.len(),
		1
	);
}
