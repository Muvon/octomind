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

//! ChatSession surface methods: attachments, message-range removal,
//! compressed-knowledge insertion and builder wiring.

use super::*;

fn test_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
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

async fn initialized_named_session(name: &str) -> ChatSession {
	let config = test_config();
	let params = SessionInitParams::new(&config, "assistant").with_name(name.to_string());
	let mut session = ChatSession::initialize(params)
		.await
		.expect("create named session");
	session.add_user_message("first question").unwrap();
	session
		.add_assistant_message("first answer", None, &config, "assistant")
		.unwrap();
	session.save().unwrap();
	session
}

#[test]
fn test_init_params_builder_wiring() {
	let config = test_config();
	let params = SessionInitParams::new(&config, "assistant")
		.with_name("build-test".to_string())
		.with_model("ollama:fake".to_string())
		.with_temperature(0.1)
		.with_max_tokens(2048)
		.with_max_retries(5)
		.with_output_mode("plain".to_string())
		.with_schema(serde_json::json!({"type": "object"}));
	assert_eq!(params.name.as_deref(), Some("build-test"));
	assert_eq!(params.model.as_deref(), Some("ollama:fake"));
	assert_eq!(params.max_retries, Some(5));
	assert!(params.schema.is_some());
}

#[test]
fn test_effective_model_and_counts() {
	let mut session = ChatSession::for_tests(vec![
		message("user", "q1"),
		message("assistant", "a1"),
		message("user", "q2"),
	]);
	assert_eq!(session.get_effective_model(), "anthropic/claude-3-5-sonnet");
	assert_eq!(session.get_message_count(), 3);
	session.invalidate_tool_cache();
}

#[test]
fn test_pending_attachment_take_semantics() {
	let mut session = ChatSession::for_tests(Vec::new());
	assert!(!session.has_pending_image());
	assert!(session.take_pending_image().is_none());
	assert!(!session.has_pending_video());
	assert!(session.take_pending_video().is_none());
}

#[tokio::test]
async fn test_attach_image_from_missing_path_errors() {
	let mut session = ChatSession::for_tests(Vec::new());
	assert!(session
		.attach_image_from_path("/definitely/not/here.png")
		.await
		.is_err());
	assert!(!session.has_pending_image());
	assert!(session
		.attach_video_from_path("/definitely/not/here.mp4")
		.await
		.is_err());
}

#[test]
fn test_remove_messages_in_range() {
	let mut session = ChatSession::for_tests(vec![
		message("user", "m0"),
		message("assistant", "m1"),
		message("user", "m2"),
		message("assistant", "m3"),
	]);
	// Removes start+1..=end: the range anchor at index 0 survives
	let (removed, had_cached) = session
		.remove_messages_in_range(0, 2)
		.expect("range removal");
	assert_eq!(removed, 2);
	assert!(!had_cached);
	assert_eq!(session.get_message_count(), 2);
	assert_eq!(session.session.messages[0].content, "m0");
	assert_eq!(session.session.messages[1].content, "m3");

	// Out-of-bounds and inverted ranges fail instead of silently truncating
	assert!(session.remove_messages_in_range(5, 9).is_err());
	assert!(session.remove_messages_in_range(1, 1).is_err());
}

#[test]
fn test_insert_compressed_knowledge() {
	let mut session = ChatSession::for_tests(vec![message("user", "task")]);
	session
		.insert_compressed_knowledge(0, "critical: build on the box".to_string())
		.expect("insert knowledge");
	assert!(session.get_message_count() >= 2);
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_creates_new_session_and_persists_messages() {
	let _data = DataDirGuard::new();
	let config = test_config();
	let params = SessionInitParams::new(&config, "assistant");
	let mut session = ChatSession::initialize(params)
		.await
		.expect("fresh session initializes");

	assert!(!session.was_resumed);
	assert!(session.session.session_file.is_some());

	session.add_user_message("hello").unwrap();
	session.save().unwrap();
	assert!(
		session.session.session_file.as_ref().unwrap().exists(),
		"first message write must create the session file"
	);
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_resume_missing_session_errors() {
	let _data = DataDirGuard::new();
	let config = test_config();
	let params =
		SessionInitParams::new(&config, "assistant").with_resume("definitely-not-here".to_string());

	let error = ChatSession::initialize(params)
		.await
		.err()
		.expect("resuming a non-existent session must fail");
	assert!(error.to_string().contains("not found"), "{error}");
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_named_existing_session_resumes_transcript() {
	let _data = DataDirGuard::new();
	let first = initialized_named_session("core-init-resume").await;
	let original_name = first.session.info.name.clone();

	let config = test_config();
	let params =
		SessionInitParams::new(&config, "assistant").with_name("core-init-resume".to_string());
	let session = ChatSession::initialize(params)
		.await
		.expect("named existing session resumes");

	assert!(session.was_resumed);
	assert_eq!(session.session.info.name, original_name);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.role == "user" && m.content.contains("first question")),
		"resumed transcript must contain the persisted user message"
	);
	assert_eq!(
		session.last_response, "first answer",
		"resume must seed last_response from the final assistant message"
	);
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_explicit_resume_of_corrupted_file_errors() {
	let _data = DataDirGuard::new();
	let sessions_dir = crate::session::persistence::get_sessions_dir().unwrap();
	let corrupted = sessions_dir.join("core-init-corrupt.jsonl.zst");
	std::fs::write(&corrupted, b"this is not a zstd stream").unwrap();

	let config = test_config();
	let params =
		SessionInitParams::new(&config, "assistant").with_resume("core-init-corrupt".to_string());

	let error = ChatSession::initialize(params)
		.await
		.err()
		.expect("explicit resume of a corrupted file must fail");
	assert!(
		error.to_string().contains("Failed to load session"),
		"{error}"
	);
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_named_corrupted_file_falls_back_to_new_session() {
	let _data = DataDirGuard::new();
	let sessions_dir = crate::session::persistence::get_sessions_dir().unwrap();
	let corrupted = sessions_dir.join("core-init-fallback.jsonl.zst");
	std::fs::write(&corrupted, b"this is not a zstd stream").unwrap();

	let config = test_config();
	let params =
		SessionInitParams::new(&config, "assistant").with_name("core-init-fallback".to_string());
	let session = ChatSession::initialize(params)
		.await
		.expect("unnamed load failure falls back to a fresh session");

	assert!(!session.was_resumed);
	assert_ne!(
		session.session.info.name, "core-init-fallback",
		"fallback must generate a new unique session name"
	);
	assert!(session.session.messages.is_empty());
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_resume_recent_without_match_creates_new() {
	let _data = DataDirGuard::new();
	let config = test_config();
	let params = SessionInitParams::new(&config, "assistant").with_resume_recent(true);
	let session = ChatSession::initialize(params)
		.await
		.expect("no recent session → create a new one");

	assert!(!session.was_resumed);
	assert!(session.session.session_file.is_some());
}

#[serial_test::serial]
#[tokio::test]
async fn initialize_resume_recent_finds_project_session() {
	let _data = DataDirGuard::new();
	// Session names embed the project basename as a dash-delimited segment
	// (`find_most_recent_session_for_project` matches `-{basename}-`).
	let basename = std::env::current_dir()
		.expect("cwd")
		.file_name()
		.and_then(|n| n.to_str())
		.expect("cwd basename")
		.to_string();
	let crafted_name = format!("991231-{basename}-2359-abcd");
	let first = initialized_named_session(&crafted_name).await;
	assert_eq!(first.session.info.name, crafted_name);

	let config = test_config();
	let params = SessionInitParams::new(&config, "assistant").with_resume_recent(true);
	let session = ChatSession::initialize(params)
		.await
		.expect("resume_recent picks up the project session");

	assert!(session.was_resumed);
	assert_eq!(session.session.info.name, crafted_name);
}
