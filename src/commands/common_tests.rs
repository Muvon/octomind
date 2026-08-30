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
use octomind::config::McpServerConfig;

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn make_spinner_starts_unfinished_and_finishes_cleanly() {
	let spinner = make_spinner();
	assert!(!spinner.is_finished());
	spinner.set_message("working");
	spinner.finish_and_clear();
	assert!(spinner.is_finished());
}

#[test]
fn make_spinner_renders_without_a_terminal() {
	// The test harness pipes stdout — drawing must still be safe.
	let spinner = make_spinner();
	spinner.tick();
	assert!(!spinner.is_finished());
	spinner.finish_and_clear();
	assert!(spinner.is_finished());
}

#[tokio::test]
#[serial_test::serial]
async fn startup_non_interactive_plain_role_succeeds() {
	let config = template_config();
	let (run_config, role) = startup(Some("assistant"), &config, false)
		.await
		.expect("plain role resolves and MCP init returns Ok");
	assert_eq!(role, "assistant");
	assert!(!run_config.model.is_empty());
}

#[tokio::test]
async fn startup_non_interactive_malformed_tap_tag_errors() {
	let config = template_config();
	let result = startup(Some("no-such-category:"), &config, false).await;
	assert!(result.is_err());
}

#[tokio::test]
#[serial_test::serial]
async fn startup_interactive_plain_role_succeeds() {
	let config = template_config();
	let (_run_config, role) = startup(Some("assistant"), &config, true)
		.await
		.expect("interactive startup clears the spinner and returns the role");
	assert_eq!(role, "assistant");
}

#[tokio::test]
async fn startup_interactive_malformed_tap_tag_errors() {
	let config = template_config();
	let result = startup(Some("no-such-category:"), &config, true).await;
	assert!(result.is_err());
}

#[tokio::test]
#[serial_test::serial]
async fn startup_mcp_only_non_interactive_initializes_role() {
	let config = template_config();
	startup_mcp_only("assistant", &config, false)
		.await
		.expect("MCP init is tolerant of per-server failures and returns Ok");
}

#[tokio::test]
#[serial_test::serial]
async fn startup_mcp_only_interactive_tracks_external_server_progress() {
	let mut config = template_config();
	// Bind an external HTTP server that refuses connections instantly so
	// the spinner callback observes a non-empty Starting list and a
	// Completed event (success: false) without any real MCP endpoint.
	config.mcp.servers.push(McpServerConfig::http(
		"stub-unreachable",
		"http://127.0.0.1:1/mcp",
		2,
		Vec::new(),
	));
	config
		.role_map
		.get_mut("assistant")
		.expect("assistant role exists in the template")
		.mcp
		.server_refs
		.push("stub-unreachable".to_string());

	startup_mcp_only("assistant", &config, true)
		.await
		.expect("interactive MCP init succeeds even with an unreachable external server");
}
