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

// ---------------------------------------------------------------------------
// remove_messages_in_range: remaining error + cached-content branches
// ---------------------------------------------------------------------------

#[test]
fn test_remove_messages_in_range_end_out_of_bounds_errors() {
	let mut session =
		ChatSession::for_tests(vec![message("user", "m0"), message("assistant", "m1")]);
	// start is in bounds but end is past the transcript: must fail, not clamp
	assert!(session.remove_messages_in_range(0, 9).is_err());
	assert_eq!(
		session.get_message_count(),
		2,
		"failed removal must not mutate"
	);
}

#[test]
fn test_remove_messages_in_range_reports_cached_content() {
	let mut cached_msg = message("assistant", "cached turn");
	cached_msg.cached = true;
	let mut session = ChatSession::for_tests(vec![
		message("user", "anchor"),
		cached_msg,
		message("user", "tail"),
	]);
	let (removed, had_cached) = session
		.remove_messages_in_range(0, 1)
		.expect("range removal");
	assert_eq!(removed, 1);
	assert!(
		had_cached,
		"a cached message inside the range must be reported"
	);
}

// ---------------------------------------------------------------------------
// insert_compressed_knowledge: marker bookkeeping
// ---------------------------------------------------------------------------

#[test]
fn test_insert_compressed_knowledge_invalid_index_errors() {
	let mut session = ChatSession::for_tests(vec![message("user", "only")]);
	let err = session
		.insert_compressed_knowledge(5, "knowledge".to_string())
		.expect_err("index past the transcript must fail");
	assert!(err.to_string().contains("Invalid index"), "{err}");
	assert_eq!(session.get_message_count(), 1);
}

#[test]
fn test_insert_compressed_knowledge_evicts_oldest_and_marks_preserved_tail() {
	// Default for_tests model (anthropic/claude-3-5-sonnet) supports caching,
	// so the full marker-management path runs.
	let mut older = message("user", "older marker");
	older.cached = true;
	let mut newer = message("assistant", "newer marker");
	newer.cached = true;
	let mut session = ChatSession::for_tests(vec![
		message("system", "anchor"),
		older,
		newer,
		message("user", "preserved question"),
	]);

	session
		.insert_compressed_knowledge(0, "compressed summary".to_string())
		.expect("insert knowledge");

	// The compressed block landed right after the anchor, cached, named
	let compressed = &session.session.messages[1];
	assert_eq!(compressed.role, "assistant");
	assert_eq!(compressed.content, "compressed summary");
	assert_eq!(compressed.name.as_deref(), Some("plan_compression"));
	assert!(
		compressed.cached,
		"compressed block is the new history boundary"
	);

	// At 2 existing markers the OLDEST was evicted to make room
	assert!(
		!session.session.messages[2].cached,
		"oldest marker must be evicted"
	);

	// The last eligible user/tool message after the block got the 2nd marker
	let tail = session.session.messages.last().expect("tail message");
	assert_eq!(tail.role, "user");
	assert!(tail.cached, "preserved tail must carry the second marker");
}

#[test]
fn test_insert_compressed_knowledge_skips_markers_for_non_caching_model() {
	let mut session =
		ChatSession::for_tests(vec![message("system", "anchor"), message("user", "q")]);
	session.session.info.model = "ollama:fake-model".to_string();

	session
		.insert_compressed_knowledge(0, "summary".to_string())
		.expect("insert knowledge");

	let compressed = &session.session.messages[1];
	assert!(
		!compressed.cached,
		"no cache marker for a non-caching model"
	);
	assert!(
		!session.session.messages.last().expect("tail").cached,
		"tail must stay uncached for a non-caching model"
	);
}

// ---------------------------------------------------------------------------
// Attachments: real file + URL paths
// ---------------------------------------------------------------------------

/// Single-shot-per-connection HTTP server serving fixed bytes for any path.
async fn spawn_bytes_server(content_type: &'static str, body: Vec<u8>) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind bytes server");
	let addr = listener.local_addr().expect("bytes server addr");
	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			let body = body.clone();
			tokio::spawn(async move {
				use tokio::io::{AsyncReadExt, AsyncWriteExt};
				let mut tmp = [0u8; 8192];
				// drain the request head
				let _ = sock.read(&mut tmp).await;
				let head = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
					body.len()
				);
				let _ = sock.write_all(head.as_bytes()).await;
				let _ = sock.write_all(&body).await;
				let _ = sock.shutdown().await;
			});
		}
	});
	format!("http://{addr}")
}

fn tiny_png_bytes() -> Vec<u8> {
	let img = image::RgbImage::from_pixel(4, 4, image::Rgb([0u8, 128, 255]));
	let mut buf = std::io::Cursor::new(Vec::new());
	img.write_to(&mut buf, image::ImageFormat::Png)
		.expect("encode tiny png");
	buf.into_inner()
}

#[tokio::test]
async fn test_attach_image_from_png_file_sets_pending_image() {
	let dir = tempfile::tempdir().unwrap();
	let png = dir.path().join("tiny.png");
	std::fs::write(&png, tiny_png_bytes()).expect("write png");

	let mut session = ChatSession::for_tests(Vec::new());
	session
		.attach_image_from_path(png.to_str().unwrap())
		.await
		.expect("attach png file");
	assert!(session.has_pending_image());
	let attachment = session.take_pending_image().expect("pending image");
	assert!(
		matches!(&attachment.data, crate::session::image::ImageData::Base64(data) if !data.is_empty()),
		"image data must be attached inlined"
	);
	assert!(!session.has_pending_image(), "take clears the pending slot");
}

#[tokio::test]
async fn test_attach_image_from_unsupported_file_errors() {
	let dir = tempfile::tempdir().unwrap();
	let txt = dir.path().join("not-an-image.txt");
	std::fs::write(&txt, b"just text").expect("write txt");

	let mut session = ChatSession::for_tests(Vec::new());
	let err = session
		.attach_image_from_path(txt.to_str().unwrap())
		.await
		.expect_err("unsupported format must fail");
	assert!(
		err.to_string().to_lowercase().contains("unsupported"),
		"unexpected error: {err}"
	);
	assert!(!session.has_pending_image());
}

#[tokio::test]
async fn test_attach_image_from_url_downloads_and_sets_pending() {
	let url = spawn_bytes_server("image/png", tiny_png_bytes()).await;
	let mut session = ChatSession::for_tests(Vec::new());
	session
		.attach_image_from_path(&format!("{url}/tiny.png"))
		.await
		.expect("attach image from url");
	assert!(session.has_pending_image());
	let attachment = session.take_pending_image().expect("pending image");
	assert!(
		matches!(&attachment.data, crate::session::image::ImageData::Base64(data) if !data.is_empty()),
		"url download must inline the image data"
	);
}

#[tokio::test]
async fn test_attach_video_from_file_and_url() {
	let dir = tempfile::tempdir().unwrap();
	let mp4 = dir.path().join("clip.mp4");
	std::fs::write(&mp4, b"\x00\x00\x00\x18ftypmp42fake-bytes").expect("write mp4");

	let mut session = ChatSession::for_tests(Vec::new());
	session
		.attach_video_from_path(mp4.to_str().unwrap())
		.await
		.expect("attach mp4 file");
	assert!(session.has_pending_video());
	let video = session.take_pending_video().expect("pending video");
	assert!(
		matches!(&video.data, crate::session::video::VideoData::Base64(data) if !data.is_empty()),
		"video data must be attached inlined"
	);

	// Unsupported extension is rejected before any decode attempt
	let txt = dir.path().join("clip.txt");
	std::fs::write(&txt, b"not a video").expect("write txt");
	let err = session
		.attach_video_from_path(txt.to_str().unwrap())
		.await
		.expect_err("unsupported video format must fail");
	assert!(
		err.to_string().to_lowercase().contains("unsupported"),
		"unexpected error: {err}"
	);

	// URL path with a supported extension downloads the same bytes
	let url = spawn_bytes_server("video/mp4", b"\x00\x00\x00\x18ftypmp42fake".to_vec()).await;
	session
		.attach_video_from_path(&format!("{url}/clip.mp4"))
		.await
		.expect("attach video from url");
	assert!(session.has_pending_video());
}

// ---------------------------------------------------------------------------
// get_full_context_tokens: active memory pack accounting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_full_context_tokens_counts_active_memory_pack_once() {
	let config = test_config();
	let mut session =
		ChatSession::for_tests(vec![message("system", "anchor"), message("user", "hello")]);
	let without_pack = session.get_full_context_tokens(&config).await;

	// A pack that is NOT materialized in messages adds its bounded cost
	session.active_memory_pack = Some("remember: build on the box".to_string());
	let with_pack = session.get_full_context_tokens(&config).await;
	assert!(
		with_pack > without_pack,
		"pack must add tokens: {with_pack} vs {without_pack}"
	);

	// Once materialized as a named message it must not be double-counted
	let mut pack_msg = message("user", "remember: build on the box");
	pack_msg.name = Some("__active_memory_pack".to_string());
	session.session.messages.push(pack_msg);
	let materialized = session.get_full_context_tokens(&config).await;
	assert!(
		materialized < with_pack,
		"materialized pack must not be counted twice: {materialized} vs {with_pack}"
	);
}

// ---------------------------------------------------------------------------
// reinitialize_for_role
// ---------------------------------------------------------------------------

#[serial_test::serial]
#[tokio::test]
async fn reinitialize_for_role_replaces_system_prompt_and_saves() {
	let _data = DataDirGuard::new();
	let _lock = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let config = test_config();

	let dir = tempfile::tempdir().unwrap();
	let mut session = ChatSession::for_tests(vec![
		message("system", "old system prompt"),
		message("user", "hi"),
	]);
	session.session.session_file = Some(dir.path().join("session.jsonl.zst"));

	session
		.reinitialize_for_role("core", &config)
		.await
		.expect("role reinitialization");

	let first = &session.session.messages[0];
	assert_eq!(first.role, "system");
	assert_ne!(
		first.content, "old system prompt",
		"system prompt must be rebuilt for the new role"
	);
	assert_eq!(
		session.session.messages[1].content, "hi",
		"history preserved"
	);
	assert!(
		session.session.session_file.as_ref().unwrap().exists(),
		"reinitialization must persist the session"
	);
}

#[serial_test::serial]
#[tokio::test]
async fn reinitialize_for_role_without_system_first_message_errors() {
	let _data = DataDirGuard::new();
	let _lock = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let config = test_config();

	let mut session = ChatSession::for_tests(vec![message("user", "no system here")]);
	session.session.session_file = Some(tempfile::tempdir().unwrap().keep());

	let err = session
		.reinitialize_for_role("core", &config)
		.await
		.expect_err("non-system first message must fail");
	assert!(
		err.to_string()
			.contains("Expected first message to be system"),
		"{err}"
	);
}

#[serial_test::serial]
#[tokio::test]
async fn reinitialize_for_role_on_empty_session_adds_system_message() {
	let _data = DataDirGuard::new();
	let _lock = crate::session::chat::test_support::ENV_LOCK.lock().await;
	let config = test_config();

	let mut session = ChatSession::for_tests(Vec::new());
	session.session.session_file = Some(tempfile::tempdir().unwrap().keep());

	session
		.reinitialize_for_role("core", &config)
		.await
		.expect("empty session reinitializes");

	assert_eq!(session.get_message_count(), 1);
	assert_eq!(session.session.messages[0].role, "system");
	assert!(!session.session.messages[0].content.is_empty());
}

// ---------------------------------------------------------------------------
// Resume: runtime state (role, critical knowledge) from the session log
// ---------------------------------------------------------------------------

/// Write a zstd session log with SUMMARY + COMMAND + KNOWLEDGE_ENTRY entries
/// plus one message line, exactly like a real session file.
fn seed_session_log(name: &str, lines: Vec<serde_json::Value>) {
	let sessions_dir = crate::session::persistence::get_sessions_dir().unwrap();
	std::fs::create_dir_all(&sessions_dir).unwrap();
	let mut info = crate::session::SessionInfo::default();
	info.name = name.to_string();
	info.model = "anthropic/claude-3-5-sonnet".to_string();

	let mut all = vec![serde_json::json!({
		"type": "SUMMARY",
		"timestamp": crate::utils::time::now_secs(),
		"session_info": info,
	})];
	all.extend(lines);
	let payload = all
		.iter()
		.map(|l| serde_json::to_string(l).unwrap())
		.collect::<Vec<_>>()
		.join("\n");
	let compressed = zstd::encode_all(payload.as_bytes(), 3).expect("compress log");
	std::fs::write(sessions_dir.join(format!("{name}.jsonl.zst")), compressed)
		.expect("write session log");
}

#[serial_test::serial]
#[tokio::test]
async fn resume_restores_role_and_critical_knowledge_from_log() {
	let _data = DataDirGuard::new();
	let user_msg = crate::session::Message {
		role: "user".to_string(),
		content: "resumed question".to_string(),
		..Default::default()
	};
	seed_session_log(
		"cov-resume-runtime",
		vec![
			serde_json::json!({"type": "COMMAND", "command": "/role core"}),
			serde_json::json!({"type": "COMMAND", "command": "/cache"}),
			serde_json::json!({"type": "KNOWLEDGE_ENTRY", "content": "keep the widget minimal"}),
			serde_json::to_value(&user_msg).unwrap(),
		],
	);

	let config = test_config();
	let params =
		SessionInitParams::new(&config, "assistant").with_resume("cov-resume-runtime".to_string());
	let session = ChatSession::initialize(params)
		.await
		.expect("resume seeded session");

	assert!(session.was_resumed);
	// /role core was logged and the caller did not name a role explicitly
	assert_eq!(
		session.role, "core",
		"logged /role must win over the default"
	);
	assert_eq!(
		session.critical_knowledge,
		vec!["keep the widget minimal".to_string()],
		"knowledge entries must be restored"
	);
	assert!(
		session
			.session
			.messages
			.iter()
			.any(|m| m.content == "resumed question"),
		"transcript messages must be restored"
	);
}
