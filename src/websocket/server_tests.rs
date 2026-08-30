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
	match error {
		ServerMessage::Error(payload) => {
			assert!(payload
				.message
				.contains("Session not found: no-such-session-zzz"));
		}
		other => panic!("expected an error frame, got {other:?}"),
	}
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
