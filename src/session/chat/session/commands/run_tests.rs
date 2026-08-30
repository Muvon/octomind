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

//! Handler-level tests for the `/run` session command: listing, unknown
//! command errors, spending-threshold cancellation, and the execution-failure
//! path (a command layer whose ACP binary cannot spawn).

use super::*;
use crate::session::layers::layer_trait::{InputMode, LayerConfig, OutputMode, OutputRole};

fn command_layer(name: &str, command: &str) -> LayerConfig {
	LayerConfig {
		name: name.to_string(),
		description: format!("{name} command layer"),
		command: command.to_string(),
		workdir: ".".to_string(),
		input_mode: InputMode::Last,
		output_mode: OutputMode::None,
		output_role: OutputRole::Assistant,
	}
}

fn config_with_commands(commands: Option<Vec<LayerConfig>>) -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config.commands = commands;
	config
}

/// Run one `/run` invocation and return `(command_executed, data)`.
async fn run_command(
	session: &mut ChatSession,
	config: &Config,
	params: &[&str],
) -> (String, serde_json::Value) {
	let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
	let result = handle_run(session, config, "assistant", params, cancel_rx)
		.await
		.unwrap_or_else(|e| panic!("run {params:?} errored: {e}"));
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	let CommandOutput::Run {
		command_executed,
		data,
	} = *output
	else {
		panic!("expected Run output");
	};
	(command_executed, data)
}

#[tokio::test]
async fn list_without_params_reports_configured_commands() {
	let config = config_with_commands(Some(vec![command_layer("cov-cmd", "cov-role")]));
	let mut session = ChatSession::for_tests(Vec::new());
	let (executed, data) = run_command(&mut session, &config, &[]).await;

	assert_eq!(executed, "");
	assert_eq!(data["action"], "list");
	assert_eq!(data["commands"], serde_json::json!(["cov-cmd"]));
	assert_eq!(data["message"], "Available commands");
}

#[tokio::test]
async fn list_without_commands_reports_none_configured() {
	let config = config_with_commands(None);
	let mut session = ChatSession::for_tests(Vec::new());
	let (_, data) = run_command(&mut session, &config, &[]).await;

	assert_eq!(data["action"], "list");
	assert_eq!(data["commands"], serde_json::json!([]));
	assert_eq!(data["message"], "No commands configured");
}

#[tokio::test]
async fn unknown_command_reports_error_and_available_commands() {
	let config = config_with_commands(Some(vec![command_layer("cov-cmd", "cov-role")]));
	let mut session = ChatSession::for_tests(Vec::new());
	let (executed, data) = run_command(&mut session, &config, &["bogus"]).await;

	assert_eq!(executed, "bogus");
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], false);
	assert_eq!(data["error"], "Command not found: bogus");
	assert_eq!(data["available_commands"], serde_json::json!(["cov-cmd"]));
}

#[tokio::test]
async fn request_threshold_breach_cancels_execution() {
	let mut config = config_with_commands(Some(vec![command_layer("cov-cmd", "cov-role")]));
	// Disable the interactive session threshold so only the request threshold
	// (which auto-declines without reading stdin) is exercised.
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.01;

	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_cost = 1.0;

	let (executed, data) = run_command(&mut session, &config, &["cov-cmd"]).await;
	assert_eq!(executed, "cov-cmd");
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], false);
	assert_eq!(
		data["error"],
		"Command execution cancelled due to request spending threshold."
	);
}

#[tokio::test]
async fn execution_failure_surfaces_spawn_error() {
	let mut config = config_with_commands(Some(vec![command_layer(
		"cov-cmd",
		"/nonexistent/cov-acp-binary-xyz",
	)]));
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.0;

	let mut session = ChatSession::for_tests(Vec::new());
	// Extra params exercise the explicit-input branch of command selection.
	let (executed, data) = run_command(&mut session, &config, &["cov-cmd", "do", "things"]).await;

	assert_eq!(executed, "cov-cmd");
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], false);
	let error = data["error"].as_str().expect("error string");
	assert!(
		error.starts_with("Command execution failed:"),
		"unexpected error: {error}"
	);
}

// ---------------------------------------------------------------------------
// Success path against a fake ACP server + implicit input selection
// ---------------------------------------------------------------------------

/// A minimal ACP server script: initialize → new_session → one agent message
/// chunk → end_turn. Speaks just enough JSON-RPC for the command layer.
#[cfg(unix)]
fn write_fake_acp_server(dir: &std::path::Path) -> std::path::PathBuf {
	let script = dir.join("fake_acp_server.sh");
	std::fs::write(
		&script,
		r#"#!/bin/sh
read -r _req
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r _req
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"fake-session"}}'
read -r _req
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"FAKE LAYER OUTPUT"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"sessionId":"fake-session","stopReason":"end_turn"}}'
"#,
	)
	.expect("write fake ACP server script");
	use std::os::unix::fs::PermissionsExt;
	std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
		.expect("make fake ACP server executable");
	script
}

#[cfg(unix)]
#[tokio::test]
async fn implicit_input_uses_last_real_user_message_and_succeeds() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let mut config = config_with_commands(Some(vec![command_layer(
		"cov-cmd",
		&format!("/bin/sh {}", script.display()),
	)]));
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.0;

	// No params after the command name: the input falls back to the most
	// recent genuine user message in the session.
	let mut session = ChatSession::for_tests(Vec::new());
	session
		.add_user_message("please review the widget")
		.expect("seed user message");

	let (executed, data) = run_command(&mut session, &config, &["cov-cmd"]).await;
	assert_eq!(executed, "cov-cmd");
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], true, "data: {data}");
	assert_eq!(data["result"], "FAKE LAYER OUTPUT");
}

#[cfg(unix)]
#[tokio::test]
async fn implicit_input_without_any_user_message_uses_default_placeholder() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let mut config = config_with_commands(Some(vec![command_layer(
		"cov-cmd",
		&format!("/bin/sh {}", script.display()),
	)]));
	config.max_session_spending_threshold = 0.0;
	config.max_request_spending_threshold = 0.0;

	// Empty session: there is no user message to reuse, the placeholder input
	// ("No recent user input found") is used and the layer still runs.
	let mut session = ChatSession::for_tests(Vec::new());
	let (executed, data) = run_command(&mut session, &config, &["cov-cmd"]).await;
	assert_eq!(executed, "cov-cmd");
	assert_eq!(data["success"], true, "data: {data}");
	assert_eq!(data["result"], "FAKE LAYER OUTPUT");
}

#[tokio::test]
async fn session_threshold_breach_cancels_execution() {
	let mut config = config_with_commands(Some(vec![command_layer("cov-cmd", "cov-role")]));
	// A session threshold breach auto-declines without reading stdin (the test
	// harness stdin is not a terminal), so no interactive prompt can block.
	config.max_session_spending_threshold = 0.01;
	config.max_request_spending_threshold = 0.0;

	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.total_cost = 1.0;

	let (executed, data) = run_command(&mut session, &config, &["cov-cmd", "do", "things"]).await;
	assert_eq!(executed, "cov-cmd");
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], false);
	assert_eq!(
		data["error"],
		"Command execution cancelled due to spending threshold."
	);
}
