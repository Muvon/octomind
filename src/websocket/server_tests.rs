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

fn handshake(origin: Option<&str>, allow_origins: &[&str]) -> Result<Response, Box<ErrorResponse>> {
	let mut req = Request::builder().uri("/");
	if let Some(origin) = origin {
		req = req.header(ORIGIN, origin);
	}
	let allowlist = allow_origins.iter().map(|o| (*o).to_string()).collect();
	OriginAllowlist(Arc::new(allowlist))
		.on_request(&req.body(()).unwrap(), Response::new(()))
		.map_err(Box::new)
}

#[test]
fn native_clients_send_no_origin_and_are_allowed() {
	assert!(handshake(None, &[]).is_ok());
}

#[test]
fn listed_origin_is_allowed() {
	assert!(handshake(Some("http://localhost:3000"), &["http://localhost:3000"]).is_ok());
}

#[test]
fn unlisted_origin_is_refused() {
	// A different port is a different origin — this is the drive-by browser case.
	let err = handshake(Some("http://localhost:3001"), &["http://localhost:3000"]).unwrap_err();
	assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[test]
fn empty_allowlist_refuses_every_browser() {
	let err = handshake(Some("https://evil.example.com"), &[]).unwrap_err();
	assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

fn image_attachment() -> Attachment {
	Attachment {
		id: "AbCdEf0123456789GhIjKlMn".to_string(),
		kind: AttachmentKind::Image,
		media_type: "image/png".to_string(),
		name: "screenshot.png".to_string(),
		size: 1234,
	}
}

#[test]
fn known_non_vision_model_refuses_websocket_image_before_file_access() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();
	let missing_root = Path::new("/definitely/missing/media/root");

	let error = load_message_attachments(&session, &[image_attachment()], missing_root)
		.expect_err("known text-only model must refuse image before resolving the file");
	assert!(error.to_string().contains("openai:gpt-3.5-turbo"));
	assert!(error.to_string().contains("does not support vision"));
	assert!(!error.to_string().contains("missing or unreadable"));
}

#[test]
fn prefixed_websocket_image_is_attached_to_empty_user_turn() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let attachment = image_attachment();
	// The writer stores media as `<id>.<ext>`; resolve_path locates it by
	// prefix, so the fixture must be laid out the same way.
	let media_path = tmp.path().join(format!("{}.png", attachment.id));
	image::RgbImage::new(4, 4)
		.save(&media_path)
		.expect("save test image");

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let loaded = load_message_attachments(&session, &[attachment], tmp.path())
		.expect("load websocket attachment");
	session
		.add_user_message_with_attachments("", loaded.images, loaded.videos)
		.expect("add attachment-only user turn");

	let message = session.session.messages.last().expect("user message");
	assert_eq!(message.content, "");
	assert_eq!(message.images.as_ref().map(Vec::len), Some(1));
	assert!(message.videos.is_none());
}

#[test]
fn attachment_with_no_matching_file_is_reported_as_not_found() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let attachment = image_attachment();

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let error = load_message_attachments(&session, std::slice::from_ref(&attachment), tmp.path())
		.expect_err("no file on disk must be reported as not found");
	assert!(error.to_string().contains("not found"));
	assert!(error.to_string().contains(&attachment.id));
}

// ---- per-session locks ----

#[tokio::test]
async fn session_lock_is_reused_per_session_id() {
	let locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
	let a = get_or_create_session_lock("s", &locks).await;
	let b = get_or_create_session_lock("s", &locks).await;
	assert!(Arc::ptr_eq(&a, &b), "same session must share one lock");

	let other = get_or_create_session_lock("other", &locks).await;
	assert!(
		!Arc::ptr_eq(&a, &other),
		"different sessions must not share"
	);
}

// ---- lookup_session ----

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
async fn lookup_session_returns_the_memory_copy_and_removes_it() {
	let sessions: Arc<Mutex<HashMap<String, ChatSession>>> = Arc::new(Mutex::new(HashMap::new()));
	sessions
		.lock()
		.await
		.insert("in-mem".to_string(), ChatSession::for_tests(Vec::new()));

	let config = template_config();
	let session = lookup_session("in-mem", &sessions, &config, "assistant")
		.await
		.expect("memory hit resolves without touching disk");
	assert_eq!(
		session.session.info.name, "test",
		"the exact in-memory instance is returned"
	);
	assert!(
		sessions.lock().await.is_empty(),
		"lookup takes the session out for exclusive processing"
	);
}

#[tokio::test]
async fn lookup_session_never_auto_creates_a_missing_session() {
	let sessions: Arc<Mutex<HashMap<String, ChatSession>>> = Arc::new(Mutex::new(HashMap::new()));
	let config = template_config();
	let error = lookup_session("no-such-session-zzz", &sessions, &config, "assistant")
		.await
		.err()
		.expect("a session that exists nowhere must be an error");
	assert!(
		error.contains("Session not found: no-such-session-zzz"),
		"{error}"
	);
	assert!(
		sessions.lock().await.is_empty(),
		"nothing may be auto-created"
	);
}

// ---- attachment hardening ----

fn attachment(id: &str, kind: AttachmentKind, media_type: &str) -> Attachment {
	Attachment {
		id: id.to_string(),
		kind,
		media_type: media_type.to_string(),
		name: format!("upload.{media_type}"),
		size: 1,
	}
}

#[test]
fn video_attachment_on_non_video_model_is_refused_before_file_access() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openai:gpt-3.5-turbo".to_string();
	let missing_root = Path::new("/definitely/missing/media/root");

	let error = load_message_attachments(
		&session,
		&[attachment(
			"AbCdEf0123456789GhIjKlMn",
			AttachmentKind::Video,
			"video/mp4",
		)],
		missing_root,
	)
	.expect_err("known non-video model must refuse before resolving the file");
	assert!(
		error.to_string().contains("does not support video"),
		"got: {error}"
	);
}

#[test]
fn attachment_pointing_at_a_directory_is_rejected() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let id = "DirEntry0123456789AbCdEf";
	std::fs::create_dir(tmp.path().join(format!("{id}.mp4"))).expect("create dir fixture");

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let error = load_message_attachments(
		&session,
		&[attachment(id, AttachmentKind::Video, "video/mp4")],
		tmp.path(),
	)
	.expect_err("a directory must never be treated as media");
	assert!(
		error.to_string().contains("must be a regular file"),
		"got: {error}"
	);
}

#[test]
fn ambiguous_attachment_prefix_is_rejected() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let id = "Ambiguous0123456789AbCdE";
	for ext in ["png", "jpg"] {
		std::fs::write(tmp.path().join(format!("{id}.{ext}")), b"x").expect("fixture");
	}

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let error = load_message_attachments(
		&session,
		&[attachment(id, AttachmentKind::Image, "image/png")],
		tmp.path(),
	)
	.expect_err("two matching files are ambiguous");
	assert!(
		error.to_string().contains("multiple matching files"),
		"got: {error}"
	);
}

#[cfg(unix)]
#[test]
fn symlinked_attachment_is_rejected() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let id = "SymLink00123456789AbCdEf";
	let target = tmp.path().join("real.png");
	std::fs::write(&target, b"x").expect("fixture");
	std::os::unix::fs::symlink(&target, tmp.path().join(format!("{id}.png"))).expect("symlink");

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let error = load_message_attachments(
		&session,
		&[attachment(id, AttachmentKind::Image, "image/png")],
		tmp.path(),
	)
	.expect_err("symlinks must not be followed");
	assert!(
		error.to_string().contains("must not be a symbolic link"),
		"got: {error}"
	);
}

#[test]
fn audio_attachment_opens_the_file_and_adds_no_media() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let id = "AudioOnly0123456789AbCdE";
	std::fs::write(tmp.path().join(format!("{id}.mp3")), b"x").expect("fixture");

	let session = ChatSession::for_tests(Vec::new());
	let loaded = load_message_attachments(
		&session,
		&[attachment(id, AttachmentKind::Audio, "audio/mp3")],
		tmp.path(),
	)
	.expect("audio needs no model capability, only a readable file");
	assert!(loaded.images.is_empty());
	assert!(loaded.videos.is_empty());
}

#[test]
fn corrupt_image_file_reports_a_load_failure() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let id = "CorruptIm0123456789AbCdE";
	std::fs::write(tmp.path().join(format!("{id}.png")), b"not a real png").expect("fixture");

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let error = load_message_attachments(
		&session,
		&[attachment(id, AttachmentKind::Image, "image/png")],
		tmp.path(),
	)
	.expect_err("garbage bytes must fail the image decoder");
	assert!(
		error
			.to_string()
			.contains("Failed to load image attachment"),
		"got: {error}"
	);
}

// ---- full connection lifecycle over a real loopback socket ----

async fn read_json<S>(ws: &mut S) -> serde_json::Value
where
	S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
	let frame = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
		.await
		.expect("frame must arrive within the timeout")
		.expect("stream must stay open")
		.expect("frame must decode");
	match frame {
		Message::Text(text) => serde_json::from_str(&text).expect("server frames are JSON"),
		other => panic!("expected a text frame, got {other:?}"),
	}
}

#[tokio::test]
async fn connection_lifecycle_over_loopback() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind loopback listener");
	let addr = listener.local_addr().expect("local addr");

	let config = Arc::new(template_config());
	let role = "assistant".to_string();
	let sessions: Arc<Mutex<HashMap<String, ChatSession>>> = Arc::new(Mutex::new(HashMap::new()));
	let session_locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
	let allow_origins: Arc<Vec<String>> = Arc::new(Vec::new());

	tokio::spawn(async move {
		if let Ok((stream, peer)) = listener.accept().await {
			let _ = handle_connection(
				stream,
				peer,
				config,
				role,
				sessions,
				session_locks,
				allow_origins,
			)
			.await;
		}
	});

	let (mut ws, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
		.await
		.expect("client connects over loopback");

	// 1. The welcome status frame arrives before anything else.
	let welcome = read_json(&mut ws).await;
	assert_eq!(welcome["type"], "status");
	assert!(
		welcome["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Connected to Octomind"),
		"got: {welcome}"
	);

	// 2. Invalid JSON is reported but does not kill the connection.
	ws.send(Message::text("{not json"))
		.await
		.expect("send invalid json");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("Invalid JSON"));

	// 3. Validation failures echo the client's request_id.
	ws.send(Message::text(
		r#"{"type":"command","session_id":"s","command":"","request_id":"req-1"}"#,
	))
	.await
	.expect("send invalid command");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert_eq!(error["request_id"], "req-1");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("command cannot be empty"));

	// 4. A command for an unknown session: ack, then a lookup error.
	ws.send(Message::text(
		r#"{"type":"command","session_id":"no-such-session-zzz","command":"info","request_id":"req-2"}"#,
	))
	.await
	.expect("send command");
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	assert_eq!(ack["request_id"], "req-2");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("Session not found: no-such-session-zzz"));

	// 5. A user message for an unknown session takes the same lookup path.
	ws.send(Message::text(
		r#"{"type":"message","session_id":"no-such-session-zzz","content":"hi","request_id":"req-3"}"#,
	))
	.await
	.expect("send message");
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("Session not found"));

	// 6. Binary frames are refused with a protocol hint.
	ws.send(Message::binary(vec![0u8]))
		.await
		.expect("send binary");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("Unsupported WebSocket message type"));

	// 7. Close terminates the connection cleanly.
	ws.send(Message::Close(None)).await.expect("send close");
	let ended = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
		.await
		.expect("connection must end within the timeout");
	assert!(
		ended.is_none() || matches!(ended, Some(Err(_))),
		"client stream must end after close"
	);
}

// ---- full server behavior over a real loopback socket ----

use std::time::Duration;

use crate::session::chat::test_support::{
	fake_provider_config, final_response, spawn_stub_with_status, ENV_LOCK,
};
use crate::session::inbox::{push_inbox_message_for_session, InboxMessage, InboxSource};

/// Points OCTOMIND_DATA_DIR at a unique temp dir and restores the previous
/// value on drop. Session storage, logs, and the evolution registry all live
/// under it, so every test that creates real sessions must isolate it.
struct TestDataDirGuard {
	previous: Option<String>,
	_dir: Option<tempfile::TempDir>,
}

impl TestDataDirGuard {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("tempdir");
		let previous = std::env::var("OCTOMIND_DATA_DIR").ok();
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: Some(dir),
		}
	}

	/// Like [`Self::new`] but for a caller-managed directory whose layout the
	/// test sabotages on purpose. The caller keeps the TempDir alive.
	fn at(path: &std::path::Path) -> Self {
		let previous = std::env::var("OCTOMIND_DATA_DIR").ok();
		std::env::set_var("OCTOMIND_DATA_DIR", path);
		Self {
			previous,
			_dir: None,
		}
	}
}

impl Drop for TestDataDirGuard {
	fn drop(&mut self) {
		match &self.previous {
			Some(value) => std::env::set_var("OCTOMIND_DATA_DIR", value),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Holds ENV_LOCK and points OLLAMA_API_URL at a scripted stub for the
/// duration of the test. Serial tests only — the env var is process-global.
struct StubEnv {
	_guard: tokio::sync::MutexGuard<'static, ()>,
}

impl StubEnv {
	async fn new(responses: Vec<serde_json::Value>) -> Self {
		Self::with_status(responses.into_iter().map(|r| (200u16, r)).collect()).await
	}

	async fn with_status(responses: Vec<(u16, serde_json::Value)>) -> Self {
		let guard = ENV_LOCK.lock().await;
		let url = spawn_stub_with_status(responses).await;
		std::env::set_var("OLLAMA_API_URL", &url);
		Self { _guard: guard }
	}
}

impl Drop for StubEnv {
	fn drop(&mut self) {
		std::env::remove_var("OLLAMA_API_URL");
	}
}

/// Fake-provider config with the compression decision model also routed to
/// the scripted stub, so `/done` never reaches a real provider.
fn ws_fake_config() -> Config {
	let mut config = fake_provider_config();
	config.compression.model.model = Some("ollama:fake-model".to_string());
	config.compression.model.max_retries = Some(0);
	config.supervisor.learning.enabled = false;
	config
}

fn msg(role: &str, content: &str) -> crate::session::Message {
	crate::session::Session::build_message(role, content)
}

/// A session with enough turns for `/done` compression to find a range.
fn compressible_session() -> ChatSession {
	let mut session = ChatSession::for_tests(vec![
		msg("system", "You are a helpful assistant."),
		msg("user", "build the frobnicator widget"),
		msg("assistant", "starting on the widget now"),
		msg("user", "make sure it compiles"),
		msg("assistant", "phase one is done and compiling"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.session.info.model = "ollama:fake-model".to_string();
	session
}

fn xml_summary_body() -> String {
	concat!(
		"<should_compress>true</should_compress>\n",
		"<original_request>build the frobnicator widget</original_request>\n",
		"<session_context>COMPRESS-E2E-CONTEXT: rust repo, widget work</session_context>\n",
		"<current_task>finish the frobnicator widget</current_task>\n",
		"<progress>phase one complete</progress>\n",
		"<analysis_findings><finding>widget lives in src/widget.rs</finding></analysis_findings>\n",
		"<errors_and_corrections><entry>fixed a compile error</entry></errors_and_corrections>\n",
		"<recent_exchanges><exchange>user asked for compilation, assistant confirmed</exchange></recent_exchanges>\n",
		"<key_entities><files><file>src/widget.rs</file></files>",
		"<names><name>Frobnicator</name></names>",
		"<decisions><decision>keep the widget minimal</decision></decisions></key_entities>\n",
		"<next_steps>wire the widget tests</next_steps>\n",
		"<critical_knowledge><knowledge>widget must stay allocation-free</knowledge></critical_knowledge>\n",
		"<open_loops><open_loop>widget rendering</open_loop></open_loops>\n",
		"<file_states><state>src/widget.rs modified</state></file_states>"
	)
	.to_string()
}

/// A loopback WebSocket server accepting any number of connections, sharing
/// the sessions map and session locks with the test so it can stage state.
struct LoopbackServer {
	addr: std::net::SocketAddr,
	sessions: Arc<Mutex<HashMap<String, ChatSession>>>,
	session_locks: SessionLocks,
}

impl LoopbackServer {
	async fn start(config: Arc<Config>) -> Self {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind loopback listener");
		let addr = listener.local_addr().expect("local addr");
		let sessions: Arc<Mutex<HashMap<String, ChatSession>>> =
			Arc::new(Mutex::new(HashMap::new()));
		let session_locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
		let allow_origins: Arc<Vec<String>> = Arc::new(Vec::new());
		let role = "assistant".to_string();
		let sessions_handle = sessions.clone();
		let locks_handle = session_locks.clone();
		tokio::spawn(async move {
			while let Ok((stream, peer)) = listener.accept().await {
				let config = config.clone();
				let sessions = sessions_handle.clone();
				let session_locks = locks_handle.clone();
				let allow_origins = allow_origins.clone();
				let role = role.clone();
				tokio::spawn(async move {
					let _ = handle_connection(
						stream,
						peer,
						config,
						role,
						sessions,
						session_locks,
						allow_origins,
					)
					.await;
				});
			}
		});
		Self {
			addr,
			sessions,
			session_locks,
		}
	}
}

type ClientWs =
	tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_ws(addr: std::net::SocketAddr) -> ClientWs {
	let (ws, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
		.await
		.expect("client connects over loopback");
	ws
}

async fn send_json(ws: &mut ClientWs, value: serde_json::Value) {
	ws.send(Message::text(value.to_string()))
		.await
		.expect("send frame");
}

async fn read_frame(ws: &mut ClientWs) -> Message {
	tokio::time::timeout(Duration::from_secs(10), ws.next())
		.await
		.expect("frame must arrive within the timeout")
		.expect("stream must stay open")
		.expect("frame must decode")
}

/// Reads frames until one satisfies the predicate, skipping unrelated
/// streaming frames (thinking, tool use, …). Bounded so a missing frame
/// fails instead of hanging.
async fn read_until(
	ws: &mut ClientWs,
	predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
	for _ in 0..100 {
		let value = read_json(ws).await;
		if predicate(&value) {
			return value;
		}
	}
	panic!("no frame satisfied the predicate within 100 frames");
}

/// Creates a session over the connection and returns its server-assigned id.
async fn create_session(ws: &mut ClientWs, session_id: Option<&str>) -> String {
	let mut request = serde_json::json!({"type": "session"});
	if let Some(id) = session_id {
		request["session_id"] = serde_json::json!(id);
	}
	send_json(ws, request).await;
	let ack = read_json(ws).await;
	assert_eq!(ack["type"], "ack", "got: {ack}");
	let status = read_json(ws).await;
	assert_eq!(status["type"], "status", "got: {status}");
	status["session_id"]
		.as_str()
		.expect("status carries the session id")
		.to_string()
}

async fn inbox_has_messages(session_id: &str) -> bool {
	crate::session::context::with_session_id(session_id.to_string(), async {
		crate::session::inbox::has_inbox_messages()
	})
	.await
}

// ---- control frames ----

#[tokio::test]
async fn ping_frames_are_answered_with_a_matching_pong() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	ws.send(Message::Ping(vec![1u8, 2, 3].into()))
		.await
		.expect("send ping");
	let frame = read_frame(&mut ws).await;
	match frame {
		Message::Pong(data) => {
			assert_eq!(&data[..], &[1u8, 2, 3][..], "pong must echo the payload")
		}
		other => panic!("expected a pong, got {other:?}"),
	}
}

#[tokio::test]
async fn pong_frames_are_ignored_and_the_connection_stays_usable() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	ws.send(Message::Pong(vec![9u8].into()))
		.await
		.expect("send pong");
	// The read loop must have continued past the Pong arm: a subsequent
	// invalid frame still gets its protocol error.
	ws.send(Message::text("{not json"))
		.await
		.expect("send junk");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("Invalid JSON"));
}

#[tokio::test]
async fn oversized_frame_terminates_the_connection() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let huge = "x".repeat(10 * 1024 * 1024 + 1);
	// A fast peer may reset while the oversized write is still in progress;
	// that is already the expected terminal outcome.
	if ws.send(Message::text(huge)).await.is_err() {
		return;
	}

	let ended = tokio::time::timeout(Duration::from_secs(15), async {
		loop {
			match ws.next().await {
				None => break,
				Some(Err(_)) => break,
				Some(Ok(Message::Close(_))) => break,
				Some(Ok(_)) => continue,
			}
		}
	})
	.await;
	assert!(
		ended.is_ok(),
		"the server must end the connection after an over-limit frame"
	);
}

#[tokio::test]
async fn abrupt_disconnect_without_close_ends_the_handler_cleanly() {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let config = Arc::new(template_config());
	let sessions: Arc<Mutex<HashMap<String, ChatSession>>> = Arc::new(Mutex::new(HashMap::new()));
	let session_locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
	let allow_origins: Arc<Vec<String>> = Arc::new(Vec::new());
	let handle = tokio::spawn(async move {
		// Accept inside the task: the client only connects after this spawn,
		// so awaiting accept on the main task would deadlock.
		let (stream, peer) = listener.accept().await.expect("accept");
		handle_connection(
			stream,
			peer,
			config,
			"assistant".to_string(),
			sessions,
			session_locks,
			allow_origins,
		)
		.await
	});

	let mut ws = connect_ws(addr).await;
	let _welcome = read_json(&mut ws).await;
	drop(ws); // no Close frame — the TCP connection just vanishes

	let result = tokio::time::timeout(Duration::from_secs(10), handle)
		.await
		.expect("handler must finish after the client drops")
		.expect("handler task must not panic");
	assert!(result.is_ok(), "a clean drop is not an error: {result:?}");
}

// ---- command exclusion lock ----

#[tokio::test]
async fn command_on_a_locked_session_reports_busy() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	// Hold the per-session lock the command path must try_lock.
	let lock = get_or_create_session_lock("busy-1", &server.session_locks).await;
	let _guard = lock.lock().await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": "busy-1", "command": "info", "request_id": "r1"
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("busy processing another request"),
		"got: {error}"
	);
}

// ---- session create / resume ----

#[tokio::test]
#[serial_test::serial]
async fn session_message_creates_an_auto_named_session_and_persists_it() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the created session is stored in memory"
	);
	let file = crate::session::get_sessions_dir()
		.expect("sessions dir")
		.join(format!("{session_id}.jsonl.zst"));
	assert!(file.exists(), "session persisted: {}", file.display());

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn named_session_resumes_from_disk_on_a_fresh_connection() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;

	// Connection 1: create the named session (persisted to disk).
	{
		let mut ws = connect_ws(server.addr).await;
		let _welcome = read_json(&mut ws).await;
		let session_id = create_session(&mut ws, Some("disk-resume-1")).await;
		assert_eq!(session_id, "disk-resume-1");
	}

	// Connection 2 with a FRESH in-memory map: the same name must resume
	// from disk, not create a second session.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let config = Arc::new(ws_fake_config());
	let sessions: Arc<Mutex<HashMap<String, ChatSession>>> = Arc::new(Mutex::new(HashMap::new()));
	let session_locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
	let sessions_for_task = sessions.clone();
	tokio::spawn(async move {
		if let Ok((stream, peer)) = listener.accept().await {
			let _ = handle_connection(
				stream,
				peer,
				config,
				"assistant".to_string(),
				sessions_for_task,
				session_locks,
				Arc::new(Vec::new()),
			)
			.await;
		}
	});
	let mut ws = connect_ws(addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": "disk-resume-1"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let status = read_json(&mut ws).await;
	assert!(
		status["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Session resumed"),
		"got: {status}"
	);

	crate::session::context::cleanup_session(&"disk-resume-1".to_string());
}

#[tokio::test]
#[serial_test::serial]
async fn user_message_resumes_a_disk_session_without_a_session_handshake() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("DISK-TURN-OK")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;

	// Create the session on disk via one connection.
	{
		let mut ws = connect_ws(server.addr).await;
		let _welcome = read_json(&mut ws).await;
		let _ = create_session(&mut ws, Some("disk-lookup-1")).await;
	}

	// A fresh map + connection: a user message alone must resume from disk
	// and run the turn.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let config = Arc::new(ws_fake_config());
	let sessions: Arc<Mutex<HashMap<String, ChatSession>>> = Arc::new(Mutex::new(HashMap::new()));
	let session_locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
	let sessions_for_task = sessions.clone();
	tokio::spawn(async move {
		if let Ok((stream, peer)) = listener.accept().await {
			let _ = handle_connection(
				stream,
				peer,
				config,
				"assistant".to_string(),
				sessions_for_task,
				session_locks,
				Arc::new(Vec::new()),
			)
			.await;
		}
	});
	let mut ws = connect_ws(addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": "disk-lookup-1", "content": "hello", "request_id": "m1"
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	let assistant = read_until(&mut ws, |v| v["type"] == "assistant").await;
	assert!(
		assistant["content"]
			.as_str()
			.unwrap_or_default()
			.contains("DISK-TURN-OK"),
		"got: {assistant}"
	);
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(cost["session_id"], "disk-lookup-1");

	crate::session::context::cleanup_session(&"disk-lookup-1".to_string());
}

#[tokio::test]
#[serial_test::serial]
async fn session_initialization_failure_is_reported_to_the_client() {
	let data = tempfile::tempdir().expect("tempdir");
	// The sessions path exists as a FILE, so session initialization cannot
	// create its storage.
	std::fs::write(data.path().join("sessions"), b"not a directory").expect("sabotage");
	let _guard = TestDataDirGuard::at(data.path());

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(&mut ws, serde_json::json!({"type": "session"})).await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Internal error"),
		"got: {error}"
	);
}

// ---- user message turns ----

#[tokio::test]
#[serial_test::serial]
async fn user_message_runs_a_full_turn_and_streams_assistant_and_cost() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("WS-TURN-OK")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message",
			"session_id": session_id,
			"content": "hello",
			"request_id": "m1"
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	assert_eq!(ack["request_id"], "m1");

	let assistant = read_until(&mut ws, |v| v["type"] == "assistant").await;
	assert!(
		assistant["content"]
			.as_str()
			.unwrap_or_default()
			.contains("WS-TURN-OK"),
		"got: {assistant}"
	);
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(cost["session_id"], session_id);
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the session is stored back after the turn"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
async fn attachment_failure_returns_a_request_scoped_error() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	server
		.sessions
		.lock()
		.await
		.insert("vision-1".to_string(), {
			let mut session = ChatSession::for_tests(Vec::new());
			session.model = "openai:gpt-3.5-turbo".to_string();
			session
		});
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message",
			"session_id": "vision-1",
			"content": "look at this",
			"request_id": "att-1",
			"attachments": [{
				"id": "AbCdEf0123456789GhIjKlMn",
				"kind": "image",
				"media_type": "image/png",
				"name": "screenshot.png",
				"size": 10
			}]
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert_eq!(error["request_id"], "att-1");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("does not support vision"),
		"got: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn api_failure_surfaces_as_an_error_frame() {
	let _data = TestDataDirGuard::new();
	let failures: Vec<(u16, serde_json::Value)> = (0..6)
		.map(|_| (500u16, serde_json::json!({"error": {"message": "boom"}})))
		.collect();
	let _env = StubEnv::with_status(failures).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": session_id, "content": "hello"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_until(&mut ws, |v| v["type"] == "error").await;
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.starts_with("Error:"),
		"got: {error}"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn failing_pipe_blocks_the_user_message_with_an_error() {
	let _data = TestDataDirGuard::new();
	let workdir = tempfile::tempdir().expect("workdir");
	std::fs::create_dir_all(workdir.path().join(".agents")).expect("agents dir");
	std::fs::write(
		workdir.path().join(".agents/guardrails.toml"),
		r#"
[[pipe]]
name = "boom"
command = "./definitely-missing-pipe-script.sh"
"#,
	)
	.expect("guardrails fixture");

	let previous_cwd = std::env::current_dir().expect("cwd");
	std::env::set_current_dir(workdir.path()).expect("chdir");
	struct CwdGuard(std::path::PathBuf);
	impl Drop for CwdGuard {
		fn drop(&mut self) {
			std::env::set_current_dir(&self.0).expect("restore cwd");
		}
	}
	let _cwd = CwdGuard(previous_cwd);

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": session_id, "content": "hello"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("failed to spawn"),
		"got: {error}"
	);

	crate::session::context::cleanup_session(&session_id);
}

// ---- attachments on the message path ----

/// Points OCTOMIND_MEDIA_ROOT at a temp dir and restores the previous value.
struct MediaRootGuard {
	previous: Option<String>,
	_dir: tempfile::TempDir,
}

impl MediaRootGuard {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("tempdir");
		let previous = std::env::var("OCTOMIND_MEDIA_ROOT").ok();
		std::env::set_var("OCTOMIND_MEDIA_ROOT", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for MediaRootGuard {
	fn drop(&mut self) {
		match &self.previous {
			Some(value) => std::env::set_var("OCTOMIND_MEDIA_ROOT", value),
			None => std::env::remove_var("OCTOMIND_MEDIA_ROOT"),
		}
	}
}

#[tokio::test]
#[serial_test::serial]
async fn video_attachment_loads_and_the_turn_reaches_prepare() {
	let _data = TestDataDirGuard::new();
	let _media = MediaRootGuard::new();
	let id = "VideoLoad0123456789AbCdE";
	std::fs::write(
		std::path::Path::new(&std::env::var("OCTOMIND_MEDIA_ROOT").expect("media root"))
			.join(format!("{id}.mp4")),
		b"fake-mp4-bytes",
	)
	.expect("video fixture");

	// A tiny context ceiling makes prepare_for_api_call fail AFTER the
	// attachments loaded — proving the load path ran without a network call.
	let mut config = ws_fake_config();
	config.max_session_tokens_threshold = 1;
	let server = LoopbackServer::start(Arc::new(config)).await;
	server.sessions.lock().await.insert("video-1".to_string(), {
		let mut session = ChatSession::for_tests(Vec::new());
		session.model = "openrouter:vendor/unknown-vision-model".to_string();
		session
	});
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message",
			"session_id": "video-1",
			"content": "please summarize this video attachment in considerable detail for me now",
			"attachments": [{
				"id": id,
				"kind": "video",
				"media_type": "video/mp4",
				"name": "clip.mp4",
				"size": 15
			}]
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("ceiling"),
		"the turn must fail at the context ceiling, after attachments loaded: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn unsupported_video_media_type_is_rejected() {
	let _data = TestDataDirGuard::new();
	let _media = MediaRootGuard::new();
	let id = "VideoBadM0123456789AbCdE";
	std::fs::write(
		std::path::Path::new(&std::env::var("OCTOMIND_MEDIA_ROOT").expect("media root"))
			.join(format!("{id}.flv")),
		b"fake-flv-bytes",
	)
	.expect("video fixture");

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	server.sessions.lock().await.insert("video-2".to_string(), {
		let mut session = ChatSession::for_tests(Vec::new());
		session.model = "openrouter:vendor/unknown-vision-model".to_string();
		session
	});
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message",
			"session_id": "video-2",
			"content": "look",
			"attachments": [{
				"id": id,
				"kind": "video",
				"media_type": "video/x-flv",
				"name": "clip.flv",
				"size": 14
			}]
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Unsupported video media type"),
		"got: {error}"
	);
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial]
async fn unreadable_audio_attachment_is_reported() {
	let _data = TestDataDirGuard::new();
	let _media = MediaRootGuard::new();
	let id = "AudioBad0123456789AbCdEf";
	let path = std::path::Path::new(&std::env::var("OCTOMIND_MEDIA_ROOT").expect("media root"))
		.join(format!("{id}.mp3"));
	std::fs::write(&path, b"fake-audio-bytes").expect("audio fixture");
	std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o000))
		.expect("chmod 000");

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	server
		.sessions
		.lock()
		.await
		.insert("audio-1".to_string(), ChatSession::for_tests(Vec::new()));
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message",
			"session_id": "audio-1",
			"content": "listen",
			"attachments": [{
				"id": id,
				"kind": "audio",
				"media_type": "audio/mp3",
				"name": "note.mp3",
				"size": 16
			}]
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.to_lowercase()
			.contains("unreadable"),
		"got: {error}"
	);
}

// ---- /done command ----

#[tokio::test]
#[serial_test::serial]
async fn done_command_reports_nothing_to_compress_and_saves() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": session_id, "command": "done", "request_id": "d1"
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	let done = read_json(&mut ws).await;
	assert_eq!(done["type"], "status");
	assert_eq!(done["data"]["command_type"], "done");
	assert_eq!(done["message"], "Nothing to compress");
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the session is stored back after /done"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn done_command_compression_failure_is_reported() {
	let _data = TestDataDirGuard::new();
	let failures: Vec<(u16, serde_json::Value)> = (0..4)
		.map(|_| (500u16, serde_json::json!({"error": {"message": "boom"}})))
		.collect();
	let _env = StubEnv::with_status(failures).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	// A memory-only session (no session file): the save after the failed
	// compression is itself reported.
	server
		.sessions
		.lock()
		.await
		.insert("done-fail-1".to_string(), compressible_session());
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": "done-fail-1", "command": "done"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to save session"),
		"a memory-only session cannot be saved after the failure: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn done_command_compresses_and_reports_the_save_failure_for_memory_sessions() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	server
		.sessions
		.lock()
		.await
		.insert("done-ok-1".to_string(), compressible_session());
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": "done-ok-1", "command": "done"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to save session"),
		"compression succeeded but the memory-only session cannot be saved: {error}"
	);
	// The compression itself ran: the in-memory session now carries the summary.
	let stored = server
		.sessions
		.lock()
		.await
		.get("done-ok-1")
		.expect("session stored back")
		.session
		.messages
		.iter()
		.map(|m| m.content.clone())
		.collect::<String>();
	assert!(
		stored.contains("COMPRESS-E2E-CONTEXT"),
		"the compressed summary must be in the session, got: {stored}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn done_command_with_args_runs_the_instructions_as_a_user_turn() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("AFTER-DONE")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command",
			"session_id": session_id,
			"command": "done",
			"args": ["continue", "with", "this"],
			"request_id": "d2"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let done = read_json(&mut ws).await;
	assert_eq!(done["type"], "status");
	assert_eq!(done["data"]["command_type"], "done");

	// The trailing instructions run as a full user turn.
	let assistant = read_until(&mut ws, |v| v["type"] == "assistant").await;
	assert!(
		assistant["content"]
			.as_str()
			.unwrap_or_default()
			.contains("AFTER-DONE"),
		"got: {assistant}"
	);
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(cost["session_id"], session_id);

	crate::session::context::cleanup_session(&session_id);
}

// ---- other commands ----

#[tokio::test]
#[serial_test::serial]
async fn exit_command_ends_the_session() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": session_id, "command": "exit"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let status = read_json(&mut ws).await;
	assert_eq!(status["type"], "status");
	assert_eq!(status["message"], "Session ended");
	assert_eq!(status["data"]["action"], "exit");
	assert!(
		!server.sessions.lock().await.contains_key(&session_id),
		"an ended session must not be stored back"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn failing_command_reports_the_error_and_keeps_the_session() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command",
			"session_id": session_id,
			"command": "learning",
			"args": ["evolution", "show", "no-such-record"]
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Command failed"),
		"got: {error}"
	);
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the session is stored back even when the command fails"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
async fn command_save_failure_is_reported_for_memory_only_sessions() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	server.sessions.lock().await.insert(
		"save-fail-1".to_string(),
		ChatSession::for_tests(Vec::new()),
	);
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": "save-fail-1", "command": "info"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to save session"),
		"got: {error}"
	);
}

// ---- pre-user inbox drain ----

#[tokio::test]
#[serial_test::serial]
async fn pre_user_inbox_messages_get_their_own_turn_before_the_user_message() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![
		final_response("INBOX-TURN"),
		final_response("INBOX-TURN"),
		final_response("MAIN-TURN"),
	])
	.await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	// Keep the background inbox monitor from racing the foreground drain. It
	// wakes on the pushes, observes the held lock, and leaves both messages for
	// the user-message handler.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	// One system-managed (schedule) and one plain (inject) message: both
	// branches of the drain, each with its own AI turn.
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Schedule {
				id: "sched-1".to_string(),
			},
			content: "scheduled reminder".to_string(),
		},
	);
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "injected before the user turn".to_string(),
		},
	);

	// Let the monitor consume the notification while it cannot take the lock;
	// it then parks with the messages still queued for the foreground drain.
	tokio::time::sleep(Duration::from_millis(100)).await;
	drop(guard);
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": session_id, "content": "hello"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;

	let mut injected = 0;
	let mut inbox_assistants = 0;
	let mut main_assistant = false;
	for _ in 0..100 {
		let frame = read_json(&mut ws).await;
		match frame["type"].as_str().unwrap_or_default() {
			"injected" => {
				injected += 1;
				assert!(
					frame["session_id"] == session_id
						&& (frame["content"].as_str().unwrap_or_default() == "scheduled reminder"
							|| frame["content"].as_str().unwrap_or_default()
								== "injected before the user turn"),
					"got: {frame}"
				);
			}
			"assistant" => {
				let content = frame["content"].as_str().unwrap_or_default();
				if content.contains("INBOX-TURN") {
					inbox_assistants += 1;
				} else if content.contains("MAIN-TURN") {
					main_assistant = true;
				}
			}
			"cost" if main_assistant => break,
			_ => {}
		}
	}
	assert_eq!(injected, 2, "both inbox messages must be announced");
	assert_eq!(
		inbox_assistants, 2,
		"each inbox message gets its own AI turn"
	);
	assert!(main_assistant, "the user message turn must also run");

	crate::session::context::cleanup_session(&session_id);
}

// ---- background inbox monitor ----

#[tokio::test]
#[serial_test::serial]
async fn inbox_monitor_processes_messages_in_the_background() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("MONITOR-TURN")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	// Let the monitor reach its parked wait state.
	tokio::time::sleep(Duration::from_millis(100)).await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "background work".to_string(),
		},
	);

	let completed = tokio::time::timeout(Duration::from_secs(20), async {
		loop {
			let last = server
				.sessions
				.lock()
				.await
				.get(&session_id)
				.and_then(|s| s.session.messages.last().cloned());
			if matches!(&last, Some(m) if m.role == "assistant" && m.content.contains("MONITOR-TURN"))
			{
				break;
			}
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(
		completed.is_ok(),
		"the monitor must run the background turn and store the session back"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn monitor_skips_processing_while_the_session_lock_is_held() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	tokio::time::sleep(Duration::from_millis(100)).await;

	// Hold the per-session lock BEFORE waking the monitor: it must wake,
	// fail try_lock, and go back to waiting with the message still queued.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let _guard = lock.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "deferred work".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(300)).await;
	assert!(
		inbox_has_messages(&session_id).await,
		"the message must stay queued while the lock is held"
	);

	// Destroy the inbox before releasing so the monitor exits without an
	// API call (no stub is scripted for this test).
	crate::session::context::cleanup_session(&session_id);
	drop(_guard);
}

#[tokio::test]
#[serial_test::serial]
async fn monitor_requeues_the_message_when_the_session_vanishes_mid_drain() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	tokio::time::sleep(Duration::from_millis(100)).await;

	// Hold the locks-map mutex BEFORE waking the monitor: it evaluates
	// has_messages, then parks on the map lock at the acquisition point.
	let map_guard = server.session_locks.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "orphaned work".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	// The session vanishes (as if a prompt took it out) while the monitor
	// is parked on the map lock.
	server.sessions.lock().await.remove(&session_id);
	drop(map_guard);

	tokio::time::sleep(Duration::from_millis(300)).await;
	assert!(
		inbox_has_messages(&session_id).await,
		"the popped message must be pushed back when the session is gone"
	);
	assert!(
		!server.sessions.lock().await.contains_key(&session_id),
		"the monitor must not resurrect the vanished session"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn monitor_defers_when_the_inbox_is_emptied_by_another_consumer() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let session_id = create_session(&mut ws, None).await;
	tokio::time::sleep(Duration::from_millis(100)).await;

	let map_guard = server.session_locks.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "stolen work".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	// Another consumer drains the inbox while the monitor is parked on the
	// map lock.
	let popped = crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::try_pop_inbox_message()
	})
	.await;
	assert!(
		popped.is_some(),
		"the test must consume the message before the monitor wakes"
	);
	drop(map_guard);

	tokio::time::sleep(Duration::from_millis(300)).await;
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the session must stay in the map untouched"
	);
	assert!(
		!inbox_has_messages(&session_id).await,
		"nothing may be requeued"
	);

	crate::session::context::cleanup_session(&session_id);
}
