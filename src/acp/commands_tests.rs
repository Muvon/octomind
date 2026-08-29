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

//! Tests for the ACP `octomind/command` extension surface: request/response
//! (de)serialization, namespace routing, and `execute_command` driving the
//! real `process_command` dispatcher with an in-memory session.

use super::*;
use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::chat::session::ChatSession;
use agent_client_protocol::schema::v1::{ExtRequest, ExtResponse};
use serde_json::value::RawValue;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

type Sessions = Rc<RefCell<HashMap<String, (ChatSession, std::path::PathBuf)>>>;
type Cancellations = Rc<RefCell<HashMap<String, SessionCancellation>>>;
type Locks = crate::acp::SessionLocks;

/// The full ACP command harness with one live in-memory session ("s1").
fn harness() -> (Sessions, Locks, RefCell<Config>, Cancellations) {
	let mut sessions = HashMap::new();
	sessions.insert(
		"s1".to_string(),
		(
			ChatSession::for_tests(Vec::new()),
			std::env::current_dir().expect("cwd"),
		),
	);
	(
		Rc::new(RefCell::new(sessions)),
		Rc::new(RefCell::new(HashMap::new())),
		RefCell::new(template_config()),
		Rc::new(RefCell::new(HashMap::new())),
	)
}

fn ext_request(method: &str, params: serde_json::Value) -> ExtRequest {
	let raw = RawValue::from_string(params.to_string()).expect("raw params");
	ExtRequest::new(method, std::sync::Arc::from(raw))
}

/// Decode an ext response body to plain JSON — `CommandResponse` itself is
/// serialize-only, so assertions read the wire shape instead.
fn decode(response: ExtResponse) -> serde_json::Value {
	serde_json::from_str(response.0.get()).expect("decode ext response")
}

#[test]
fn command_request_deserializes_and_defaults_args() {
	let req: CommandRequest =
		serde_json::from_str(r#"{"session_id": "s1", "command": "/help"}"#).expect("parse");
	assert_eq!(req.session_id, "s1");
	assert_eq!(req.command, "/help");
	assert!(req.args.is_empty(), "args default to an empty vec");

	let req: CommandRequest = serde_json::from_str(
		r#"{"session_id": "s1", "command": "/model", "args": ["ollama:x", "extra"]}"#,
	)
	.expect("parse with args");
	assert_eq!(req.args, vec!["ollama:x".to_string(), "extra".to_string()]);

	assert!(serde_json::from_str::<CommandRequest>(r#"{"command": "/help"}"#).is_err());
	assert!(serde_json::from_str::<CommandRequest>(r#"{"session_id": "s1"}"#).is_err());
}

#[test]
fn command_response_serializes_its_three_shapes() {
	let ok = CommandResponse {
		success: true,
		output: None,
		error: None,
	};
	assert_eq!(
		serde_json::to_value(&ok).unwrap(),
		serde_json::json!({"success": true, "output": null, "error": null})
	);

	let out = CommandResponse {
		success: true,
		output: Some(serde_json::json!({"action": "exit"})),
		error: None,
	};
	assert_eq!(
		serde_json::to_value(&out).unwrap()["output"]["action"],
		"exit"
	);

	let err = CommandResponse {
		success: false,
		output: None,
		error: Some("boom".into()),
	};
	assert_eq!(serde_json::to_value(&err).unwrap()["error"], "boom");
}

#[tokio::test]
async fn handle_ext_method_rejects_foreign_namespaces() {
	let (sessions, locks, config, cancellations) = harness();
	let result = handle_ext_method(
		ext_request(
			"other/namespace",
			serde_json::json!({"session_id": "s1", "command": "/help"}),
		),
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(result.is_err(), "only octomind/command is routed here");
}

#[tokio::test]
async fn handle_ext_method_reports_invalid_params_as_a_failed_command() {
	let (sessions, locks, config, cancellations) = harness();
	let response = handle_ext_method(
		ext_request(COMMAND_NAMESPACE, serde_json::json!({"session_id": 42})),
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await
	.expect("invalid params still answer 200 with a failed payload");

	let decoded = decode(response);
	assert_eq!(decoded["success"], false, "{decoded}");
	let error = decoded["error"].as_str().expect("error set");
	assert!(error.contains("Invalid request"), "{decoded}");
}

#[tokio::test]
async fn execute_command_errors_on_an_unknown_session() {
	let (sessions, locks, config, cancellations) = harness();
	let request = CommandRequest {
		session_id: "nope".to_string(),
		command: "/help".to_string(),
		args: Vec::new(),
	};
	let response = execute_command(
		&request,
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(!response.success);
	let error = response.error.expect("error set");
	assert!(error.contains("session not found: nope"), "{error}");
	// The live session must not have been collateral damage.
	assert!(sessions.borrow().contains_key("s1"));
}

#[tokio::test]
async fn execute_command_runs_help_and_returns_the_session_to_the_map() {
	let (sessions, locks, config, cancellations) = harness();
	let request = CommandRequest {
		session_id: "s1".to_string(),
		command: "/help".to_string(),
		args: Vec::new(),
	};

	let first = execute_command(
		&request,
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(first.success, "{first:?}");
	assert!(
		sessions.borrow().contains_key("s1"),
		"session reinserted after the command"
	);

	// The reinserted session is usable again — a second command succeeds.
	let second = execute_command(
		&request,
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(second.success, "{second:?}");
}

#[tokio::test]
async fn execute_command_maps_exit_to_an_exit_action() {
	let (sessions, locks, config, cancellations) = harness();
	let request = CommandRequest {
		session_id: "s1".to_string(),
		command: "/exit".to_string(),
		args: Vec::new(),
	};
	let response = execute_command(
		&request,
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(response.success, "{response:?}");
	assert_eq!(
		response.output.expect("exit carries an action"),
		serde_json::json!({"action": "exit"})
	);
}

#[tokio::test]
async fn execute_command_reports_unknown_commands() {
	let (sessions, locks, config, cancellations) = harness();
	let request = CommandRequest {
		session_id: "s1".to_string(),
		command: "/definitely-not-a-command".to_string(),
		args: Vec::new(),
	};
	let response = execute_command(
		&request,
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(!response.success);
	assert_eq!(
		response.error.expect("error set"),
		"Unknown command: /definitely-not-a-command"
	);
}

#[tokio::test]
async fn execute_command_joins_args_and_persists_config_mutations() {
	let (sessions, locks, config, cancellations) = harness();
	let request = CommandRequest {
		session_id: "s1".to_string(),
		command: "/loglevel".to_string(),
		args: vec!["debug".to_string()],
	};
	let response = execute_command(
		&request,
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await;
	assert!(response.success, "{response:?}");
	// The command mutated its config clone; execute_command must write it
	// back to the shared RefCell or the change is silently dropped.
	assert!(
		config.borrow().get_log_level().is_debug_enabled(),
		"config mutation persisted to the shared config"
	);
}

#[tokio::test]
async fn handle_ext_method_end_to_end_through_the_dispatcher() {
	let (sessions, locks, config, cancellations) = harness();
	let response = handle_ext_method(
		ext_request(
			COMMAND_NAMESPACE,
			serde_json::json!({"session_id": "s1", "command": "/exit"}),
		),
		&sessions,
		&locks,
		&config,
		"assistant",
		&cancellations,
	)
	.await
	.expect("ext method handled");

	let decoded = decode(response);
	assert_eq!(decoded["success"], true, "{decoded}");
	assert_eq!(decoded["output"], serde_json::json!({"action": "exit"}));
}
