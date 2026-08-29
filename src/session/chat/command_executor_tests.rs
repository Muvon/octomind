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

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn test_command_listing_and_lookup_agree() {
	let config = template_config();
	let commands = list_available_commands(&config, "assistant");
	// The template ships command layers (review/explain/optimize/test)
	assert!(!commands.is_empty(), "template defines command layers");
	for name in &commands {
		assert!(
			command_exists(&config, "assistant", name),
			"listed command {name} must exist"
		);
	}
	assert!(!command_exists(&config, "assistant", "no-such-command"));
}

#[test]
fn test_command_help_names_every_command() {
	let config = template_config();
	let help = get_command_help(&config, "assistant");
	for name in list_available_commands(&config, "assistant") {
		assert!(help.contains(&name), "help must mention {name}: {help}");
	}
}

#[tokio::test]
async fn test_execute_unknown_command_errors() {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	let mut session = crate::session::chat::session::ChatSession::for_tests(Vec::new());
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let result = execute_command_layer(
		"no-such-command",
		"input",
		&mut session,
		&config,
		"assistant",
		rx,
	)
	.await;
	let err = result.expect_err("unknown command must error");
	assert!(err.to_string().contains("not found"), "{err}");
}

// ---------------------------------------------------------------------------
// persist_session_message
// ---------------------------------------------------------------------------

fn read_session_log(path: &std::path::Path) -> String {
	let bytes = std::fs::read(path).expect("session log must be readable");
	let decoded = zstd::decode_all(std::io::Cursor::new(&bytes)).expect("session log is zstd");
	String::from_utf8(decoded).expect("session log is UTF-8")
}

#[test]
fn test_persist_session_message_writes_json_line() {
	let dir = tempfile::tempdir().unwrap();
	let file = dir.path().join("session.jsonl");

	let message = crate::session::Message {
		role: "user".to_string(),
		content: "hello from the test".to_string(),
		..Default::default()
	};
	persist_session_message(&file, &message);

	let log = read_session_log(&file);
	let line = log.lines().next().unwrap_or_default();
	let parsed: serde_json::Value = serde_json::from_str(line).expect("persisted line is JSON");
	assert_eq!(parsed["role"], "user");
	assert_eq!(parsed["content"], "hello from the test");
}

#[test]
fn test_persist_session_message_appends_multiple_lines() {
	let dir = tempfile::tempdir().unwrap();
	let file = dir.path().join("session.jsonl");

	persist_session_message(
		&file,
		&crate::session::Message {
			role: "user".to_string(),
			content: "first".to_string(),
			..Default::default()
		},
	);
	persist_session_message(
		&file,
		&crate::session::Message {
			role: "assistant".to_string(),
			content: "second".to_string(),
			..Default::default()
		},
	);

	let log = read_session_log(&file);
	let lines: Vec<&str> = log.lines().collect();
	assert_eq!(lines.len(), 2, "two persists must produce two lines: {log}");
	assert!(lines[0].contains("\"first\""));
	assert!(lines[1].contains("\"second\""));
}

#[test]
fn test_persist_session_message_failure_is_best_effort() {
	// Parent directory does not exist — the append must fail silently
	// (logged, never panic) and leave no file behind.
	let dir = tempfile::tempdir().unwrap();
	let file = dir.path().join("missing-dir").join("session.jsonl");

	persist_session_message(
		&file,
		&crate::session::Message {
			role: "user".to_string(),
			content: "doomed".to_string(),
			..Default::default()
		},
	);

	assert!(!file.exists(), "no file must be created on append failure");
}

// ---------------------------------------------------------------------------
// execute_command_layer — full ACP round trip against a fake ACP server
// ---------------------------------------------------------------------------

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

/// Config from the default template plus one `fakecmd` command layer whose
/// `command` runs the given program. `mode` is the output_mode under test.
#[cfg(unix)]
fn config_with_command(mode: &str, command: &str) -> Config {
	let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
	let fixture = format!(
		"\n[[commands]]\n\
			 name = \"fakecmd\"\n\
			 description = \"Fake ACP command for tests\"\n\
			 command = \"{escaped}\"\n\
			 workdir = \".\"\n\
			 input_mode = \"last\"\n\
			 output_mode = \"{mode}\"\n\
			 output_role = \"assistant\"\n"
	);
	let mut src = include_str!("../../../config-templates/default.toml").to_string();
	src.push_str(&fixture);
	let mut config: Config = toml::from_str(&src).expect("parse test config with fakecmd");
	config.build_role_map();
	config
}

#[cfg(unix)]
fn session_with_file(
	messages: Vec<crate::session::Message>,
) -> (
	crate::session::chat::session::ChatSession,
	tempfile::TempDir,
) {
	let dir = tempfile::tempdir().unwrap();
	let mut session = crate::session::chat::session::ChatSession::for_tests(messages);
	session.session.session_file = Some(dir.path().join("session.jsonl"));
	(session, dir)
}

#[cfg(unix)]
fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_command_layer_output_mode_none_leaves_session_untouched() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let config = config_with_command("none", &format!("/bin/sh {}", script.display()));

	let (mut session, _keep) = session_with_file(vec![message("user", "question")]);
	let before = session.session.messages.len();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let output = execute_command_layer(
		"fakecmd",
		"do the thing",
		&mut session,
		&config,
		"assistant",
		rx,
	)
	.await
	.expect("fake ACP command must succeed");

	assert_eq!(output, "FAKE LAYER OUTPUT");
	assert_eq!(
		session.session.messages.len(),
		before,
		"output_mode none must not touch session messages"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_command_layer_output_mode_append_persists_message() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let config = config_with_command("append", &format!("/bin/sh {}", script.display()));

	let (mut session, _keep) = session_with_file(vec![message("user", "question")]);
	let session_file = session.session.session_file.clone().unwrap();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let output = execute_command_layer(
		"fakecmd",
		"do the thing",
		&mut session,
		&config,
		"assistant",
		rx,
	)
	.await
	.expect("fake ACP command must succeed");

	assert_eq!(output, "FAKE LAYER OUTPUT");
	assert_eq!(session.session.messages.len(), 2, "append adds one message");
	let last = session.session.messages.last().unwrap();
	assert_eq!(last.role, "assistant");
	assert_eq!(last.content, "FAKE LAYER OUTPUT");

	// The output was persisted to the session log and the session was saved
	let log = read_session_log(&session_file);
	assert!(
		log.contains("FAKE LAYER OUTPUT"),
		"session log must contain the appended output"
	);
	assert!(
		log.contains("COMMAND_EXEC"),
		"session log must contain the COMMAND_EXEC entry"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_command_layer_output_mode_replace_rebuilds_session() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let config = config_with_command("replace", &format!("/bin/sh {}", script.display()));

	let (mut session, _keep) = session_with_file(vec![
		message("system", "sys-prompt"),
		message("user", "old-question"),
	]);
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let output = execute_command_layer(
		"fakecmd",
		"do the thing",
		&mut session,
		&config,
		"assistant",
		rx,
	)
	.await
	.expect("fake ACP command must succeed");

	assert_eq!(output, "FAKE LAYER OUTPUT");
	// System message survives the rebuild, the old user message does not,
	// and the command output is the final message.
	assert_eq!(
		session.session.messages.first().map(|m| m.role.as_str()),
		Some("system"),
		"system message must be preserved by replace"
	);
	assert_eq!(
		session.session.messages.first().map(|m| m.content.as_str()),
		Some("sys-prompt")
	);
	assert_eq!(
		session.session.messages.last().map(|m| m.content.as_str()),
		Some("FAKE LAYER OUTPUT")
	);
	assert!(
		!session
			.session
			.messages
			.iter()
			.any(|m| m.content == "old-question"),
		"replace must drop prior non-system messages"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_command_layer_output_mode_last_appends_single_message() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let config = config_with_command("last", &format!("/bin/sh {}", script.display()));

	let (mut session, _keep) = session_with_file(vec![message("user", "question")]);
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let output = execute_command_layer(
		"fakecmd",
		"do the thing",
		&mut session,
		&config,
		"assistant",
		rx,
	)
	.await
	.expect("fake ACP command must succeed");

	assert_eq!(output, "FAKE LAYER OUTPUT");
	assert_eq!(
		session.session.messages.len(),
		2,
		"last appends one message"
	);
	let last = session.session.messages.last().unwrap();
	assert_eq!(last.role, "assistant");
	assert_eq!(last.content, "FAKE LAYER OUTPUT");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_command_layer_output_mode_restart_clears_history() {
	let dir = tempfile::tempdir().unwrap();
	let script = write_fake_acp_server(dir.path());
	let config = config_with_command("restart", &format!("/bin/sh {}", script.display()));

	let (mut session, _keep) = session_with_file(vec![
		message("system", "sys-prompt"),
		message("user", "old-question"),
	]);
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let output = execute_command_layer(
		"fakecmd",
		"do the thing",
		&mut session,
		&config,
		"assistant",
		rx,
	)
	.await
	.expect("fake ACP command must succeed");

	assert_eq!(output, "FAKE LAYER OUTPUT");
	assert_eq!(
		session.session.messages.len(),
		1,
		"restart leaves exactly the last output message"
	);
	let only = &session.session.messages[0];
	assert_eq!(only.role, "assistant");
	assert_eq!(only.content, "FAKE LAYER OUTPUT");
}

#[cfg(unix)]
#[tokio::test]
async fn test_execute_command_layer_missing_program_errors() {
	// The command references a program that cannot spawn — the error must
	// propagate out of execute_command_layer (after the COMMAND_EXEC and
	// COMMAND_INPUT log entries were written).
	let config = config_with_command("none", "octomind-test-no-such-program-xyz");

	let (mut session, _keep) = session_with_file(Vec::new());
	let session_file = session.session.session_file.clone().unwrap();
	let (_tx, rx) = tokio::sync::watch::channel(false);

	let result =
		execute_command_layer("fakecmd", "input", &mut session, &config, "assistant", rx).await;

	let err = result.expect_err("missing program must error");
	assert!(
		err.to_string().contains("No such file")
			|| err.to_string().contains("not found")
			|| err.to_string().contains("Permission denied"),
		"unexpected error: {err}"
	);

	let log = read_session_log(&session_file);
	assert!(
		log.contains("COMMAND_INPUT"),
		"input log entry must be written before the spawn failure"
	);
}
