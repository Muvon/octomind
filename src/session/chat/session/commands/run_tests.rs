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
