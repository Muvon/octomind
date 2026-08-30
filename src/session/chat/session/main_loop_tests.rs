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
use crate::session::chat::test_support::{fake_provider_config, spawn_stub, ENV_LOCK};
use crate::session::Message;

fn msg(role: &str) -> Message {
	Message {
		role: role.to_string(),
		..Default::default()
	}
}

fn msgs(roles: &[&str]) -> Vec<Message> {
	roles.iter().map(|r| msg(r)).collect()
}

fn template_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn session_args() -> super::super::GenericSessionArgs {
	super::super::GenericSessionArgs::new("assistant".to_string())
}

/// Sandbox `OCTOMIND_DATA_DIR` at a fresh tempdir for the guard's lifetime.
/// Tests using it must stay `#[serial]` — env vars are process-global.
struct DataDirGuard {
	_dir: tempfile::TempDir,
	previous: Option<std::ffi::OsString>,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().unwrap();
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			_dir: dir,
			previous,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// The single `.jsonl.zst` file in the sandboxed sessions dir — e2e run tests
/// create exactly one session each.
fn sole_session_file() -> std::path::PathBuf {
	let dir = crate::session::persistence::get_sessions_dir().expect("sessions dir");
	let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
		.expect("read sessions dir")
		.filter_map(|entry| entry.ok().map(|entry| entry.path()))
		.filter(|path| path.extension().is_some_and(|ext| ext == "zst"))
		.collect();
	files.sort();
	assert_eq!(
		files.len(),
		1,
		"expected exactly one session file, got {files:?}"
	);
	files.pop().unwrap()
}

#[test]
fn test_first_call_truncates_to_user_message() {
	// User message added, API call interrupted before any tool ran →
	// remove the user message for a clean retry.
	let messages = msgs(&["system", "user"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(1)), Some(1));

	// Assistant text may already be streaming — still no tools → truncate.
	let messages = msgs(&["system", "user", "assistant"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(1)), Some(1));
}

#[test]
fn test_multiturn_with_tools_preserves_everything() {
	// Tool results after the user message: truncating would orphan the
	// assistant(tool_calls) + tool_result pairing the API already accepted.
	let messages = msgs(&["system", "user", "assistant", "tool"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(1)), None);
}

#[test]
fn test_tools_from_previous_turns_do_not_count() {
	// A tool message BEFORE this operation's user message belongs to a prior
	// turn — the current operation is still a clean first call.
	let messages = msgs(&["user", "assistant", "tool", "user"]);
	assert_eq!(interrupted_call_truncation(&messages, Some(3)), Some(3));
}

#[test]
fn test_missing_or_stale_index_preserves_state() {
	let messages = msgs(&["system", "user"]);
	// No operation context → nothing to truncate
	assert_eq!(interrupted_call_truncation(&messages, None), None);
	// Index at/past the end (already rolled back elsewhere) → no-op
	assert_eq!(interrupted_call_truncation(&messages, Some(2)), None);
	assert_eq!(interrupted_call_truncation(&messages, Some(99)), None);
	// Empty session
	assert_eq!(interrupted_call_truncation(&[], Some(0)), None);
}

#[test]
fn test_clipboard_image_refused_for_known_non_vision_model() {
	use crate::session::chat::reedline_adapter::PendingClipboardItem;
	use crate::session::image::{ImageAttachment, ImageData, SourceType};

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();
	let attachment = ImageAttachment {
		data: ImageData::Base64("unused".to_string()),
		media_type: "image/png".to_string(),
		source_type: SourceType::Clipboard,
		dimensions: Some((1, 1)),
		size_bytes: None,
	};

	apply_clipboard_items(&mut session, vec![PendingClipboardItem::Image(attachment)]);
	assert!(!session.has_pending_image());
}

#[test]
fn test_clipboard_image_attached_for_unknown_proxy_model() {
	use crate::session::chat::reedline_adapter::PendingClipboardItem;
	use crate::session::image::{ImageAttachment, ImageData, SourceType};

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/totally-unknown-model-xyz".to_string();
	let attachment = ImageAttachment {
		data: ImageData::Base64("unused".to_string()),
		media_type: "image/png".to_string(),
		source_type: SourceType::Clipboard,
		dimensions: Some((1, 1)),
		size_bytes: None,
	};

	apply_clipboard_items(&mut session, vec![PendingClipboardItem::Image(attachment)]);
	assert!(session.has_pending_image());
}

#[test]
fn test_clipboard_video_refused_for_known_non_video_model() {
	use crate::session::chat::reedline_adapter::PendingClipboardItem;
	use crate::session::video::{SourceType, VideoAttachment, VideoData};

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();
	let attachment = VideoAttachment {
		data: VideoData::Base64("unused".to_string()),
		media_type: "video/mp4".to_string(),
		source_type: SourceType::Clipboard,
		dimensions: Some((1, 1)),
		size_bytes: None,
		duration_secs: None,
	};

	apply_clipboard_items(&mut session, vec![PendingClipboardItem::Video(attachment)]);
	assert!(!session.has_pending_video());
}

#[test]
fn test_clipboard_video_attached_for_unknown_proxy_model() {
	use crate::session::chat::reedline_adapter::PendingClipboardItem;
	use crate::session::video::{SourceType, VideoAttachment, VideoData};

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/totally-unknown-model-xyz".to_string();
	let attachment = VideoAttachment {
		data: VideoData::Base64("unused".to_string()),
		media_type: "video/mp4".to_string(),
		source_type: SourceType::Clipboard,
		dimensions: Some((1, 1)),
		size_bytes: None,
		duration_secs: None,
	};

	apply_clipboard_items(&mut session, vec![PendingClipboardItem::Video(attachment)]);
	assert!(session.has_pending_video());
}

#[test]
fn test_telemetry_context_reports_resume_sandbox_and_server_count() {
	let config = template_config();
	let expected = (false, config.sandbox, config.mcp.servers.len() as u32);
	assert_eq!(telemetry_context(&session_args(), &config), expected);

	// Either resume flavor marks the session as resumed for telemetry.
	let mut args = session_args();
	args.resume = Some("some-session".to_string());
	assert_eq!(telemetry_context(&args, &config).0, true);

	let mut args = session_args();
	args.resume_recent = true;
	assert_eq!(telemetry_context(&args, &config).0, true);
}

#[test]
fn test_record_session_telemetry_smoke() {
	// Buffers (or drops, under DNT) one session-end row without panicking.
	let session = ChatSession::for_tests(Vec::new());
	record_session_telemetry(&session, "piped", false, false, 0);
}

#[tokio::test]
async fn test_start_webhook_guards_rejects_unknown_hook() {
	let config = template_config();
	let mut args = session_args();
	args.hooks = vec!["missing-hook".to_string()];

	let error = start_webhook_guards(&args, &config, "test-session")
		.await
		.err()
		.expect("unknown hook must fail fast");
	assert!(error.to_string().contains("not found"), "{error}");
}

#[tokio::test]
async fn test_start_webhook_guards_starts_listener_for_valid_hook() {
	let script_dir = tempfile::tempdir().unwrap();
	let script_path = script_dir.path().join("hook.sh");
	std::fs::write(&script_path, "#!/bin/sh\nexit 0\n").unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
	}

	let mut config = template_config();
	config.hooks = vec![crate::config::HookConfig {
		name: "test-hook".to_string(),
		bind: "127.0.0.1:0".to_string(),
		script: script_path.to_string_lossy().to_string(),
		timeout: 5,
	}];
	let mut args = session_args();
	args.hooks = vec!["test-hook".to_string()];

	let guards = start_webhook_guards(&args, &config, "test-session")
		.await
		.expect("valid hook starts a listener");
	assert_eq!(guards.len(), 1);
	// Dropping the guards stops the listener again.
}

#[serial_test::serial]
#[tokio::test]
async fn test_init_session_runtime_without_hooks() {
	let _data = DataDirGuard::new();
	let config = template_config();
	let args = session_args();
	let chat_session = ChatSession::for_tests(Vec::new());
	let sid = chat_session.session.info.name.clone();

	crate::session::context::with_session_id(sid, async {
		let _guards = init_session_runtime(&args, &config, &chat_session, "assistant")
			.await
			.expect("runtime boots without hooks");
	})
	.await;
}

#[serial_test::serial]
#[tokio::test]
async fn test_run_session_with_input_plain_turn() {
	let _guard = ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();

	let url = spawn_stub(vec![crate::session::chat::test_support::final_response(
		"Hello from stub",
	)])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	run_interactive_session_with_input(&session_args(), &config, "hi")
		.await
		.expect("plain turn completes");

	let loaded =
		crate::session::persistence::load_session(&sole_session_file()).expect("session persisted");
	assert!(
		loaded
			.messages
			.iter()
			.any(|m| m.role == "user" && m.content.contains("hi")),
		"user input must be persisted"
	);
	assert!(
		loaded
			.messages
			.iter()
			.any(|m| m.role == "assistant" && m.content.contains("Hello from stub")),
		"stub reply must be persisted"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn test_run_session_with_input_done_command_exits_cleanly() {
	let _guard = ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();

	// No stub needed: /done on a fresh session has nothing to compress and
	// must return before any API call.
	let config = fake_provider_config();
	run_interactive_session_with_input(&session_args(), &config, "/done")
		.await
		.expect("bare /done exits cleanly");

	let loaded = crate::session::persistence::load_session(&sole_session_file())
		.expect("session persisted after /done");
	assert!(
		loaded.messages.iter().all(|m| m.role != "user"),
		"bare /done must not add a user message"
	);
}

#[serial_test::serial]
#[tokio::test]
async fn test_run_session_with_input_done_with_instructions_processes_turn() {
	let _guard = ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();

	let url = spawn_stub(vec![crate::session::chat::test_support::final_response(
		"Wrapped up",
	)])
	.await;
	std::env::set_var("OLLAMA_API_URL", &url);

	let config = fake_provider_config();
	run_interactive_session_with_input(&session_args(), &config, "/done wrap up")
		.await
		.expect("/done with instructions falls through to a normal turn");

	let loaded =
		crate::session::persistence::load_session(&sole_session_file()).expect("session persisted");
	assert!(
		loaded
			.messages
			.iter()
			.any(|m| m.role == "user" && m.content.contains("wrap up")),
		"trailing instructions must become the next user message"
	);
	assert!(
		loaded
			.messages
			.iter()
			.any(|m| m.role == "assistant" && m.content.contains("Wrapped up")),
		"the post-/done turn must reach the model"
	);

	std::env::remove_var("OLLAMA_API_URL");
}

#[serial_test::serial]
#[tokio::test]
async fn test_run_session_with_input_info_command_handled() {
	let _guard = ENV_LOCK.lock().await;
	let _data = DataDirGuard::new();

	// /info is handled as a command: no API call, session saved, clean exit.
	let config = fake_provider_config();
	run_interactive_session_with_input(&session_args(), &config, "/info")
		.await
		.expect("command input is handled without an API call");

	let loaded = crate::session::persistence::load_session(&sole_session_file())
		.expect("session persisted after command");
	assert!(
		loaded.messages.iter().all(|m| m.role != "user"),
		"a handled command must not add a user message"
	);
}

#[tokio::test]
async fn test_print_command_output_jsonl_and_cli_branches() {
	use crate::session::chat::session::commands::CommandOutput;

	let mut session = ChatSession::for_tests(Vec::new());

	// JSONL runtime mode prints the serialized output
	let mut config = template_config();
	config.runtime_output_mode = Some("jsonl".to_string());
	let mut output = CommandOutput::Error {
		error: "boom".to_string(),
		context: None,
	};
	print_command_output(&mut output, &mut session, &config).await;

	// Plain mode renders through the CLI display path
	let config = template_config();
	let mut output = CommandOutput::Error {
		error: "boom".to_string(),
		context: None,
	};
	print_command_output(&mut output, &mut session, &config).await;
}
