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

#[test]
fn slow_client_output_is_bounded_by_count_and_closes_on_overflow() {
	let (sender, receiver) = WsSender::channel();
	for _ in 0..MAX_QUEUED_FRAMES {
		sender.send(Message::text("x")).unwrap();
	}
	assert!(sender.send(Message::text("overflow")).is_err());
	assert_eq!(receiver.len(), MAX_QUEUED_FRAMES);
	assert!(sender.closed.is_cancelled());
	assert!(sender.send(Message::text("later")).is_err());
}

#[test]
fn outgoing_byte_budget_covers_queued_and_in_flight_frames() {
	let (sender, mut receiver) = WsSender::channel();
	sender.send(Message::text("x".repeat(MAX_QUEUED_BYTES))).unwrap();
	let in_flight = receiver.try_recv().unwrap();
	assert_eq!(sender.bytes.available_permits(), 0);
	drop(in_flight);
	assert_eq!(sender.bytes.available_permits(), MAX_QUEUED_BYTES);
	sender.send(Message::text("x".repeat(MAX_QUEUED_BYTES))).unwrap();
	assert!(sender.send(Message::text("overflow")).is_err());
	assert_eq!(receiver.len(), 1);
}

#[tokio::test]
async fn unused_session_locks_are_reclaimed_without_splitting_live_locks() {
	let locks = SessionLocks::default();
	let first = get_or_create_session_lock("live", &locks).await;
	let guard = Arc::clone(&first).lock_owned().await;
	drop(first);
	for index in 0..1000 {
		drop(get_or_create_session_lock(&format!("missing-{index}"), &locks).await);
	}
	let same = get_or_create_session_lock("live", &locks).await;
	assert!(same.try_lock().is_err());
	assert_eq!(locks.lock().await.len(), 1);
	drop(guard);
	drop(same);
	get_or_create_session_lock("next", &locks).await;
	assert_eq!(locks.lock().await.len(), 1);
}

/// Accepts one loopback connection and returns its heartbeat reader with the
/// given interval, plus the sender its pings go through.
async fn heartbeat_pair(interval: Duration) -> (WsReader, WsSender, tokio::task::JoinHandle<()>, ClientWs) {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let (client, accepted) = tokio::join!(connect_ws(addr), async {
		let (stream, peer) = listener.accept().await.unwrap();
		let (sink, frames) = tokio_tungstenite::accept_async(stream).await.unwrap().split();
		let (sender, writer) = WsSender::spawn(sink);
		(WsReader::new(frames, peer, interval), sender, writer)
	});
	let (reader, sender, writer) = accepted;
	(reader, sender, writer, client)
}

#[tokio::test]
async fn a_silent_peer_is_dropped_after_a_missed_heartbeat() {
	let (mut reader, sender, _writer, _client) = heartbeat_pair(Duration::from_millis(50)).await;
	// The client never reads, so it never answers the ping.
	let frame = tokio::time::timeout(Duration::from_secs(5), reader.next(&sender))
		.await
		.expect("a silent peer must be dropped");
	assert!(frame.is_none());
}

#[tokio::test]
async fn a_peer_answering_pings_stays_connected() {
	// Wide enough that scheduling jitter in a loaded test run cannot delay a pong past one interval.
	let interval = Duration::from_millis(200);
	let (mut reader, sender, _writer, mut client) = heartbeat_pair(interval).await;
	// Reading makes the client answer each ping with a pong.
	let pongs = tokio::spawn(async move { while client.next().await.is_some() {} });
	let deadline = tokio::time::Instant::now() + interval * 5;
	while let Ok(frame) = tokio::time::timeout_at(deadline, reader.next(&sender)).await {
		assert!(
			matches!(frame, Some(Ok(Message::Pong(_)))),
			"expected the connection to stay open, got {frame:?}"
		);
	}
	pongs.abort();
}

#[test]
fn session_admission_preserves_existing_sessions_and_releases_failed_loads() {
	let server = ServerState::new(Arc::new(ws_fake_config()), "assistant".into(), Vec::new());
	let failed = server.reserve_session("failed-load").unwrap();
	assert_eq!(server.residents.lock().unwrap().len(), 1);
	drop(failed);
	assert!(server.residents.lock().unwrap().is_empty());
	for index in 0..MAX_LOADED_SESSIONS {
		server.reserve_session(&format!("session-{index}")).unwrap().committed = true;
	}
	assert!(server.reserve_session("extra").is_err());
	assert!(server.reserve_session("session-0").is_ok());
	assert_eq!(server.residents.lock().unwrap().len(), MAX_LOADED_SESSIONS);
}

async fn deferred_input_limit_preserves_cancellation(content: String, count: usize) {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let handler = tokio::spawn(async move {
		let (stream, peer) = listener.accept().await.unwrap();
		let (sink, frames) = tokio_tungstenite::accept_async(stream).await.unwrap().split();
		let mut reader = WsReader::new(frames, peer, HEARTBEAT_INTERVAL);
		let (sender, writer) = WsSender::spawn(sink);
		let server = ServerState::new(Arc::new(ws_fake_config()), "assistant".into(), Vec::new());
		let ctx = ConnCtx {
			server,
			closed: CancellationToken::new(),
		};
		let mut cancellation = SessionCancellation::new();
		let mut operation = cancellation.new_operation();
		let _turn = TurnGuard::register(&ctx.server, "limited-input", &cancellation);
		let processing = async {
			operation.changed().await?;
			assert!(*operation.borrow());
			Ok(())
		};
		let mut deferred = VecDeque::new();
		read_while_processing(processing, &mut reader, &sender, &ctx, &mut deferred, &mut false, peer)
			.await
			.unwrap();
		assert!(deferred.len() <= MAX_DEFERRED_FRAMES);
		assert!(deferred.iter().map(Message::len).sum::<usize>() <= MAX_DEFERRED_BYTES);
		drop(sender);
		writer.await.unwrap();
	});
	let mut client = connect_ws(addr).await;
	for _ in 0..count {
		send_json(
			&mut client,
			serde_json::json!({
				"type":"message", "session_id":"limited-input", "content":content, "request_id":"queued"
			}),
		)
		.await;
	}
	let error = read_json(&mut client).await;
	assert_eq!(error["type"], "error");
	assert_eq!(error["request_id"], "queued");
	assert!(error["message"].as_str().unwrap().contains("pending input limit"));
	send_json(
		&mut client,
		serde_json::json!({"type":"cancel", "session_id":"limited-input"}),
	)
	.await;
	assert_eq!(read_json(&mut client).await["message_type"], "cancel");
	tokio::time::timeout(Duration::from_secs(5), handler)
		.await
		.unwrap()
		.unwrap();
}

#[tokio::test]
async fn deferred_input_count_is_bounded_while_cancel_still_works() {
	deferred_input_limit_preserves_cancellation("queued".into(), MAX_DEFERRED_FRAMES + 1).await;
}

#[tokio::test]
async fn deferred_input_bytes_are_bounded_while_cancel_still_works() {
	deferred_input_limit_preserves_cancellation("x".repeat(MAX_DEFERRED_BYTES / 2), 2).await;
}

#[tokio::test]
async fn a_failed_control_reply_does_not_drop_the_running_turn() {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let handler = tokio::spawn(async move {
		let (stream, peer) = listener.accept().await.unwrap();
		let (_, frames) = tokio_tungstenite::accept_async(stream).await.unwrap().split();
		let mut reader = WsReader::new(frames, peer, HEARTBEAT_INTERVAL);
		let (sender, receiver) = WsSender::channel();
		drop(receiver);
		let ctx = ConnCtx {
			server: ServerState::new(Arc::new(ws_fake_config()), "assistant".into(), Vec::new()),
			closed: CancellationToken::new(),
		};
		let mut completed = false;
		let processing = async {
			ctx.closed.cancelled().await;
			completed = true;
			Ok(())
		};
		let mut ended = false;
		read_while_processing(
			processing,
			&mut reader,
			&sender,
			&ctx,
			&mut VecDeque::new(),
			&mut ended,
			peer,
		)
		.await
		.unwrap();
		assert!(completed);
		assert!(ended);
	});
	let mut client = connect_ws(addr).await;
	send_json(
		&mut client,
		serde_json::json!({"type":"cancel", "session_id":"still-running"}),
	)
	.await;
	tokio::time::timeout(Duration::from_secs(5), handler)
		.await
		.unwrap()
		.unwrap();
}

fn handshake(origin: Option<&str>, allow_origins: &[&str]) -> Result<Response, Box<ErrorResponse>> {
	handshake_with(origin, None, allow_origins, None)
}

fn handshake_with(
	origin: Option<&str>,
	authorization: Option<&str>,
	allow_origins: &[&str],
	token: Option<&str>,
) -> Result<Response, Box<ErrorResponse>> {
	let mut req = Request::builder().uri("/");
	if let Some(origin) = origin {
		req = req.header(ORIGIN, origin);
	}
	if let Some(authorization) = authorization {
		req = req.header(AUTHORIZATION, authorization);
	}
	let allowlist = allow_origins.iter().map(|o| (*o).to_string()).collect();
	HandshakeGuard {
		origins: Arc::new(allowlist),
		token: token.map(Arc::from),
	}
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

#[test]
fn configured_token_must_be_presented_as_a_bearer() {
	let token = Some("s3cret-token");
	assert!(handshake_with(None, Some("Bearer s3cret-token"), &[], token).is_ok());

	for presented in [None, Some("Bearer wrong"), Some("s3cret-token"), Some("Bearer ")] {
		let err = handshake_with(None, presented, &[], token).unwrap_err();
		assert_eq!(err.status(), StatusCode::UNAUTHORIZED, "{presented:?}");
	}

	// The origin check still runs first: a refused browser never reaches the token.
	let err = handshake_with(
		Some("https://evil.example.com"),
		Some("Bearer s3cret-token"),
		&[],
		token,
	)
	.unwrap_err();
	assert_eq!(err.status(), StatusCode::FORBIDDEN);
}

#[test]
fn without_a_token_no_authorization_is_needed() {
	assert!(handshake_with(None, Some("Bearer anything"), &[], None).is_ok());
	assert!(!constant_time_eq(b"abc", b"abd"));
	assert!(!constant_time_eq(b"abc", b"abcd"));
	assert!(constant_time_eq(b"abc", b"abc"));
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
	image::RgbImage::new(4, 4).save(&media_path).expect("save test image");

	let mut session = ChatSession::for_tests(Vec::new());
	session.model = "openrouter:vendor/unknown-vision-model".to_string();
	let loaded = load_message_attachments(&session, &[attachment], tmp.path()).expect("load websocket attachment");
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
async fn releasing_a_client_turn_wakes_pending_idle_tool_calls_without_inbox_messages() {
	use crate::mcp::orchestration::schedule::storage::{ScheduleAction, ScheduleEntry, ScheduledToolCall};
	use crate::session::context::{cleanup_session, init_session_services, with_session_id};

	let id = format!("ws-idle-schedule-{}", uuid::Uuid::new_v4());
	let notify = with_session_id(id.clone(), async {
		init_session_services("assistant");
		let entry = ScheduleEntry::new_idle(
			String::new(),
			ScheduleAction::ToolCall {
				tool_call: ScheduledToolCall {
					name: "schedule".to_string(),
					arguments: serde_json::json!({"command": "list"}),
				},
				workdir: std::env::current_dir().unwrap(),
			},
			false,
		);
		crate::session::context::get_schedule_storage(&id)
			.lock()
			.unwrap()
			.add(entry);
		assert!(!crate::session::inbox::has_inbox_messages());
		crate::session::inbox::get_inbox_notify().unwrap()
	})
	.await;

	wake_inbox_monitor(id.clone()).await;
	let woke = tokio::time::timeout(Duration::from_secs(1), notify.notified()).await;
	cleanup_session(&id);
	woke.expect("an idle tool call must be reconsidered after the client turn releases its lock");
}

#[tokio::test]
async fn session_lock_is_reused_per_session_id() {
	let locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));
	let a = get_or_create_session_lock("s", &locks).await;
	let b = get_or_create_session_lock("s", &locks).await;
	assert!(Arc::ptr_eq(&a, &b), "same session must share one lock");

	let other = get_or_create_session_lock("other", &locks).await;
	assert!(!Arc::ptr_eq(&a, &other), "different sessions must not share");
}

// ---- lookup_session ----

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
async fn lookup_session_returns_the_memory_copy_and_removes_it() {
	let state = ServerState::new(Arc::new(template_config()), "assistant".to_string(), Vec::new());
	state
		.sessions
		.lock()
		.await
		.insert("in-mem".to_string(), ChatSession::for_tests(Vec::new()));

	let session = lookup_session("in-mem", &state)
		.await
		.expect("memory hit resolves without touching disk");
	assert_eq!(
		session.session.info.name, "test",
		"the exact in-memory instance is returned"
	);
	assert!(
		state.sessions.lock().await.is_empty(),
		"lookup takes the session out for exclusive processing"
	);
}

#[tokio::test]
async fn lookup_session_never_auto_creates_a_missing_session() {
	let state = ServerState::new(Arc::new(template_config()), "assistant".to_string(), Vec::new());
	let error = lookup_session("no-such-session-zzz", &state)
		.await
		.err()
		.expect("a session that exists nowhere must be an error");
	assert!(error.contains("Session not found: no-such-session-zzz"), "{error}");
	assert!(state.sessions.lock().await.is_empty(), "nothing may be auto-created");
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
	assert!(error.to_string().contains("does not support video"), "got: {error}");
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
	assert!(error.to_string().contains("must be a regular file"), "got: {error}");
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
	assert!(error.to_string().contains("multiple matching files"), "got: {error}");
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
		error.to_string().contains("Failed to load image attachment"),
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

	let state = ServerState::new(Arc::new(template_config()), "assistant".to_string(), Vec::new());

	tokio::spawn(async move {
		if let Ok((stream, peer)) = listener.accept().await {
			let _ = handle_connection(stream, peer, state).await;
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
	ws.send(Message::text("{not json")).await.expect("send invalid json");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"].as_str().unwrap_or_default().contains("Invalid JSON"));

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
	ws.send(Message::binary(vec![0u8])).await.expect("send binary");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"]
		.as_str()
		.unwrap_or_default()
		.contains("Unsupported WebSocket message type"));

	// 7. Close terminates the connection cleanly: the server answers with its
	// own close frame (the closing handshake), then the stream ends.
	ws.send(Message::Close(None)).await.expect("send close");
	let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			match ws.next().await {
				Some(Ok(Message::Close(_))) => continue,
				other => break other,
			}
		}
	})
	.await
	.expect("connection must end within the timeout");
	assert!(
		ended.is_none() || matches!(ended, Some(Err(_))),
		"client stream must end after close, got {ended:?}"
	);
}

// ---- full server behavior over a real loopback socket ----

use std::time::Duration;

use crate::session::chat::test_support::{fake_provider_config, final_response, spawn_stub_with_status, ENV_LOCK};
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
		Self { previous, _dir: None }
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
/// its state with the test so it can stage sessions, locks and turns.
struct LoopbackServer {
	addr: std::net::SocketAddr,
	sessions: Arc<Mutex<HashMap<String, ChatSession>>>,
	session_locks: SessionLocks,
	state: ServerState,
}

impl LoopbackServer {
	async fn start(config: Arc<Config>) -> Self {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind loopback listener");
		let addr = listener.local_addr().expect("local addr");
		let state = ServerState::new(config, "assistant".to_string(), Vec::new());
		let sessions = state.sessions.clone();
		let session_locks = state.session_locks.clone();
		let shared = state.clone();
		tokio::spawn(async move {
			while let Ok((stream, peer)) = listener.accept().await {
				let state = state.clone();
				tokio::spawn(async move {
					let _ = handle_connection(stream, peer, state).await;
				});
			}
		});
		Self {
			addr,
			sessions,
			session_locks,
			state: shared,
		}
	}
}

type ClientWs = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_ws(addr: std::net::SocketAddr) -> ClientWs {
	let (ws, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
		.await
		.expect("client connects over loopback");
	ws
}

async fn send_json(ws: &mut ClientWs, value: serde_json::Value) {
	ws.send(Message::text(value.to_string())).await.expect("send frame");
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
async fn read_until(ws: &mut ClientWs, predicate: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
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

fn live_inbox_monitors() -> usize {
	super::LIVE_INBOX_MONITORS.load(std::sync::atomic::Ordering::SeqCst)
}

/// The count is process-global and a previous test's monitor may still be
/// winding down, so both the baseline and the final check poll instead of
/// sampling once — under a fully loaded suite a reap can take seconds.
async fn wait_for_inbox_monitors(target: usize) -> usize {
	for _ in 0..300 {
		if live_inbox_monitors() == target {
			break;
		}
		tokio::time::sleep(Duration::from_millis(50)).await;
	}
	live_inbox_monitors()
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

	ws.send(Message::Ping(vec![1u8, 2, 3].into())).await.expect("send ping");
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

	ws.send(Message::Pong(vec![9u8].into())).await.expect("send pong");
	// The read loop must have continued past the Pong arm: a subsequent
	// invalid frame still gets its protocol error.
	ws.send(Message::text("{not json")).await.expect("send junk");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert!(error["message"].as_str().unwrap_or_default().contains("Invalid JSON"));
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
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let state = ServerState::new(Arc::new(template_config()), "assistant".to_string(), Vec::new());
	let handle = tokio::spawn(async move {
		// Accept inside the task: the client only connects after this spawn,
		// so awaiting accept on the main task would deadlock.
		let (stream, peer) = listener.accept().await.expect("accept");
		handle_connection(stream, peer, state).await
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
	assert_eq!(error["request_id"], "r1", "busy error must echo the request: {error}");
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
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let state = ServerState::new(Arc::new(ws_fake_config()), "assistant".to_string(), Vec::new());
	tokio::spawn(async move {
		if let Ok((stream, peer)) = listener.accept().await {
			let _ = handle_connection(stream, peer, state).await;
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
async fn bind_ack_lists_session_commands_only_for_a_client_that_asks() {
	let _data = TestDataDirGuard::new();
	// The shipped template's custom commands, so `/run <name>` entries are covered too.
	let template: Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).expect("default template parses");
	let custom = template.commands.clone().unwrap_or_default();
	assert!(!custom.is_empty(), "the default template ships a custom command");
	let mut config = ws_fake_config();
	config.commands = template.commands;
	let server = LoopbackServer::start(Arc::new(config)).await;

	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": "commands-1", "capabilities": [COMMANDS_CAPABILITY]}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	let commands = ack["commands"]
		.as_array()
		.unwrap_or_else(|| panic!("commands listed: {ack}"));
	let names: Vec<&str> = commands.iter().map(|c| c["name"].as_str().expect("name")).collect();
	let mut expected: Vec<String> = BUILTIN_COMMANDS
		.iter()
		.filter(|&&(.., reach)| reach == Reach::Anywhere)
		.map(|&(name, ..)| name.trim_start_matches('/').to_string())
		.collect();
	expected.extend(custom.iter().map(|c| format!("run {}", c.name)));
	assert_eq!(
		names, expected,
		"`/help` order, CLI-only left out, custom commands last"
	);
	for cli_only in ["copy", "clear", "new", "exit", "image"] {
		assert!(!names.contains(&cli_only), "{cli_only} cannot run over a websocket");
	}

	let entry = |name: &str| {
		commands
			.iter()
			.find(|c| c["name"] == name)
			.unwrap_or_else(|| panic!("{name} listed: {ack}"))
	};
	assert_eq!(entry("model")["hint"], "[provider:model]");
	assert!(
		entry("info").get("hint").is_none(),
		"a command without arguments has no hint"
	);
	let run = entry(&format!("run {}", custom[0].name));
	assert_eq!(run["hint"], "[input]");
	assert_eq!(run["description"], custom[0].description);

	// A client that does not ask gets the ack it always did.
	let mut plain = connect_ws(server.addr).await;
	let _welcome = read_json(&mut plain).await;
	send_json(
		&mut plain,
		serde_json::json!({"type": "session", "session_id": "commands-2"}),
	)
	.await;
	let ack = read_json(&mut plain).await;
	assert_eq!(ack["type"], "ack", "got: {ack}");
	assert!(ack.get("commands").is_none(), "got: {ack}");

	crate::session::context::cleanup_session(&"commands-1".to_string());
	crate::session::context::cleanup_session(&"commands-2".to_string());
}

// ---- per-session role tool binding ----

/// Server role `assistant` owns core+orchestration; `outbound` swaps
/// orchestration for runtime, so each role has a builtin server the other lacks.
fn two_role_config() -> Config {
	let mut config = ws_fake_config();
	let mut assistant = config.role_map["assistant"].clone();
	assistant.mcp.server_refs = vec!["core".to_string(), "orchestration".to_string()];
	assistant.mcp.allowed_tools = vec!["core:*".to_string(), "orchestration:*".to_string()];
	let mut outbound = assistant.clone();
	outbound.name = "outbound".to_string();
	outbound.mcp.server_refs = vec!["core".to_string(), "runtime".to_string()];
	outbound.mcp.allowed_tools = vec!["core:*".to_string(), "runtime:*".to_string()];
	config.role_map.insert("assistant".to_string(), assistant);
	config.roles.push(outbound.clone());
	config.role_map.insert("outbound".to_string(), outbound);
	config
}

/// Registers and enables an in-memory agent owned by `session_id` alone.
async fn register_session_agent(session_id: &str, name: &str) {
	crate::session::context::with_session_id(session_id.to_string(), async {
		crate::mcp::runtime::dynamic_agents::register_agent(crate::mcp::runtime::dynamic_agents::DynamicAgentConfig {
			name: name.to_string(),
			description: "session-owned probe".to_string(),
			system: "probe".to_string(),
			welcome: String::new(),
			model: None,
			temperature: None,
			top_p: None,
			top_k: None,
			server_refs: Vec::new(),
			allowed_tools: Vec::new(),
			workdir: ".".to_string(),
		})
		.expect("register agent");
		crate::mcp::runtime::dynamic_agents::enable_agent(name).expect("enable agent");
	})
	.await;
}

/// Runs one turn whose scripted model reply calls `tool`, returning that call's
/// `tool_result` frame once the turn has closed.
async fn tool_turn(ws: &mut ClientWs, session_id: &str, tool: &str) -> serde_json::Value {
	send_json(
		ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": tool}),
	)
	.await;
	let result = read_until(ws, |v| v["type"] == "tool_result").await;
	read_until(ws, |v| v["type"] == "cost").await;
	result
}

fn assert_refused(frame: &serde_json::Value) {
	assert_eq!(frame["success"], false, "got: {frame}");
	assert!(
		frame["content"].as_str().unwrap_or_default().contains("not found"),
		"got: {frame}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn role_switch_binds_each_session_to_its_own_tools() {
	let _data = TestDataDirGuard::new();
	let calls = [
		("mcp", serde_json::json!({"action": "list"})),
		("schedule", serde_json::json!({"command": "list"})),
		("mcp", serde_json::json!({"action": "list"})),
		("schedule", serde_json::json!({"command": "list"})),
		("agent_outbound_probe", serde_json::json!({"task": "x"})),
		("agent_default_probe", serde_json::json!({"task": "x"})),
	];
	let script = calls
		.into_iter()
		.flat_map(|(tool, args)| {
			[
				crate::session::chat::test_support::tool_call_response(tool, args),
				final_response("done"),
			]
		})
		.collect();
	let _env = StubEnv::new(script).await;
	let config = two_role_config();
	// `octomind server` builds the process-wide tool map for its role at startup.
	crate::mcp::initialize_mcp_for_role("assistant", &config)
		.await
		.expect("server MCP init");
	let server = LoopbackServer::start(Arc::new(config)).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let switched = create_session(&mut ws, Some("role-bind-switched")).await;
	let default = create_session(&mut ws, Some("role-bind-default")).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": switched, "command": "role", "args": ["outbound"]
		}),
	)
	.await;
	let status = read_until(&mut ws, |v| v["type"] == "status").await;
	assert!(status["data"].to_string().contains("outbound"), "got: {status}");

	// Each session dispatches the tools of its own role...
	let own = tool_turn(&mut ws, &switched, "mcp").await;
	assert_eq!(own["success"], true, "switched session lost its role tool: {own}");
	let own = tool_turn(&mut ws, &default, "schedule").await;
	assert_eq!(own["success"], true, "default session lost its role tool: {own}");

	// ...and never the other role's.
	assert_refused(&tool_turn(&mut ws, &default, "mcp").await);
	assert_refused(&tool_turn(&mut ws, &switched, "schedule").await);

	// Tools one session registered at runtime stay invisible to the other.
	register_session_agent(&switched, "outbound_probe").await;
	register_session_agent(&default, "default_probe").await;
	assert_refused(&tool_turn(&mut ws, &default, "agent_outbound_probe").await);
	assert_refused(&tool_turn(&mut ws, &switched, "agent_default_probe").await);

	crate::session::context::cleanup_session(&switched);
	crate::session::context::cleanup_session(&default);
}

#[tokio::test]
#[serial_test::serial]
async fn role_switch_keeps_the_servers_other_sessions_use_running() {
	const STUB: &str = "ws-role-shared-stub";
	let _data = TestDataDirGuard::new();
	let echo = || crate::session::chat::test_support::tool_call_response("echo", serde_json::json!({"msg": "hi"}));
	let _env = StubEnv::new(vec![echo(), final_response("done"), echo(), final_response("done")]).await;
	// Only the server role references the stdio server, so the switch away from
	// that role must leave it to the session still on it.
	let mut config = two_role_config();
	config.mcp.servers.push(crate::config::McpServerConfig::Stdin {
		name: STUB.to_string(),
		command: "python3".to_string(),
		args: vec![concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp_stub_server.py").to_string()],
		timeout_seconds: 10,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	});
	let assistant = config.role_map.get_mut("assistant").expect("assistant role");
	assistant.mcp.server_refs.push(STUB.to_string());
	assistant.mcp.allowed_tools.push(format!("{STUB}:*"));
	crate::mcp::initialize_mcp_for_role("assistant", &config)
		.await
		.expect("server MCP init");
	let server = LoopbackServer::start(Arc::new(config)).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	let user = create_session(&mut ws, Some("shared-stub-user")).await;
	let switcher = create_session(&mut ws, Some("shared-stub-switcher")).await;
	let used = tool_turn(&mut ws, &user, "echo").await;
	assert_eq!(used["success"], true, "got: {used}");
	let live = crate::mcp::client::get(STUB).expect("stub server connected");

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "command", "session_id": switcher, "command": "role", "args": ["outbound"]
		}),
	)
	.await;
	let status = read_until(&mut ws, |v| v["type"] == "status").await;
	assert!(status["data"].to_string().contains("outbound"), "got: {status}");

	// The very connection the user session's calls ride on — a restarted server
	// would be a new one, and every call in flight at the switch would be lost.
	let after = crate::mcp::client::get(STUB).expect("stub server must survive the switch");
	assert!(Arc::ptr_eq(&live, &after), "stub server was restarted");
	assert!(!after.is_closed(), "stub server connection was closed");
	let used = tool_turn(&mut ws, &user, "echo").await;
	assert_eq!(used["success"], true, "got: {used}");

	let _ = crate::mcp::process::cleanup_server_process(STUB);
	crate::session::context::cleanup_session(&user);
	crate::session::context::cleanup_session(&switcher);
}

#[tokio::test]
#[serial_test::serial]
async fn restarted_server_resumes_a_switched_session_under_its_own_role() {
	let _data = TestDataDirGuard::new();
	let script = (0..2)
		.flat_map(|_| {
			[
				crate::session::chat::test_support::tool_call_response("mcp", serde_json::json!({"action": "list"})),
				final_response("done"),
			]
		})
		.collect();
	let _env = StubEnv::new(script).await;
	let config = Arc::new(two_role_config());
	crate::mcp::initialize_mcp_for_role("assistant", &config)
		.await
		.expect("server MCP init");

	let session = {
		let server = LoopbackServer::start(Arc::clone(&config)).await;
		let mut ws = connect_ws(server.addr).await;
		let _welcome = read_json(&mut ws).await;
		let session = create_session(&mut ws, Some("role-restore")).await;
		send_json(
			&mut ws,
			serde_json::json!({
				"type": "command", "session_id": session, "command": "role", "args": ["outbound"]
			}),
		)
		.await;
		let status = read_until(&mut ws, |v| v["type"] == "status").await;
		assert!(status["data"].to_string().contains("outbound"), "got: {status}");
		session
	};

	// Each pass is a process restart: a fresh server and no session-scoped state,
	// only the session file. The first turn comes from a client that never sends
	// `/role` — once after a `session` rebind, once as a bare message.
	for rebind in [true, false] {
		crate::session::context::cleanup_session(&session);
		let server = LoopbackServer::start(Arc::clone(&config)).await;
		let mut ws = connect_ws(server.addr).await;
		let _welcome = read_json(&mut ws).await;
		if rebind {
			create_session(&mut ws, Some(&session)).await;
		}
		let own = tool_turn(&mut ws, &session, "mcp").await;
		assert_eq!(
			own["success"], true,
			"rebind={rebind}: resumed session lost its role tool: {own}"
		);
	}

	crate::session::context::cleanup_session(&session);
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
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
	let addr = listener.local_addr().expect("addr");
	let state = ServerState::new(Arc::new(ws_fake_config()), "assistant".to_string(), Vec::new());
	tokio::spawn(async move {
		if let Ok((stream, peer)) = listener.accept().await {
			let _ = handle_connection(stream, peer, state).await;
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

/// The sink emits a finished turn's cost frame. The server used to send its own
/// copy right behind it, and clients read that as a second finished turn
/// (octomind-api stored every `pending_work` cost twice).
#[tokio::test]
#[serial_test::serial]
async fn a_finished_turn_sends_exactly_one_cost_frame() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("ONE-COST-OK")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session = create_session(&mut ws, Some("one-cost-1")).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": session, "content": "hello", "request_id": "m1"
		}),
	)
	.await;
	read_until(&mut ws, |v| v["type"] == "cost").await;

	// Whatever the turn still sends arrives right behind its cost.
	while let Ok(Some(Ok(frame))) = tokio::time::timeout(Duration::from_millis(1500), ws.next()).await {
		if let Message::Text(text) = frame {
			let value: serde_json::Value = serde_json::from_str(&text).expect("server frames are JSON");
			assert_ne!(value["type"], "cost", "a second cost frame for one turn: {value}");
		}
	}

	crate::session::context::cleanup_session(&session);
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
		error["message"].as_str().unwrap_or_default().contains("Internal error"),
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
		assistant["content"].as_str().unwrap_or_default().contains("WS-TURN-OK"),
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
	server.sessions.lock().await.insert("vision-1".to_string(), {
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
		error["message"].as_str().unwrap_or_default().starts_with("Error:"),
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
			.contains("was not found when the session started"),
		"got: {error}"
	);

	crate::session::context::cleanup_session(&session_id);
}

/// The same guardrail as above, reached WITHOUT touching the process cwd: the
/// bind frame's `cwd` alone must make the session load `.agents/` from there.
/// This is how octomind-api gives each octo its own workspace on one machine.
#[tokio::test]
#[serial_test::serial]
async fn session_cwd_scopes_guardrails_without_chdir() {
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
	assert_ne!(
		std::env::current_dir().expect("cwd"),
		workdir.path(),
		"the process must NOT be in the workspace, or this proves nothing"
	);

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "session",
			"session_id": "octo-cwd-1",
			"cwd": workdir.path().to_string_lossy(),
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack", "got: {ack}");
	let status = read_json(&mut ws).await;
	assert_eq!(status["type"], "status", "got: {status}");

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": "octo-cwd-1", "content": "hello"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("was not found when the session started"),
		"the workspace's pipe must have run — got: {error}"
	);

	crate::session::context::cleanup_session(&"octo-cwd-1".to_string());
}

/// A directory that does not exist is refused before any session is created
/// or taken out of memory.
#[tokio::test]
#[serial_test::serial]
async fn session_cwd_that_is_not_a_directory_is_refused() {
	let _data = TestDataDirGuard::new();
	// Must be absolute on every OS (`/foo` is not on Windows) so it passes
	// protocol validation and reaches the directory check.
	let parent = tempfile::tempdir().expect("tempdir");
	let missing = parent.path().join("missing-octo-workspace");
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "session",
			"session_id": "octo-cwd-missing",
			"cwd": missing.to_string_lossy(),
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("cwd is not a directory"),
		"got: {error}"
	);
	assert!(
		!server.sessions.lock().await.contains_key("octo-cwd-missing"),
		"a refused bind must not create the session"
	);
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
		Self { previous, _dir: dir }
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
		std::path::Path::new(&std::env::var("OCTOMIND_MEDIA_ROOT").expect("media root")).join(format!("{id}.mp4")),
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
		error["message"].as_str().unwrap_or_default().contains("ceiling"),
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
		std::path::Path::new(&std::env::var("OCTOMIND_MEDIA_ROOT").expect("media root")).join(format!("{id}.flv")),
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
	let path =
		std::path::Path::new(&std::env::var("OCTOMIND_MEDIA_ROOT").expect("media root")).join(format!("{id}.mp3"));
	std::fs::write(&path, b"fake-audio-bytes").expect("audio fixture");
	std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o000)).expect("chmod 000");

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
		assistant["content"].as_str().unwrap_or_default().contains("AFTER-DONE"),
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
		error["message"].as_str().unwrap_or_default().contains("Command failed"),
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
	server
		.sessions
		.lock()
		.await
		.insert("save-fail-1".to_string(), ChatSession::for_tests(Vec::new()));
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

#[tokio::test]
async fn client_admission_waits_for_background_setup_and_teardown() {
	let server = ServerState::new(Arc::new(template_config()), "developer".into(), Vec::new());
	let session_id = "background-handoff";
	let lock = get_or_create_session_lock(session_id, &server.session_locks).await;
	for after_turn in [false, true] {
		let background = BackgroundTurnGuard::try_lock(&server, session_id, &lock).unwrap();
		if after_turn {
			let cancellation = SessionCancellation::new();
			drop(TurnGuard::register(&server, session_id, &cancellation));
		}
		let client = lock_for_client(&lock, &server, session_id);
		tokio::pin!(client);
		assert!(futures::poll!(&mut client).is_pending());
		drop(background);
		assert!(
			BackgroundTurnGuard::try_lock(&server, session_id, &lock).is_none(),
			"queued client must precede the next background batch"
		);
		let foreground = client.await.expect("background work must not refuse the client");
		assert!(lock_for_client(&lock, &server, session_id).await.is_none());
		assert!(BackgroundTurnGuard::try_lock(&server, session_id, &lock).is_none());
		drop(foreground);
		assert!(lock_for_client(&lock, &server, session_id).await.is_some());
	}
}

#[tokio::test]
async fn aborted_background_task_releases_client_admission() {
	let server = ServerState::new(Arc::new(template_config()), "developer".into(), Vec::new());
	let session_id = "aborted-background";
	let lock = get_or_create_session_lock(session_id, &server.session_locks).await;
	let task_server = server.clone();
	let task_lock = Arc::clone(&lock);
	let (started_tx, started_rx) = tokio::sync::oneshot::channel();
	let task = tokio::spawn(async move {
		let _background = BackgroundTurnGuard::try_lock(&task_server, session_id, &task_lock).unwrap();
		started_tx.send(()).unwrap();
		std::future::pending::<()>().await;
	});
	started_rx.await.unwrap();
	let client = lock_for_client(&lock, &server, session_id);
	tokio::pin!(client);
	assert!(futures::poll!(&mut client).is_pending());
	task.abort();
	assert!(task.await.unwrap_err().is_cancelled());
	let foreground = client.await.unwrap();
	assert!(lock_for_client(&lock, &server, session_id).await.is_none());
	drop(foreground);
	assert!(BackgroundTurnGuard::try_lock(&server, session_id, &lock).is_some());
}

#[tokio::test]
async fn user_message_during_another_clients_turn_reports_busy() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = "busy-user";
	let lock = get_or_create_session_lock(session_id, &server.session_locks).await;
	let _guard = lock.lock().await;
	let cancellation = SessionCancellation::new();
	let _turn = TurnGuard::register(&server.state, session_id, &cancellation);
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": session_id, "content": "hello", "request_id": "busy-message"
		}),
	)
	.await;
	assert_eq!(read_json(&mut ws).await["type"], "ack");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert_eq!(error["request_id"], "busy-message");
	assert_eq!(
		error["message"],
		format!(
			"Session '{}' is busy processing another request. Please wait.",
			session_id
		)
	);
}

#[tokio::test]
#[serial_test::serial]
async fn client_frames_wait_for_background_turns_on_completion_or_cancel() {
	let _data = TestDataDirGuard::new();
	let env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let monitor = InboxSource::Monitor {
		id: "monitor".into(),
		description: "background output".into(),
	};
	for (kind, source, cancel) in [
		("message", monitor.clone(), true),
		("command", monitor, true),
		(
			"message",
			InboxSource::TapRun {
				id: "tap".into(),
				role: "assistant".into(),
			},
			false,
		),
		("command", InboxSource::BackgroundJob { id: "job".into() }, false),
	] {
		let mut ws = connect_ws(server.addr).await;
		let _welcome = read_json(&mut ws).await;
		let session_id = create_session(&mut ws, None).await;
		push_inbox_message_for_session(
			&session_id,
			InboxMessage {
				source,
				content: "background output".into(),
			},
		);
		read_until(&mut ws, |frame| frame["type"] == "injected").await;
		env.arrived().notified().await;
		let request = if kind == "message" {
			serde_json::json!({"type": kind, "session_id": session_id, "content": "user wins", "request_id": "foreground"})
		} else {
			serde_json::json!({"type": kind, "session_id": session_id, "command": "info", "request_id": "foreground"})
		};
		send_json(&mut ws, request).await;
		let ack = read_json(&mut ws).await;
		assert_eq!(ack["request_id"], "foreground", "got: {ack}");
		if kind == "message" {
			push_inbox_message_for_session(
				&session_id,
				InboxMessage {
					source: InboxSource::Monitor {
						id: "pending".into(),
						description: "queued output".into(),
					},
					content: "monitor must wait".into(),
				},
			);
		}
		if cancel {
			send_json(&mut ws, serde_json::json!({"type": "cancel", "session_id": session_id})).await;
		}
		let background = frames_through_cost(&mut ws).await;
		assert!(
			!background.iter().any(|frame| frame["type"] == "error"),
			"got: {background:?}"
		);
		assert_eq!(
			background.iter().any(|frame| frame["message_type"] == "cancel"),
			cancel,
			"got: {background:?}"
		);
		assert_eq!(background.last().unwrap()["cancelled"] == true, cancel);
		if kind == "message" {
			let foreground = frames_through_cost(&mut ws).await;
			assert!(
				!foreground.iter().any(|frame| frame["type"] == "error"),
				"got: {foreground:?}"
			);
			assert!(
				foreground.iter().any(|frame| frame["type"] == "assistant"),
				"got: {foreground:?}"
			);
			let cost = foreground.last().unwrap();
			assert_eq!(cost["request_id"], "foreground");
			assert_ne!(
				cost["cancelled"], true,
				"stopping background work must not stop the user's turn"
			);
			assert!(
				!foreground.iter().any(|frame| frame["type"] == "injected"),
				"got: {foreground:?}"
			);
			let injected = read_until(&mut ws, |frame| frame["type"] == "injected").await;
			assert_eq!(injected["content"], "monitor must wait");
			let after_user = frames_through_cost(&mut ws).await;
			assert_ne!(after_user.last().unwrap()["cancelled"], true);
		} else {
			let status = read_json(&mut ws).await;
			assert_eq!(status["type"], "status", "got: {status}");
			assert!(status["data"].is_object());
		}
		let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
		let _guard = lock.lock().await;
		assert!(!server.state.background_turns.lock().unwrap().contains(&session_id));
		crate::session::context::cleanup_session(&session_id);
	}
}

#[tokio::test]
#[serial_test::serial]
async fn cancelled_background_turn_leaves_the_next_batch_for_a_new_wakeup() {
	let _data = TestDataDirGuard::new();
	let env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "first".into(),
		},
	);
	read_until(&mut ws, |frame| frame["type"] == "injected").await;
	env.arrived().notified().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "next".into(),
		},
	);
	// Consume the queued notification so only an explicit later wakeup can start another pass.
	crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::get_inbox_notify().unwrap().notified().await;
	})
	.await;
	send_json(&mut ws, serde_json::json!({"type": "cancel", "session_id": session_id})).await;
	let frames = frames_through_cost(&mut ws).await;
	assert_eq!(frames.last().unwrap()["cancelled"], true);
	const QUIET_WINDOW: Duration = Duration::from_millis(200);
	assert!(
		tokio::time::timeout(QUIET_WINDOW, ws.next()).await.is_err(),
		"cancel must not chain into the next batch"
	);
	assert!(inbox_has_messages(&session_id).await);
	wake_inbox_monitor(session_id.clone()).await;
	let injected = read_until(&mut ws, |frame| frame["type"] == "injected").await;
	assert_eq!(injected["content"], "next");
	let frames = frames_through_cost(&mut ws).await;
	assert_ne!(frames.last().unwrap()["cancelled"], true);
	crate::session::context::cleanup_session(&session_id);
}

// ---- pre-user inbox drain ----

#[tokio::test]
#[serial_test::serial]
async fn pre_user_inbox_drain_batches_results_and_keeps_user_injections_separate() {
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
	// Two system-managed messages (schedule, finished job) answer in ONE turn;
	// the plain inject behind them carries its own task and gets its own turn.
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
			source: InboxSource::BackgroundJob {
				id: "job-1".to_string(),
			},
			content: "job finished".to_string(),
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
						&& matches!(
							frame["content"].as_str().unwrap_or_default(),
							"scheduled reminder" | "job finished" | "injected before the user turn"
						),
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
	assert_eq!(injected, 3, "every inbox message must be announced");
	assert_eq!(
		inbox_assistants, 2,
		"the two results answer in one turn, the user injection in its own"
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
			if matches!(&last, Some(m) if m.role == "assistant" && m.content.contains("MONITOR-TURN")) {
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

/// Sends a bind for `session_id` on a fresh connection.
async fn bind_from_another_connection(addr: std::net::SocketAddr, session_id: &str) -> ClientWs {
	let mut ws = connect_ws(addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": session_id}),
	)
	.await;
	ws
}

async fn read_bind_answer(ws: &mut ClientWs, session_id: &str) {
	let ack = read_json(ws).await;
	assert_eq!(ack["type"], "ack", "got: {ack}");
	assert_eq!(ack["message_type"], "session", "got: {ack}");
	let status = read_json(ws).await;
	assert_eq!(status["session_id"], session_id, "got: {status}");
}

async fn stays_unanswered(ws: &mut ClientWs) -> bool {
	tokio::time::timeout(Duration::from_millis(300), ws.next())
		.await
		.is_err()
}

#[tokio::test]
#[serial_test::serial]
async fn a_bind_while_a_turn_runs_attaches_without_loading_a_copy() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut first = connect_ws(server.addr).await;
	let _welcome = read_json(&mut first).await;
	let session_id = create_session(&mut first, None).await;

	// A turn drives the session: it holds the lock and has the session out of memory.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	let live = server
		.sessions
		.lock()
		.await
		.remove(&session_id)
		.expect("the bound session is in memory");
	let cancellation = SessionCancellation::new();
	let turn = TurnGuard::register(&server.state, &session_id, &cancellation);

	// Another connection binds meanwhile — a phone reopening the chat.
	let mut second = bind_from_another_connection(server.addr, &session_id).await;
	read_bind_answer(&mut second, &session_id).await;
	assert!(
		!server.sessions.lock().await.contains_key(&session_id),
		"the bind must not load a second copy of a session a turn drives"
	);

	// The turn ends: the session it drove is the one in memory.
	drop(turn);
	server.sessions.lock().await.insert(session_id.clone(), live);
	drop(guard);
	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn a_bind_waits_out_a_brief_holder_of_the_session_then_binds() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut first = connect_ws(server.addr).await;
	let _welcome = read_json(&mut first).await;
	let session_id = create_session(&mut first, None).await;

	// Something other than a turn holds the session: another bind setting it up, a close.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	let mut second = bind_from_another_connection(server.addr, &session_id).await;
	assert!(stays_unanswered(&mut second).await, "the bind must wait for the holder");

	drop(guard);
	read_bind_answer(&mut second, &session_id).await;
	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn a_bind_waiting_on_the_session_attaches_once_a_turn_takes_it() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut first = connect_ws(server.addr).await;
	let _welcome = read_json(&mut first).await;
	let session_id = create_session(&mut first, None).await;

	// A frame holds the session — still loading it — and only then starts its turn.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	let mut second = bind_from_another_connection(server.addr, &session_id).await;
	assert!(
		stays_unanswered(&mut second).await,
		"the bind must wait while no turn runs"
	);

	let cancellation = SessionCancellation::new();
	let turn = TurnGuard::register(&server.state, &session_id, &cancellation);
	read_bind_answer(&mut second, &session_id).await;

	drop(turn);
	drop(guard);
	crate::session::context::cleanup_session(&session_id);
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
	assert!(!inbox_has_messages(&session_id).await, "nothing may be requeued");

	crate::session::context::cleanup_session(&session_id);
}

// ---- handback with no client connected ----

/// A `tap` run outlives the turn that started it. The driver closes the socket
/// on the terminal cost frame (octomind-api `AcpRunner::stream`), so when the
/// run finishes minutes later there is no live connection. The monitor belongs
/// to the loaded session, not to the connection, so it answers the handback
/// anyway; the frames wait on the session's stream for whoever resumes it.
///
/// Observed in prod 2026-09-12 — `tap list` reported `done`, the user was told
/// "lands in my next message", and nothing ever arrived.
#[tokio::test]
#[serial_test::serial]
async fn a_handback_landing_after_the_socket_closed_runs_with_no_client_and_a_resume_replays_it() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("HANDBACK-TURN")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let session_id = "handback-1";

	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": session_id}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	let stream = ack["stream"]
		.as_str()
		.expect("the bind ack names the stream")
		.to_string();
	let read_to = ack["stream_seq"].as_u64().expect("the ack reports the stream position");
	let _greeting = read_json(&mut ws).await;

	// The turn ended and the driver hung up.
	ws.close(None).await.ok();
	drop(ws);
	tokio::time::sleep(Duration::from_millis(200)).await;

	push_inbox_message_for_session(
		session_id,
		InboxMessage {
			source: InboxSource::TapRun {
				id: "tap-1".to_string(),
				role: "assistant:researcher".to_string(),
			},
			content: "[Tap-run 'tap-1' completed]\n\nthe research".to_string(),
		},
	);

	let answered = tokio::time::timeout(Duration::from_secs(20), async {
		loop {
			let last = server
				.sessions
				.lock()
				.await
				.get(session_id)
				.and_then(|s| s.session.messages.last().cloned());
			if matches!(&last, Some(m) if m.role == "assistant" && m.content.contains("HANDBACK-TURN")) {
				break;
			}
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(
		answered.is_ok(),
		"the monitor must answer the handback with nobody connected"
	);
	assert!(!inbox_has_messages(session_id).await, "the handback was consumed");

	// The next bind resumes the stream and reads what it missed.
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(
		&mut ws,
		serde_json::json!({
			"type": "session", "session_id": session_id,
			"resume": {"stream": stream, "after": read_to}
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["resumed"], true, "got: {ack}");
	let replayed = frames_through_cost(&mut ws).await;
	let seqs: Vec<u64> = replayed
		.iter()
		.map(|frame| frame["stream_seq"].as_u64().expect("replayed frames are numbered"))
		.collect();
	assert_eq!(
		seqs,
		(read_to + 1..=read_to + seqs.len() as u64).collect::<Vec<_>>(),
		"every missed frame once, in order"
	);
	assert!(
		replayed
			.iter()
			.any(|f| f["type"] == "injected" && f["content"].as_str().unwrap_or_default().contains("the research")),
		"the handback is announced: {replayed:?}"
	);
	assert!(
		replayed
			.iter()
			.any(|f| f["type"] == "assistant" && f["content"].as_str().unwrap_or_default().contains("HANDBACK-TURN")),
		"and answered: {replayed:?}"
	);

	crate::session::context::cleanup_session(&session_id.to_string());
}

// ---- pending work: the cost frame tells a driver more is coming ----

/// A turn that delegates ends on its promise — "the researcher is on it" — while
/// the run works for minutes. Its result streams back as a follow-up turn, and
/// only to a connection that is still open; octomind-api hung up on the first
/// cost frame, so routines settled on the promise and chats replied with it
/// (dev, 2026-09-14). The cost frame has to say that more is coming.
#[tokio::test]
#[serial_test::serial]
async fn a_turn_that_leaves_a_tap_run_going_says_so_on_its_cost_frame() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("ON-IT"), final_response("NOTHING-LEFT")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	let status = Arc::new(std::sync::RwLock::new(crate::session::tap_runs::TapJobStatus::Running));
	let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
	let job = crate::session::tap_runs::TapJob {
		id: "tap-researcher-000001".to_string(),
		role: "assistant:researcher".to_string(),
		workdir: String::new(),
		started_at: std::time::SystemTime::now(),
		status: Arc::clone(&status),
		error: None,
		cancel_tx,
		live: Arc::new(std::sync::RwLock::new(crate::session::tap_runs::TapLiveState::default())),
	};
	crate::session::context::with_session_id(session_id.clone(), async move {
		crate::session::tap_runs::register_job(job);
	})
	.await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "research this"}),
	)
	.await;
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(cost["pending_work"], true, "the run is still going: {cost}");

	*status.write().expect("status lock") = crate::session::tap_runs::TapJobStatus::Done;
	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "anything else?"}),
	)
	.await;
	read_until(&mut ws, |v| {
		v["type"] == "assistant" && v["content"].as_str().unwrap_or_default().contains("NOTHING-LEFT")
	})
	.await;
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert!(
		cost.get("pending_work").is_none(),
		"nothing is left to hand back, so the frame reads as it always did: {cost}"
	);

	crate::session::context::cleanup_session(&session_id);
}

/// Parks the session's live monitor with a handback still queued: the push wakes
/// it while the session lock is held (as a client frame would hold it), so it
/// gives up its pass and parks, and releasing the lock does not wake it again.
async fn park_a_handback_behind_the_session_lock(server: &LoopbackServer, session_id: &str) {
	tokio::time::sleep(Duration::from_millis(100)).await;
	let lock = get_or_create_session_lock(session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	push_inbox_message_for_session(
		session_id,
		InboxMessage {
			source: InboxSource::TapRun {
				id: "tap-1".to_string(),
				role: "assistant:researcher".to_string(),
			},
			content: "[Tap-run 'tap-1' completed]\n\nthe research".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(300)).await;
	drop(guard);
	tokio::time::sleep(Duration::from_millis(300)).await;
	assert!(
		inbox_has_messages(session_id).await,
		"precondition: the parked monitor left the handback queued"
	);
}

/// A handback that lands while a client frame holds the session lock wakes the
/// monitor, which fails `try_lock` and parks again — and a turn's own wake-up
/// fires BEFORE that lock is released. Nothing woke the monitor afterwards, so a
/// driver told `pending_work` would wait out its cap for a turn that never ran.
#[tokio::test]
#[serial_test::serial]
async fn a_handback_parked_behind_a_client_frame_runs_once_that_frame_lets_go() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("HANDBACK-AFTER-LOCK")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	park_a_handback_behind_the_session_lock(&server, &session_id).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": session_id, "command": "info"}),
	)
	.await;
	read_until(&mut ws, |v| {
		v["type"] == "assistant"
			&& v["content"]
				.as_str()
				.unwrap_or_default()
				.contains("HANDBACK-AFTER-LOCK")
	})
	.await;

	crate::session::context::cleanup_session(&session_id);
}

/// A handback still queued when the client's next message arrives is answered
/// first, inside that message's frame, and closes with a cost frame of its own.
/// octomind-api reads a cost frame as the end of the turn it sent, so it hung up
/// on the handback and the reply to its own message was never recorded.
#[tokio::test]
#[serial_test::serial]
async fn a_handback_answered_ahead_of_a_message_does_not_end_that_message_turn() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("HANDBACK-FIRST"), final_response("REPLY-SECOND")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	park_a_handback_behind_the_session_lock(&server, &session_id).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "any update?"}),
	)
	.await;
	let handback = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(
		handback["pending_work"], true,
		"the message's own turn is still to come: {handback}"
	);
	read_until(&mut ws, |v| {
		v["type"] == "assistant" && v["content"].as_str().unwrap_or_default().contains("REPLY-SECOND")
	})
	.await;
	let reply = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert!(reply.get("pending_work").is_none(), "got: {reply}");

	crate::session::context::cleanup_session(&session_id);
}

/// A background inbox turn that fails must say so. The pre-user drain in this
/// same file already does (`Error processing injected message`), but the
/// monitor only `log_debug!`s — invisible at the info level machines run at —
/// and then emits a Cost frame regardless. To every client that is an ordinary
/// finished turn: our FrameTee treats `cost` as terminal, so the connector
/// resolves the chat with "the agent sent no reply" and the real failure is
/// never recorded anywhere.
///
/// Matches an unexplained prod pair on 2026-09-12 — two cost frames 14s apart
/// carrying identical token counts, i.e. a drain whose API call did nothing.
#[tokio::test]
#[serial_test::serial]
async fn a_failing_background_inbox_turn_reports_an_error_frame() {
	let _data = TestDataDirGuard::new();
	let failures: Vec<(u16, serde_json::Value)> = (0..6)
		.map(|_| (500u16, serde_json::json!({"error": {"message": "boom"}})))
		.collect();
	let _env = StubEnv::with_status(failures).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	// The connection stays open, so this session's own live monitor drains it.
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::TapRun {
				id: "tap-1".to_string(),
				role: "assistant:researcher".to_string(),
			},
			content: "[Tap-run 'tap-1' completed]\n\nthe research".to_string(),
		},
	);

	let error = read_until(&mut ws, |v| v["type"] == "error").await;
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("injected message"),
		"got: {error}"
	);

	crate::session::context::cleanup_session(&session_id);
}

/// One monitor per LOADED SESSION, owned by the server.
///
/// octomind-api re-sends the `session` bind ahead of every turn, so a session is
/// bound again and again, from one connection or several. All of them share one
/// monitor; closing a connection leaves it alone; it ends when the session is
/// unloaded, so nothing leaks for the life of the process.
#[tokio::test]
#[serial_test::serial]
async fn the_session_monitor_is_one_per_loaded_session_and_ends_only_when_it_unloads() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	// Baseline once any straggler from an earlier test has been reaped.
	let before = wait_for_inbox_monitors(0).await;

	let mut first = connect_ws(server.addr).await;
	let _welcome = read_json(&mut first).await;
	let session_id = create_session(&mut first, None).await;
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"the first bind starts the session's monitor"
	);

	// Re-binding the same session on the same connection must not add another.
	let rebound = create_session(&mut first, Some(&session_id)).await;
	assert_eq!(rebound, session_id);
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"a re-bind on the same connection is not a second monitor"
	);

	let mut second = connect_ws(server.addr).await;
	let _welcome = read_json(&mut second).await;
	create_session(&mut second, Some(&session_id)).await;
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"nor is a bind from another connection"
	);

	first.close(None).await.ok();
	drop(first);
	tokio::time::sleep(Duration::from_millis(300)).await;
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"a closing connection does not stop the session's monitor"
	);

	send_json(
		&mut second,
		serde_json::json!({"type": "close", "session_id": session_id}),
	)
	.await;
	let _ack = read_json(&mut second).await;
	let closed = read_json(&mut second).await;
	assert_eq!(closed["data"]["command_type"], "close", "got: {closed}");
	assert_eq!(
		wait_for_inbox_monitors(before).await,
		before,
		"unloading the session ends its monitor"
	);

	// Loaded again, the session gets a monitor of its own.
	create_session(&mut second, Some(&session_id)).await;
	assert_eq!(live_inbox_monitors() - before, 1);

	crate::session::context::cleanup_session(&session_id);
}

/// A session that enters memory through a message or command, with no bind on
/// the connection, is monitored like a bound one.
#[tokio::test]
#[serial_test::serial]
async fn a_session_loaded_lazily_by_a_command_gets_its_monitor() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let before = wait_for_inbox_monitors(0).await;

	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	send_json(&mut ws, serde_json::json!({"type": "close", "session_id": session_id})).await;
	let _ack = read_json(&mut ws).await;
	let _closed = read_json(&mut ws).await;
	assert_eq!(wait_for_inbox_monitors(before).await, before);

	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": session_id, "command": "info"}),
	)
	.await;
	read_until(&mut ws, |v| v["type"] == "status" && v["data"].is_object()).await;
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"the lazy load starts the session's monitor"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn completed_gate_turn_final_cost_matches_info_on_user_and_inbox_paths() {
	for inbox in [false, true] {
		let _data = TestDataDirGuard::new();
		let _env = StubEnv::new(vec![
			final_response(r#"{"scope":"self_contained","forbids_verification":false,"verification_policy_update":"unchanged","verification_policy_evidence":"","answer_only":false,"conditions":[],"operational_constraints":[]}"#),
			final_response("WS-GATE-TIMING: finished"),
			final_response(r#"<shape name="circular" found="no">independent evidence</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>
<verdict>PASS</verdict>"#),
		]).await;
		let mut config = ws_fake_config();
		config.supervisor.enabled = true;
		config.supervisor.model.model = Some("ollama:fake-model".into());
		config.supervisor.gate.enabled = true;
		config.supervisor.plan.enabled = false;
		config.decision.model = None;
		let server = LoopbackServer::start(Arc::new(config)).await;
		let mut ws = connect_ws(server.addr).await;
		let _welcome = read_json(&mut ws).await;
		let session_id = create_session(&mut ws, None).await;
		if inbox {
			push_inbox_message_for_session(
				&session_id,
				InboxMessage {
					source: InboxSource::Inject,
					content: "Explain what this session does.".into(),
				},
			);
		} else {
			send_json(
				&mut ws,
				serde_json::json!({"type":"message", "session_id":session_id, "content":"Explain what this session does."}),
			)
			.await;
		}
		let cost = read_until(&mut ws, |frame| frame["type"] == "cost" || frame["type"] == "error").await;
		assert_eq!(cost["type"], "cost", "{cost}");
		assert!(
			cost["supervisor_accounting"]["mechanics"]["gate"]["chat_calls"]
				.as_u64()
				.unwrap() > 0,
			"{cost}"
		);
		// The session lock becomes available only after the outer executor and save finish.
		let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
		drop(lock.lock().await);
		send_json(
			&mut ws,
			serde_json::json!({"type":"command", "session_id":session_id, "command":"info"}),
		)
		.await;
		let info = loop {
			let frame = read_json(&mut ws).await;
			assert_ne!(frame["type"], "cost", "only one completion frame per turn: {frame}");
			assert_ne!(frame["type"], "error", "{frame}");
			if frame["type"] == "status" && frame["data"].is_object() {
				break frame["data"].clone();
			}
		};
		assert_eq!(
			cost["supervisor_accounting"], info["supervisor_accounting"],
			"inbox={inbox}"
		);
		assert_eq!(cost["session_cost"], info["total_cost"]);
		ws.close(None).await.unwrap();
		crate::session::context::cleanup_session(&session_id);
	}
}
// ---- spending stop: the front-end must distinguish a budget stop from a finished turn ----

/// A tool-call turn whose result cost crosses `max_request_spending_threshold`
/// must end with a "Stopped: request spending threshold reached" status BEFORE
/// the cost frame — a driver reading only the cost frame would treat the stop
/// as an ordinary finished turn.
#[tokio::test]
#[serial_test::serial]
async fn user_turn_request_spending_stop_sends_the_stopped_status_before_cost() {
	let _data = TestDataDirGuard::new();
	// The tool-call response carries usage cost 0.002 — over the 0.0001
	// threshold, so the post-tool-result spending check stops the request
	// before any follow-up call is made.
	let _env = StubEnv::new(vec![crate::session::chat::test_support::tool_call_response(
		"schedule",
		serde_json::json!({"command": "list"}),
	)])
	.await;
	let mut config = ws_fake_config();
	config.max_request_spending_threshold = 0.0001;
	let server = LoopbackServer::start(Arc::new(config)).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "message", "session_id": session_id, "content": "list my schedules"
		}),
	)
	.await;
	let stopped = read_until(&mut ws, |v| {
		v["type"] == "status" && v["message"].as_str().unwrap_or_default().contains("Stopped:")
	})
	.await;
	assert!(
		stopped["message"]
			.as_str()
			.unwrap_or_default()
			.contains("request spending threshold"),
		"got: {stopped}"
	);
	assert_eq!(stopped["session_id"], session_id);
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(cost["session_id"], session_id);

	crate::session::context::cleanup_session(&session_id);
}

/// The inbox monitor drives its own turns; a budget stop there must produce the
/// same "Stopped" status over the background channel, not just a cost frame.
#[tokio::test]
#[serial_test::serial]
async fn monitor_inbox_turn_request_spending_stop_sends_the_stopped_status() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![crate::session::chat::test_support::tool_call_response(
		"schedule",
		serde_json::json!({"command": "list"}),
	)])
	.await;
	let mut config = ws_fake_config();
	config.max_request_spending_threshold = 0.0001;
	let server = LoopbackServer::start(Arc::new(config)).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "list my schedules".into(),
		},
	);
	let _injected = read_until(&mut ws, |v| v["type"] == "injected").await;
	let stopped = read_until(&mut ws, |v| {
		v["type"] == "status" && v["message"].as_str().unwrap_or_default().contains("Stopped:")
	})
	.await;
	assert!(
		stopped["message"]
			.as_str()
			.unwrap_or_default()
			.contains("request spending threshold"),
		"got: {stopped}"
	);
	let cost = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(cost["session_id"], session_id);

	crate::session::context::cleanup_session(&session_id);
}

/// `cleanup_session` destroys the inbox while the session's monitor is
/// parked on it. The monitor must notice on its next wakeup and exit — a
/// monitor that parks forever on a destroyed Notify is one leaked task per
/// unloaded session for the life of the process.
#[tokio::test]
#[serial_test::serial]
async fn monitor_exits_when_cleanup_destroyed_the_inbox() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let before = wait_for_inbox_monitors(0).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	assert_eq!(live_inbox_monitors() - before, 1);
	tokio::time::sleep(Duration::from_millis(200)).await;

	// Capture the schedule Notify the parked monitor waits on, then destroy
	// the session-scoped state. notify_one() stores a permit, so the wakeup
	// lands whether the monitor has parked yet or not.
	let notify = crate::session::context::get_schedule_notify(&session_id);
	crate::session::context::cleanup_session(&session_id);
	notify.notify_one();

	assert_eq!(
		wait_for_inbox_monitors(before).await,
		before,
		"the monitor must exit once the inbox is gone, not park forever"
	);
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"cleanup of session-scoped state must not drop the live session"
	);

	ws.close(None).await.ok();
}

/// A timed schedule that falls due while no connection is open still fires: the
/// monitor belongs to the session, so closing the socket that bound it neither
/// stops it nor leaves the entry unanswered.
#[tokio::test]
#[serial_test::serial]
async fn a_due_timed_schedule_fires_after_the_connection_closed() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("TIMER-TURN")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let before = wait_for_inbox_monitors(0).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	ws.close(None).await.ok();
	drop(ws);
	tokio::time::sleep(Duration::from_millis(300)).await;
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"the monitor outlives the connection that bound the session"
	);

	crate::session::context::get_schedule_storage(&session_id)
		.lock()
		.unwrap()
		.add(crate::mcp::orchestration::schedule::storage::ScheduleEntry::new(
			"tick".to_string(),
			crate::mcp::orchestration::schedule::storage::ScheduleAction::Message {
				message: "timer says go".to_string(),
			},
			chrono::Local::now() + chrono::Duration::milliseconds(300),
			None,
		));
	crate::session::context::notify_schedule_change(&session_id);

	let fired = tokio::time::timeout(Duration::from_secs(20), async {
		loop {
			let last = server
				.sessions
				.lock()
				.await
				.get(&session_id)
				.and_then(|s| s.session.messages.last().cloned());
			if matches!(&last, Some(m) if m.role == "assistant" && m.content.contains("TIMER-TURN")) {
				break;
			}
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(
		fired.is_ok(),
		"the due schedule must run its turn with nobody connected"
	);

	crate::session::context::cleanup_session(&session_id);
}

// ---- server construction ----

#[test]
fn server_new_parses_the_address_and_stores_role_and_origins() {
	let server = WebSocketServer::new(
		"127.0.0.1",
		0,
		ws_fake_config(),
		"assistant".to_string(),
		vec!["https://octomind.dev".to_string()],
	)
	.expect("a valid host:port must parse");
	assert_eq!(server.addr.port(), 0);
	assert_eq!(server.state.role, "assistant");
	assert_eq!(server.state.allow_origins.len(), 1);

	let bad = WebSocketServer::new("not a host", 1, ws_fake_config(), "assistant".to_string(), Vec::new());
	assert!(bad.is_err(), "an unparseable address must be rejected");
}

// ---- batch-5: residual arms ----

/// Remove the sessions directory so every append to a session file fails
/// with ENOENT. Portable: works for any user (no chmod) on every OS, and
/// nothing recreates the directory on the paths under test.
fn break_session_persistence() {
	let sessions_dir = crate::session::get_sessions_dir().expect("sessions dir");
	std::fs::remove_dir_all(sessions_dir).expect("remove sessions dir");
}

/// Write one session-log entry as an independent zstd frame — the on-disk
/// format is one complete frame per line.
fn write_session_log_entry(path: &std::path::Path, line: &str) {
	let file = std::fs::File::create(path).expect("create session log");
	let mut encoder = zstd::stream::Encoder::new(file, 3).expect("zstd encoder");
	std::io::Write::write_all(&mut encoder, line.as_bytes()).expect("write log line");
	encoder.finish().expect("finish zstd frame");
}

/// An OLLAMA_API_URL stub that delays its response, so a test can intervene
/// while a turn is in flight (kill the connection, break persistence, …).
/// `arrived()` fires once the request body has been fully read — the signal
/// that the turn has started and it is safe to sabotage the environment.
struct SlowStubEnv {
	arrived: Arc<tokio::sync::Notify>,
	_guard: tokio::sync::MutexGuard<'static, ()>,
}

impl SlowStubEnv {
	async fn new(delay: Duration) -> Self {
		use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

		let guard = ENV_LOCK.lock().await;
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind slow stub listener");
		let addr = listener.local_addr().expect("slow stub addr");
		let arrived = Arc::new(tokio::sync::Notify::new());

		let arrived_bg = arrived.clone();
		tokio::spawn(async move {
			while let Ok((mut sock, _)) = listener.accept().await {
				let arrived = arrived_bg.clone();
				tokio::spawn(async move {
					// Same request framing as the scripted stub in test_support.
					let mut buf = Vec::new();
					let mut tmp = [0u8; 8192];
					let header_end = loop {
						let n = sock.read(&mut tmp).await.unwrap_or(0);
						if n == 0 {
							return;
						}
						buf.extend_from_slice(&tmp[..n]);
						if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
							break pos + 4;
						}
					};
					let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
					let content_length: usize = headers
						.lines()
						.find_map(|l| l.strip_prefix("content-length:"))
						.and_then(|v| v.trim().parse().ok())
						.unwrap_or(0);
					while buf.len() < header_end + content_length {
						let n = sock.read(&mut tmp).await.unwrap_or(0);
						if n == 0 {
							break;
						}
						buf.extend_from_slice(&tmp[..n]);
					}

					arrived.notify_one();
					tokio::time::sleep(delay).await;

					let body = serde_json::json!({
						"choices": [{
							"message": {"role": "assistant", "content": "SLOW-TURN"},
							"finish_reason": "stop"
						}],
						"usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
					})
					.to_string();
					let response = format!(
						"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
						body.len(),
						body
					);
					let _ = sock.write_all(response.as_bytes()).await;
					let _ = sock.shutdown().await;
				});
			}
		});

		std::env::set_var("OLLAMA_API_URL", format!("http://{addr}/v1/chat/completions"));
		Self { arrived, _guard: guard }
	}

	fn arrived(&self) -> &tokio::sync::Notify {
		&self.arrived
	}
}

impl Drop for SlowStubEnv {
	fn drop(&mut self) {
		std::env::remove_var("OLLAMA_API_URL");
	}
}

#[tokio::test]
#[serial_test::serial]
async fn user_message_on_a_locked_session_is_reported_as_busy() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	// Hold the per-session lock as a concurrent client turn would.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let _guard = lock.lock().await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id.as_str(), "content": "hi", "request_id": "m1"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert_eq!(error["request_id"], "m1", "busy error must echo the request: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("is busy processing another request"),
		"got: {error}"
	);

	drop(_guard);
	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn unknown_command_is_refused_instead_of_treated_as_user_input() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": session_id.as_str(), "command": "frobnicate"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Unknown command: '/frobnicate'"),
		"got: {error}"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn session_resume_with_an_unresolvable_tap_role_reports_the_restore_failure() {
	let _data = TestDataDirGuard::new();
	// An unparseable taps.toml makes every tap lookup fail offline — no
	// network fetch, no default-tap clone.
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	std::fs::write(data_dir.join("taps.toml"), "taps = [").expect("write invalid taps.toml");

	// Forge a disk session whose recorded role is a tap tag no tap provides.
	let sessions_dir = crate::session::get_sessions_dir().expect("sessions dir");
	write_session_log_entry(
		&sessions_dir.join("forged.jsonl.zst"),
		r#"{"type":"SUMMARY","session_info":{"role":"ghost:agent","name":"forged"}}"#,
	);

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(&mut ws, serde_json::json!({"type": "session", "session_id": "forged"})).await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to restore role 'ghost:agent'"),
		"got: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn failing_command_on_a_memory_session_reports_the_save_failure() {
	let _data = TestDataDirGuard::new();
	// /workflow (no args) enumerates taps; an unparseable taps.toml makes the
	// command itself fail — offline and deterministic.
	let data_dir = crate::directories::get_octomind_data_dir().expect("data dir");
	std::fs::write(data_dir.join("taps.toml"), "taps = [").expect("write invalid taps.toml");

	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	// Memory-only session: saving it must fail after the command errored.
	server
		.sessions
		.lock()
		.await
		.insert("mem-only".to_string(), ChatSession::for_tests(Vec::new()));
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": "mem-only", "command": "workflow"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to save session"),
		"the command error must not mask the save failure: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn exit_command_on_a_memory_session_reports_the_save_failure() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	server
		.sessions
		.lock()
		.await
		.insert("mem-exit".to_string(), ChatSession::for_tests(Vec::new()));
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": "mem-exit", "command": "exit"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to save session"),
		"an ended session that cannot be saved must say so: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn done_command_on_a_persisted_session_reports_the_compression_failure() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::with_status(vec![
		(200, final_response("first reply")),
		(200, final_response("second reply")),
		(500, serde_json::json!({"error": {"message": "boom"}})),
	])
	.await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	// Two exchanges give /done a compressible range.
	for content in ["one", "two"] {
		send_json(
			&mut ws,
			serde_json::json!({"type": "message", "session_id": session_id.as_str(), "content": content}),
		)
		.await;
		let _ack = read_json(&mut ws).await;
		read_until(&mut ws, |frame| frame["type"] == "cost").await;
	}

	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": session_id.as_str(), "command": "done"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Compression failed"),
		"a persisted session saves fine, so the compression error itself is reported: {error}"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn pre_user_drain_reports_inbox_and_user_persistence_failures() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	break_session_persistence();

	// Queue an inbox message behind the session lock so the background monitor
	// parks and the foreground pre-user drain owns it.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "unpersistable injection".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	drop(guard);

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id.as_str(), "content": "hello"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let injected = read_until(&mut ws, |frame| frame["type"] == "injected").await;
	assert_eq!(injected["content"], "unpersistable injection", "got: {injected}");

	let inbox_error = read_until(&mut ws, |frame| {
		frame["type"] == "error"
			&& frame["message"]
				.as_str()
				.unwrap_or_default()
				.contains("Error processing injected message")
	})
	.await;
	assert!(
		inbox_error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Error processing injected message"),
		"got: {inbox_error}"
	);

	let user_error = read_until(&mut ws, |frame| {
		frame["type"] == "error" && frame["message"].as_str().unwrap_or_default().starts_with("Error:")
	})
	.await;
	assert!(
		user_error["message"].as_str().unwrap_or_default().starts_with("Error:"),
		"the user turn itself must also report the persistence failure: {user_error}"
	);

	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the session is returned to the map on the error path"
	);
	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn monitor_swallows_an_inbox_batch_it_cannot_persist_and_keeps_the_session() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	// Let the monitor reach its parked wait state.
	tokio::time::sleep(Duration::from_millis(200)).await;

	break_session_persistence();

	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "unpersistable monitor work".to_string(),
		},
	);

	let injected = read_until(&mut ws, |frame| frame["type"] == "injected").await;
	assert_eq!(injected["content"], "unpersistable monitor work", "got: {injected}");

	// The batch cannot be persisted, so no AI turn may run.
	let stray = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
	assert!(stray.is_err(), "no frame may follow the failed batch, got: {stray:?}");

	assert!(
		!inbox_has_messages(&session_id).await,
		"the batch was consumed even though persisting it failed"
	);
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the session must be returned to the map"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn monitor_finishing_a_turn_after_the_connection_closes_keeps_the_session_and_its_monitor() {
	let _data = TestDataDirGuard::new();
	let _env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let before = wait_for_inbox_monitors(0).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;
	tokio::time::sleep(Duration::from_millis(200)).await;

	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "slow background work".to_string(),
		},
	);
	let injected = read_until(&mut ws, |frame| frame["type"] == "injected").await;
	assert_eq!(injected["content"], "slow background work", "got: {injected}");

	// The turn is now blocked on the slow stub. Kill the connection and make
	// the post-turn save fail while it runs.
	_env.arrived().notified().await;
	ws.close(None).await.ok();
	drop(ws);
	break_session_persistence();

	// The turn ends with nobody connected: the session goes back to the map and
	// the monitor stays for the session's next event.
	let stored_back = tokio::time::timeout(Duration::from_secs(20), async {
		while !server.sessions.lock().await.contains_key(&session_id) {
			tokio::time::sleep(Duration::from_millis(50)).await;
		}
	})
	.await;
	assert!(stored_back.is_ok(), "the turn must finish and store the session back");
	assert_eq!(
		live_inbox_monitors() - before,
		1,
		"a closed connection does not end the session's monitor"
	);
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"the finished session must be returned to the map"
	);
	assert!(
		!inbox_has_messages(&session_id).await,
		"the turn consumed its inbox message"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn user_turn_save_failure_is_reported_before_the_cost_frame() {
	let _data = TestDataDirGuard::new();
	let _env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id.as_str(), "content": "hello"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;

	// The user message is persisted at turn start; break persistence while the
	// API call is in flight so only the post-turn save fails.
	_env.arrived().notified().await;
	break_session_persistence();

	let assistant = read_until(&mut ws, |frame| frame["type"] == "assistant").await;
	assert!(
		assistant["content"].as_str().unwrap_or_default().contains("SLOW-TURN"),
		"got: {assistant}"
	);

	let save_error = read_until(&mut ws, |frame| {
		frame["type"] == "error"
			&& frame["message"]
				.as_str()
				.unwrap_or_default()
				.contains("Failed to save session")
	})
	.await;
	assert!(
		save_error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to save session"),
		"got: {save_error}"
	);
	let cost = read_until(&mut ws, |frame| frame["type"] == "cost").await;
	assert_eq!(
		cost["type"], "cost",
		"the cost frame closes the turn even after a save failure"
	);

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
#[serial_test::serial]
async fn start_binds_and_accepts_connections() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![]).await;

	// Reserve then release a port so the real server can bind it.
	let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind probe");
	let port_a = probe.local_addr().expect("probe addr").port();
	drop(probe);

	let server_a = WebSocketServer::new(
		"127.0.0.1",
		port_a,
		ws_fake_config(),
		"assistant".to_string(),
		Vec::new(),
	)
	.expect("server without origins");
	let addr_a = server_a.addr;
	tokio::spawn(async move {
		let _ = server_a.start().await;
	});

	// Wait for the listener, then a bare TCP probe: the accept loop spawns a
	// handler whose handshake fails on the immediately-closed socket — the
	// handler-failure log arm — without disturbing the server.
	for _ in 0..100 {
		if tokio::net::TcpStream::connect(addr_a).await.is_ok() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}

	let mut ws_a = connect_ws(addr_a).await;
	let welcome_a = read_json(&mut ws_a).await;
	assert_eq!(welcome_a["type"], "status", "got: {welcome_a}");
	assert!(
		welcome_a["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Connected to Octomind WebSocket server (role: assistant)"),
		"got: {welcome_a}"
	);

	// Same for a server configured with an allowlisted origin: the print
	// branch differs, and a native client (no Origin header) is still let in.
	let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind probe");
	let port_b = probe.local_addr().expect("probe addr").port();
	drop(probe);
	let server_b = WebSocketServer::new(
		"127.0.0.1",
		port_b,
		ws_fake_config(),
		"assistant".to_string(),
		vec!["http://localhost:3000".to_string()],
	)
	.expect("server with origins");
	let addr_b = server_b.addr;
	tokio::spawn(async move {
		let _ = server_b.start().await;
	});
	for _ in 0..100 {
		if tokio::net::TcpStream::connect(addr_b).await.is_ok() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}

	let mut ws_b = connect_ws(addr_b).await;
	let welcome_b = read_json(&mut ws_b).await;
	assert_eq!(welcome_b["type"], "status", "got: {welcome_b}");
}

#[tokio::test]
#[serial_test::serial]
async fn pre_user_drain_reports_a_failing_injected_turn_and_still_runs_the_user_turn() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::with_status(vec![
		(500, serde_json::json!({"error": {"message": "boom"}})),
		(200, final_response("user reply")),
	])
	.await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, None).await;

	// Queue an inbox message behind the session lock so the background monitor
	// parks and the foreground pre-user drain owns it.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "doomed injection".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	drop(guard);

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id.as_str(), "content": "hello"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let injected = read_until(&mut ws, |frame| frame["type"] == "injected").await;
	assert_eq!(injected["content"], "doomed injection", "got: {injected}");

	// The injected turn hits the 500 stub and must be reported as failed…
	let injected_error = read_until(&mut ws, |frame| {
		frame["type"] == "error"
			&& frame["message"]
				.as_str()
				.unwrap_or_default()
				.contains("Error processing injected message")
	})
	.await;
	assert!(
		injected_error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Error processing injected message"),
		"got: {injected_error}"
	);

	// …but the client's own turn still runs to completion on the next response.
	let assistant = read_until(&mut ws, |frame| frame["type"] == "assistant").await;
	assert!(
		assistant["content"].as_str().unwrap_or_default().contains("user reply"),
		"got: {assistant}"
	);
	let cost = read_until(&mut ws, |frame| frame["type"] == "cost").await;
	assert_eq!(cost["type"], "cost");

	crate::session::context::cleanup_session(&session_id);
}

// ---- cancel / close / role-on-bind ----

/// A provider that accepts the request and never answers, so a turn stays in
/// flight until it is cancelled.
async fn spawn_hanging_stub() -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind hanging stub");
	let addr = listener.local_addr().expect("addr");
	tokio::spawn(async move {
		while let Ok((sock, _)) = listener.accept().await {
			tokio::spawn(async move {
				tokio::time::sleep(Duration::from_secs(120)).await;
				drop(sock);
			});
		}
	});
	format!("http://{}/v1/chat/completions", addr)
}

/// Reads every frame up to and including the first `cost`.
async fn frames_through_cost(ws: &mut ClientWs) -> Vec<serde_json::Value> {
	let mut frames = Vec::new();
	for _ in 0..100 {
		let value = read_json(ws).await;
		let done = value["type"] == "cost";
		frames.push(value);
		if done {
			return frames;
		}
	}
	panic!("no cost frame within 100 frames: {frames:?}");
}

#[tokio::test]
#[serial_test::serial]
async fn cancel_stops_the_running_turn_and_later_frames_still_run_in_order() {
	let _data = TestDataDirGuard::new();
	let env = StubEnv {
		_guard: ENV_LOCK.lock().await,
	};
	std::env::set_var("OLLAMA_API_URL", spawn_hanging_stub().await);
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, Some("cancel-1")).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "hang", "request_id": "m1"}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["request_id"], "m1", "got: {ack}");
	// Let the turn reach the in-flight provider call.
	tokio::time::sleep(Duration::from_millis(300)).await;

	// Sent while the turn runs: the command waits, the cancel acts at once.
	send_json(
		&mut ws,
		serde_json::json!({"type": "command", "session_id": session_id, "command": "info", "request_id": "c1"}),
	)
	.await;
	send_json(
		&mut ws,
		serde_json::json!({"type": "cancel", "session_id": session_id, "request_id": "x1"}),
	)
	.await;

	let frames = tokio::time::timeout(Duration::from_secs(20), frames_through_cost(&mut ws))
		.await
		.expect("a cancelled turn must close promptly");
	let cancel_ack = frames
		.iter()
		.position(|f| f["type"] == "ack" && f["request_id"] == "x1")
		.unwrap_or_else(|| panic!("the cancel is acknowledged during the turn: {frames:?}"));
	assert_eq!(frames[cancel_ack]["message_type"], "cancel");
	assert!(
		!frames.iter().any(|f| f["type"] == "error"),
		"a requested stop is not an error: {frames:?}"
	);
	let cost = frames.last().unwrap();
	assert_eq!(cost["cancelled"], true, "got: {cost}");
	assert!(
		!frames.iter().any(|f| f["request_id"] == "c1"),
		"the queued command must not jump ahead of the running turn: {frames:?}"
	);

	// The deferred command runs right after, acknowledged in order.
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["request_id"], "c1", "got: {ack}");
	let status = read_until(&mut ws, |v| v["type"] == "status").await;
	assert!(status["data"].is_object(), "got: {status}");
	assert!(
		server.sessions.lock().await.contains_key(&session_id),
		"a cancelled turn stores its session back"
	);

	crate::session::context::cleanup_session(&session_id);
	drop(env);
}

/// Hangs on the first provider request, answers every later one at once.
async fn spawn_hang_then_answer_stub(answer: &'static str) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
	let addr = listener.local_addr().expect("addr");
	tokio::spawn(async move {
		let mut first = true;
		while let Ok((mut sock, _)) = listener.accept().await {
			let hang = std::mem::replace(&mut first, false);
			tokio::spawn(async move {
				use tokio::io::{AsyncReadExt, AsyncWriteExt};
				let mut buf = vec![0u8; 1 << 20];
				let mut total = 0;
				loop {
					let n = sock.read(&mut buf[total..]).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					total += n;
					let text = String::from_utf8_lossy(&buf[..total]).to_lowercase();
					let Some(head) = text.find("\r\n\r\n") else {
						continue;
					};
					let len: usize = text
						.split("content-length:")
						.nth(1)
						.and_then(|s| s.split(['\r', '\n']).next())
						.and_then(|s| s.trim().parse().ok())
						.unwrap_or(0);
					if total >= head + 4 + len {
						break;
					}
				}
				if hang {
					tokio::time::sleep(Duration::from_secs(120)).await;
					return;
				}
				let body = final_response(answer).to_string();
				let response = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
			});
		}
	});
	format!("http://{}/v1/chat/completions", addr)
}

/// A client that drops mid-turn leaves the turn running (R10). The next bind
/// that resumes the stream gets every frame the client missed exactly once,
/// then the rest of the turn live — down to the cost that answers the original
/// request.
#[tokio::test]
#[serial_test::serial]
async fn a_turn_outlives_its_dropped_connection_and_a_resuming_bind_reads_the_rest() {
	let _data = TestDataDirGuard::new();
	let _env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let session_id = "resume-1";

	let mut first = connect_ws(server.addr).await;
	let _welcome = read_json(&mut first).await;
	send_json(
		&mut first,
		serde_json::json!({"type": "session", "session_id": session_id}),
	)
	.await;
	let ack = read_json(&mut first).await;
	let stream = ack["stream"]
		.as_str()
		.expect("the bind ack names the stream")
		.to_string();
	let _greeting = read_json(&mut first).await;

	// A queued result makes the message run an injected turn first, so the
	// turn streams frames both before and after the connection drops.
	let lock = get_or_create_session_lock(session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	push_inbox_message_for_session(
		session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "queued work".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	drop(guard);

	send_json(
		&mut first,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "hello", "request_id": "m1"}),
	)
	.await;
	let message_ack = read_json(&mut first).await;
	assert_eq!(message_ack["type"], "ack");
	assert!(message_ack.get("stream_seq").is_none(), "acks are not session output");
	let injected = read_json(&mut first).await;
	assert_eq!(injected["type"], "injected", "got: {injected}");
	assert_eq!(injected["stream_seq"], 1);
	let answer = read_until(&mut first, |v| v["type"] == "assistant").await;
	let last_read = answer["stream_seq"].as_u64().expect("session output is numbered");
	// Gone without a goodbye, while the message's own turn waits on the model.
	drop(first);

	let mut second = connect_ws(server.addr).await;
	let _welcome = read_json(&mut second).await;
	send_json(
		&mut second,
		serde_json::json!({
			"type": "session", "session_id": session_id,
			"resume": {"stream": stream, "after": last_read}
		}),
	)
	.await;
	let ack = read_json(&mut second).await;
	assert_eq!(ack["type"], "ack", "got: {ack}");
	assert_eq!(ack["stream"], stream.as_str());
	assert_eq!(ack["resumed"], true, "got: {ack}");
	let bound_at = ack["stream_seq"].as_u64().expect("the ack reports the stream position");

	let frames = tokio::time::timeout(Duration::from_secs(20), async {
		let mut frames: Vec<serde_json::Value> = Vec::new();
		loop {
			let frame = read_json(&mut second).await;
			let done = frame["type"] == "cost" && frame["request_id"] == "m1";
			frames.push(frame);
			if done {
				return frames;
			}
		}
	})
	.await
	.expect("the turn must finish on the resumed connection");

	let seqs: Vec<u64> = frames.iter().filter_map(|f| f["stream_seq"].as_u64()).collect();
	let expected: Vec<u64> = (last_read + 1..=last_read + seqs.len() as u64).collect();
	assert_eq!(
		seqs, expected,
		"every missed frame once, in order, then the rest: {frames:?}"
	);
	assert!(
		seqs.iter().any(|seq| *seq > bound_at),
		"the turn was still running and streamed live after the bind: {frames:?}"
	);
	let cost = frames.last().unwrap();
	assert!(
		cost.get("cancelled").is_none(),
		"a dropped client no longer stops its turn: {cost}"
	);
	assert!(
		frames
			.iter()
			.any(|f| f["type"] == "assistant" && f["stream_seq"].as_u64() > Some(bound_at)),
		"the message's own answer arrived live: {frames:?}"
	);

	crate::session::context::cleanup_session(&session_id.to_string());
}

/// Starts a slow turn for `session_id` on a fresh connection and returns that
/// connection once the model has the request.
async fn connection_running_a_slow_turn(
	server: &LoopbackServer,
	env: &SlowStubEnv,
	session_id: &str,
	request_id: &str,
) -> ClientWs {
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	create_session(&mut ws, Some(session_id)).await;
	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "hello", "request_id": request_id}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	env.arrived().notified().await;
	ws
}

/// A client re-binding without `resume` while another live connection reads a
/// running turn only binds: the turn keeps streaming to its owner.
#[tokio::test]
#[serial_test::serial]
async fn a_bind_without_resume_does_not_take_a_running_turn_from_its_live_owner() {
	let _data = TestDataDirGuard::new();
	let env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut owner = connection_running_a_slow_turn(&server, &env, "owned-1", "m1").await;

	let mut other = connect_ws(server.addr).await;
	let _welcome = read_json(&mut other).await;
	send_json(
		&mut other,
		serde_json::json!({"type": "session", "session_id": "owned-1"}),
	)
	.await;
	let ack = read_json(&mut other).await;
	assert_eq!(ack["type"], "ack");
	assert!(ack["stream"].is_string(), "got: {ack}");
	assert!(ack.get("resumed").is_none(), "got: {ack}");
	let greeting = read_json(&mut other).await;
	assert_eq!(greeting["type"], "status");

	let cost = read_until(&mut owner, |v| v["type"] == "cost").await;
	assert_eq!(cost["request_id"], "m1", "the owner reads its turn to the end: {cost}");
	let stray = tokio::time::timeout(Duration::from_millis(500), other.next()).await;
	assert!(stray.is_err(), "the plain bind received the owner's turn: {stray:?}");

	crate::session::context::cleanup_session(&"owned-1".to_string());
}

/// When the connection reading a running turn is gone, a plain bind (no
/// `resume`) takes the rest of that turn live — without a replay; the ack's
/// `stream_seq` tells it what it did not see.
#[tokio::test]
#[serial_test::serial]
async fn a_bind_without_resume_adopts_a_running_turn_nobody_owns() {
	let _data = TestDataDirGuard::new();
	let env = SlowStubEnv::new(Duration::from_secs(2)).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let gone = connection_running_a_slow_turn(&server, &env, "ownerless-1", "m1").await;
	drop(gone);
	// Let the server see the connection go.
	tokio::time::sleep(Duration::from_millis(300)).await;

	let mut adopter = connect_ws(server.addr).await;
	let _welcome = read_json(&mut adopter).await;
	send_json(
		&mut adopter,
		serde_json::json!({"type": "session", "session_id": "ownerless-1"}),
	)
	.await;
	let ack = read_json(&mut adopter).await;
	assert!(ack.get("resumed").is_none(), "got: {ack}");
	let bound_at = ack["stream_seq"].as_u64().expect("the ack reports the stream position");
	let greeting = read_json(&mut adopter).await;
	assert_eq!(greeting["type"], "status");

	let rest = frames_through_cost(&mut adopter).await;
	let cost = rest.last().unwrap();
	assert_eq!(
		cost["request_id"], "m1",
		"the dropped client's turn finished here: {cost}"
	);
	assert!(cost.get("cancelled").is_none(), "got: {cost}");
	assert!(
		rest.iter()
			.all(|f| f["stream_seq"].as_u64().is_some_and(|seq| seq > bound_at)),
		"only live frames, nothing replayed: {rest:?}"
	);

	crate::session::context::cleanup_session(&"ownerless-1".to_string());
}

/// Output is numbered without gaps from the first frame a loaded session sends,
/// and the bind ack says where that numbering stands. A session that is closed
/// and loaded again starts a new stream, so resuming the old one is refused and
/// nothing is replayed.
#[tokio::test]
#[serial_test::serial]
async fn session_output_is_numbered_per_stream_and_a_reloaded_session_refuses_the_old_one() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("NUMBERED")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": "numbered-1", "request_id": "b1"}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	assert_eq!(ack["request_id"], "b1");
	assert_eq!(ack["stream_seq"], 0, "a fresh stream: {ack}");
	assert!(ack.get("resumed").is_none(), "no resume was asked: {ack}");
	assert!(
		ack["capabilities"]
			.as_array()
			.is_some_and(|caps| caps.iter().any(|cap| cap == "stream_resume_v1")),
		"got: {ack}"
	);
	let stream = ack["stream"].as_str().expect("stream id").to_string();
	let greeting = read_json(&mut ws).await;
	assert_eq!(greeting["type"], "status");
	assert!(
		greeting.get("stream_seq").is_none(),
		"the greeting answers the bind: {greeting}"
	);

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": "numbered-1", "content": "hi", "request_id": "m1"}),
	)
	.await;
	let message_ack = read_json(&mut ws).await;
	assert!(message_ack.get("stream_seq").is_none(), "got: {message_ack}");
	let turn = frames_through_cost(&mut ws).await;
	let seqs: Vec<u64> = turn
		.iter()
		.map(|f| f["stream_seq"].as_u64().expect("every turn frame is numbered"))
		.collect();
	let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
	assert_eq!(seqs, expected, "gapless from 1: {turn:?}");

	// Re-binding keeps the stream while the session stays loaded.
	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": "numbered-1"}),
	)
	.await;
	let rebound = read_json(&mut ws).await;
	assert_eq!(rebound["stream"], stream.as_str());
	assert_eq!(rebound["stream_seq"], seqs.len() as u64);
	let _greeting = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "close", "session_id": "numbered-1"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let closed = read_json(&mut ws).await;
	assert_eq!(closed["data"]["command_type"], "close", "got: {closed}");

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "session", "session_id": "numbered-1",
			"resume": {"stream": stream, "after": 1}
		}),
	)
	.await;
	let reloaded = read_json(&mut ws).await;
	assert_eq!(reloaded["resumed"], false, "got: {reloaded}");
	assert_ne!(reloaded["stream"], stream.as_str(), "a reload mints a new stream");
	assert_eq!(reloaded["stream_seq"], 0);
	let next = read_json(&mut ws).await;
	assert_eq!(next["type"], "status", "nothing is replayed: {next}");

	crate::session::context::cleanup_session(&"numbered-1".to_string());
}

#[tokio::test]
#[serial_test::serial]
async fn cancel_during_the_pre_user_drain_skips_the_user_turn_too() {
	let _data = TestDataDirGuard::new();
	let env = StubEnv {
		_guard: ENV_LOCK.lock().await,
	};
	std::env::set_var("OLLAMA_API_URL", spawn_hang_then_answer_stub("NEVER").await);
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, Some("drain-cancel-1")).await;

	// Queue an inbox message behind the session lock so the monitor parks and
	// the message's pre-user drain owns it.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "slow injected work".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	drop(guard);

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "USER-PROMPT-XYZ"}),
	)
	.await;
	read_until(&mut ws, |v| v["type"] == "injected").await;
	tokio::time::sleep(Duration::from_millis(300)).await;
	send_json(&mut ws, serde_json::json!({"type": "cancel", "session_id": session_id})).await;

	// The frame closes with a cancelled cost; its user turn never runs.
	let frames = tokio::time::timeout(Duration::from_secs(20), async {
		let mut frames = Vec::new();
		loop {
			let v = read_json(&mut ws).await;
			let done = v["type"] == "cost" && v["cancelled"] == true;
			frames.push(v);
			if done {
				return frames;
			}
		}
	})
	.await
	.expect("the stopped frame must close promptly");
	assert!(
		!frames
			.iter()
			.any(|f| f["type"] == "assistant" && f["content"].as_str().unwrap_or("").contains("NEVER")),
		"the user turn ran after the stop: {frames:?}"
	);
	assert!(
		!frames.iter().any(|f| f["type"] == "error"),
		"a requested stop is not an error: {frames:?}"
	);
	let sessions = server.sessions.lock().await;
	let stored = sessions.get(&session_id).expect("session stored back");
	assert!(
		!stored
			.session
			.messages
			.iter()
			.any(|m| m.content.contains("USER-PROMPT-XYZ")),
		"the cancelled frame's user message must not run"
	);
	drop(sessions);

	crate::session::context::cleanup_session(&session_id);
	drop(env);
}

#[tokio::test]
#[serial_test::serial]
async fn only_the_requests_own_cost_echoes_its_request_id() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("injected reply"), final_response("user reply")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, Some("rid-cost-1")).await;

	// One injected message waits, so the request first drains it.
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	let guard = lock.lock().await;
	push_inbox_message_for_session(
		&session_id,
		InboxMessage {
			source: InboxSource::Inject,
			content: "queued work".to_string(),
		},
	);
	tokio::time::sleep(Duration::from_millis(100)).await;
	drop(guard);

	send_json(
		&mut ws,
		serde_json::json!({"type": "message", "session_id": session_id, "content": "hi", "request_id": "m-77"}),
	)
	.await;
	let drained = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert!(
		drained.get("request_id").is_none(),
		"a drained turn's cost is not the request's: {drained}"
	);
	let own = read_until(&mut ws, |v| v["type"] == "cost").await;
	assert_eq!(own["request_id"], "m-77", "got: {own}");

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
async fn cancel_without_a_running_turn_is_only_acknowledged() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "cancel", "session_id": "idle-zzz", "request_id": "x1"}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack");
	assert_eq!(ack["message_type"], "cancel");

	// Nothing else follows: the next frame answered is the next request's.
	ws.send(Message::Ping(vec![7u8].into())).await.expect("ping");
	assert!(matches!(read_frame(&mut ws).await, Message::Pong(_)));
}

#[tokio::test]
async fn elicitation_response_for_an_unknown_id_is_an_error() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "elicitation_response", "session_id": "idle-elicit", "id": "nope",
			"request_id": "r1", "result": {"action": "decline"}
		}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["type"], "ack", "got: {ack}");
	assert_eq!(ack["message_type"], "elicitation_response");
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert_eq!(error["request_id"], "r1");
	assert_eq!(
		error["code"],
		crate::websocket::protocol::ELICITATION_GONE_ERROR,
		"clients match the code: relays may rewrite the message"
	);
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("no pending elicitation 'nope'"),
		"got: {error}"
	);
}

#[tokio::test]
async fn elicitation_response_is_handled_while_the_session_lock_is_held() {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let state = ServerState::new(Arc::new(template_config()), "assistant".into(), Vec::new());
	let session = "ws-elicit-held-turn";
	let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
	let (release_tx, release_rx) = tokio::sync::oneshot::channel();
	let handler = tokio::spawn(async move {
		let (stream, peer) = listener.accept().await.unwrap();
		let (sink, frames) = tokio_tungstenite::accept_async(stream).await.unwrap().split();
		let mut reader = WsReader::new(frames, peer, HEARTBEAT_INTERVAL);
		let (sender, writer) = WsSender::spawn(sink);
		let lock = get_or_create_session_lock(session, &state.session_locks).await;
		let processing = async {
			let _guard = lock.lock().await;
			entered_tx.send(()).unwrap();
			release_rx.await.unwrap();
			Ok(())
		};
		let ctx = ConnCtx {
			server: state,
			closed: CancellationToken::new(),
		};
		read_while_processing(
			processing,
			&mut reader,
			&sender,
			&ctx,
			&mut VecDeque::new(),
			&mut false,
			peer,
		)
		.await
		.unwrap();
		drop(sender);
		writer.await.unwrap();
	});
	let mut client = connect_ws(addr).await;
	entered_rx.await.unwrap();

	send_json(
		&mut client,
		serde_json::json!({
			"type": "elicitation_response", "session_id": session, "id": "nope",
			"request_id": "r1", "result": {"action": "cancel"}
		}),
	)
	.await;
	// Both frames arrive while the turn still holds the lock.
	assert_eq!(read_json(&mut client).await["type"], "ack");
	let error = read_json(&mut client).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert_eq!(error["request_id"], "r1");

	release_tx.send(()).unwrap();
	drop(client);
	handler.await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn close_unloads_the_session_and_a_later_bind_resumes_it_from_disk() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;
	let session_id = create_session(&mut ws, Some("close-1")).await;
	assert!(server.sessions.lock().await.contains_key(&session_id));

	send_json(
		&mut ws,
		serde_json::json!({"type": "close", "session_id": session_id, "request_id": "k1"}),
	)
	.await;
	let ack = read_json(&mut ws).await;
	assert_eq!(ack["message_type"], "close", "got: {ack}");
	let status = read_json(&mut ws).await;
	assert_eq!(status["type"], "status", "got: {status}");
	assert_eq!(status["data"]["command_type"], "close");
	assert_eq!(status["data"]["was_loaded"], true);
	assert!(
		!server.sessions.lock().await.contains_key(&session_id),
		"close takes the session out of memory"
	);

	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": session_id}),
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

	crate::session::context::cleanup_session(&session_id);
}

#[tokio::test]
async fn close_of_a_busy_session_is_refused() {
	let server = LoopbackServer::start(Arc::new(template_config())).await;
	let lock = get_or_create_session_lock("busy-close", &server.session_locks).await;
	let _held = lock.lock().await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "close", "session_id": "busy-close", "request_id": "k1"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error");
	assert_eq!(error["request_id"], "k1");
	assert!(
		error["message"].as_str().unwrap_or_default().contains("busy"),
		"got: {error}"
	);
}

#[tokio::test]
#[serial_test::serial]
async fn bind_with_a_role_creates_the_session_under_that_role_and_resume_keeps_it() {
	let _data = TestDataDirGuard::new();
	let script = ["mcp", "schedule"]
		.into_iter()
		.flat_map(|tool| {
			[
				crate::session::chat::test_support::tool_call_response(
					tool,
					if tool == "mcp" {
						serde_json::json!({"action": "list"})
					} else {
						serde_json::json!({"command": "list"})
					},
				),
				final_response("done"),
			]
		})
		.collect();
	let _env = StubEnv::new(script).await;
	let config = two_role_config();
	crate::mcp::initialize_mcp_for_role("assistant", &config)
		.await
		.expect("server MCP init");
	let server = LoopbackServer::start(Arc::new(config)).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": "role-on-bind", "role": "outbound"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let status = read_json(&mut ws).await;
	assert_eq!(status["type"], "status", "got: {status}");
	assert_eq!(server.sessions.lock().await["role-on-bind"].role, "outbound");

	// The bound role's tools dispatch; the server role's do not.
	let own = tool_turn(&mut ws, "role-on-bind", "mcp").await;
	assert_eq!(own["success"], true, "bound role lost its tool: {own}");
	assert_refused(&tool_turn(&mut ws, "role-on-bind", "schedule").await);

	// A later bind naming another role resumes the session as it is.
	send_json(
		&mut ws,
		serde_json::json!({"type": "session", "session_id": "role-on-bind", "role": "assistant"}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let _status = read_json(&mut ws).await;
	assert_eq!(server.sessions.lock().await["role-on-bind"].role, "outbound");

	crate::session::context::cleanup_session(&"role-on-bind".to_string());
}

#[tokio::test]
#[serial_test::serial]
async fn bind_with_an_unknown_tap_role_is_refused_without_creating_a_session() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut ws = connect_ws(server.addr).await;
	let _welcome = read_json(&mut ws).await;

	send_json(
		&mut ws,
		serde_json::json!({
			"type": "session", "session_id": "bad-role",
			"role": "no-such-tap-zzz:nothing", "request_id": "b1"
		}),
	)
	.await;
	let _ack = read_json(&mut ws).await;
	let error = read_json(&mut ws).await;
	assert_eq!(error["type"], "error", "got: {error}");
	assert_eq!(error["request_id"], "b1");
	assert!(
		error["message"]
			.as_str()
			.unwrap_or_default()
			.contains("Failed to resolve role 'no-such-tap-zzz:nothing'"),
		"got: {error}"
	);
	assert!(!server.sessions.lock().await.contains_key("bad-role"));
	assert!(!crate::session::get_sessions_dir()
		.unwrap()
		.join("bad-role.jsonl.zst")
		.exists());
}

#[tokio::test]
async fn server_with_a_token_refuses_an_unauthenticated_client() {
	let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("probe");
	let addr = probe.local_addr().expect("addr");
	drop(probe);
	let server = WebSocketServer::new(
		"127.0.0.1",
		addr.port(),
		template_config(),
		"assistant".to_string(),
		Vec::new(),
	)
	.expect("server")
	.with_token(Some("tok-123".to_string()));
	tokio::spawn(async move {
		let _ = server.start().await;
	});
	for _ in 0..100 {
		if tokio::net::TcpStream::connect(addr).await.is_ok() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}

	let refused = tokio_tungstenite::connect_async(format!("ws://{addr}/")).await;
	assert!(refused.is_err(), "no token must be refused");

	use tokio_tungstenite::tungstenite::client::IntoClientRequest;
	let mut request = format!("ws://{addr}/").into_client_request().unwrap();
	request
		.headers_mut()
		.insert(AUTHORIZATION, "Bearer tok-123".parse().unwrap());
	let (mut ws, _) = tokio_tungstenite::connect_async(request)
		.await
		.expect("the right token is accepted");
	let welcome = read_json(&mut ws).await;
	assert_eq!(welcome["type"], "status");
}

fn register_streaming_tap(id: &str) {
	use crate::session::tap_runs::{self, TapJob, TapJobStatus, TapLiveState};
	let (cancel_tx, _) = tokio::sync::watch::channel(false);
	tap_runs::register_job(TapJob {
		id: id.to_string(),
		role: "developer:cto".to_string(),
		workdir: ".".to_string(),
		started_at: std::time::SystemTime::now(),
		status: Arc::new(std::sync::RwLock::new(TapJobStatus::Running)),
		error: None,
		cancel_tx,
		live: Arc::new(std::sync::RwLock::new(TapLiveState::default())),
	});
	tap_runs::with_run_events(|events, parent| events.begin(parent, id, "developer:cto"))
		.expect("start run events")
		.expect("websocket session");
}

fn child_text(text: &str) -> serde_json::Value {
	serde_json::json!({"params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})
}

#[tokio::test]
#[serial_test::serial]
async fn run_events_reach_every_bound_client_between_turns() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut first = connect_ws(server.addr).await;
	read_json(&mut first).await;
	let parent = create_session(&mut first, None).await;
	let mut second = connect_ws(server.addr).await;
	read_json(&mut second).await;
	create_session(&mut second, Some(&parent)).await;
	// A repeated bind must not duplicate run events on the same connection.
	create_session(&mut second, Some(&parent)).await;
	let run_id = "tap-developer-cto-abcdef";
	crate::session::context::with_session_id(parent.clone(), async {
		register_streaming_tap(run_id);
		crate::mcp::agent::functions::route_child_update(&child_text("idle child"), Some(run_id), true);
	})
	.await;
	for client in [&mut first, &mut second] {
		let running = read_json(client).await;
		assert_eq!(running["type"], "run_event");
		assert_eq!(running["seq"], 1);
		let text = read_json(client).await;
		assert_eq!(text["type"], "run_event");
		assert_eq!(
			text["event"],
			serde_json::json!({"kind":"assistant","content":"idle child"})
		);
		assert_eq!(text["session_id"], parent);
		assert_eq!(text["run_id"], run_id);
		assert_eq!(text["role"], "developer:cto");
		assert_eq!(text["seq"], 2);
	}
	crate::session::context::cleanup_session(&parent);
}

#[tokio::test]
async fn run_log_and_child_events_bypass_processing_and_session_lock() {
	use crate::session::context::{set_notification_sender_for_session, with_session_id};
	let parent = "ws-run-log-held-turn";
	let run_id = "tap-developer-cto-abcdef";
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let addr = listener.local_addr().unwrap();
	let state = ServerState::new(Arc::new(template_config()), "assistant".into(), Vec::new());
	let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
	let (release_tx, release_rx) = tokio::sync::oneshot::channel();
	let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
	let handler = tokio::spawn(async move {
		let (stream, peer) = listener.accept().await.unwrap();
		let (sink, frames) = tokio_tungstenite::accept_async(stream).await.unwrap().split();
		let mut reader = WsReader::new(frames, peer, HEARTBEAT_INTERVAL);
		let (sender, writer) = WsSender::spawn(sink);
		let subscriptions = RunSubscriptions(sender.clone());
		with_session_id(parent.to_string(), async {
			crate::session::context::init_session_services("developer:cto");
			crate::session::tap_runs::enable_run_events(Some(sender.clone())).unwrap();
			set_notification_sender_for_session(&parent.to_string(), notify_tx);
			register_streaming_tap(run_id);
		})
		.await;
		let lock = get_or_create_session_lock(parent, &state.session_locks).await;
		let processing = async {
			let _guard = lock.lock().await;
			entered_tx.send(()).unwrap();
			release_rx.await.unwrap();
			Ok(())
		};
		let ctx = ConnCtx {
			server: state,
			closed: CancellationToken::new(),
		};
		read_while_processing(
			processing,
			&mut reader,
			&sender,
			&ctx,
			&mut VecDeque::new(),
			&mut false,
			peer,
		)
		.await
		.unwrap();
		drop(subscriptions);
		drop(sender);
		writer.await.unwrap();
	});
	let mut client = connect_ws(addr).await;
	entered_rx.await.unwrap();
	assert_eq!(read_json(&mut client).await["seq"], 1);
	with_session_id(parent.to_string(), async {
		for text in ["first", "second"] {
			crate::mcp::agent::functions::route_child_update(&child_text(text), Some(run_id), true);
		}
		crate::mcp::agent::functions::route_child_update(
			&serde_json::json!({"params":{"update":{
				"sessionUpdate":"tool_call","toolCallId":"call-1","title":"read","rawInput":{"path":"file.rs"}
			}}}),
			Some(run_id),
			true,
		);
	})
	.await;
	for seq in 2..=4 {
		let event = read_json(&mut client).await;
		assert_eq!(
			event["type"], "run_event",
			"child frames must never be plain parent output"
		);
		assert_eq!(event["seq"], seq);
	}
	assert!(
		notify_rx.try_recv().is_err(),
		"parent notification sink must stay empty"
	);
	send_json(
		&mut client,
		serde_json::json!({"type":"run_log","session_id":parent,"run_id":run_id,"request_id":"snapshot"}),
	)
	.await;
	assert_eq!(read_json(&mut client).await["type"], "ack");
	let log = read_json(&mut client).await;
	assert_eq!(log["type"], "run_log");
	assert_eq!(log["request_id"], "snapshot");
	assert_eq!(log["source"], "live");
	assert!(log.get("error").is_none());
	assert_eq!(log["seq"], 4);
	assert_eq!(log["events"].as_array().unwrap().len(), 3);
	assert_eq!(log["events"][1]["content"], "firstsecond");
	with_session_id(parent.to_string(), async {
		crate::mcp::agent::functions::route_child_update(&child_text("after snapshot"), Some(run_id), true);
	})
	.await;
	let next = read_json(&mut client).await;
	assert_eq!(next["seq"].as_u64().unwrap(), log["seq"].as_u64().unwrap() + 1);
	assert_eq!(next["event"]["content"], "after snapshot");
	for bad in ["../escape", "owner/repo", "tap-x-xyz"] {
		send_json(
			&mut client,
			serde_json::json!({"type":"run_log","session_id":parent,"run_id":bad,"request_id":"bad-id"}),
		)
		.await;
		let error = read_json(&mut client).await;
		assert_eq!(
			error,
			serde_json::json!({
				"type":"run_log","session_id":parent,"run_id":bad,"status":"unknown","source":"disk",
				"seq":0,"truncated":false,"events":[],"error":format!("invalid run_id: {bad}"),"request_id":"bad-id"
			})
		);
	}
	release_tx.send(()).unwrap();
	handler.await.unwrap();
	crate::session::context::cleanup_session(&parent.to_string());
}

#[tokio::test]
async fn unmappable_tap_update_keeps_live_state_and_streams_the_next_event() {
	use crate::session::tap_runs;
	let parent = "ws-unmappable-tap-update".to_string();
	let run_id = "tap-developer-cto-abcdef";
	let (sender, mut receiver) = WsSender::channel();
	crate::session::context::with_session_id(parent.clone(), async {
		crate::session::context::init_session_services("developer:cto");
		tap_runs::enable_run_events(Some(sender)).unwrap();
		register_streaming_tap(run_id);
		let running: serde_json::Value =
			serde_json::from_str(receiver.try_recv().unwrap().frame.to_text().unwrap()).unwrap();
		assert_eq!(running["seq"], 1);

		crate::mcp::agent::functions::route_child_update(
			&serde_json::json!({"params":{"update":{
				"sessionUpdate":"tool_call","title":"read","rawInput":{"path":"file.rs"}
			}}}),
			Some(run_id),
			true,
		);
		assert!(matches!(
			receiver.try_recv(),
			Err(tokio::sync::mpsc::error::TryRecvError::Empty)
		));
		let job = tap_runs::find_job(run_id).unwrap();
		assert_eq!(job.status, tap_runs::TapJobStatus::Running);
		assert_eq!(job.live.last_action.as_deref(), Some("read file.rs"));

		crate::mcp::agent::functions::route_child_update(&child_text("still working"), Some(run_id), true);
		let next: serde_json::Value =
			serde_json::from_str(receiver.try_recv().unwrap().frame.to_text().unwrap()).unwrap();
		assert_eq!(
			next,
			serde_json::json!({
				"type":"run_event","session_id":parent,"run_id":run_id,"role":"developer:cto","seq":2,
				"event":{"kind":"assistant","content":"still working"}
			})
		);
		assert_eq!(
			tap_runs::find_job(run_id).unwrap().status,
			tap_runs::TapJobStatus::Running
		);
	})
	.await;
	crate::session::context::cleanup_session(&parent);
}

#[tokio::test]
async fn tap_child_forwarding_outside_websocket_is_unchanged() {
	let parent = "cli-tap-forwarding".to_string();
	let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
	crate::session::context::with_session_id(parent.clone(), async {
		crate::session::context::init_session_services("developer:cto");
		crate::session::context::set_notification_sender_for_session(&parent, sender);
		assert!(crate::session::tap_runs::with_run_events(|_, _| Ok(()))
			.unwrap()
			.is_none());
		crate::mcp::agent::functions::route_child_update(
			&child_text("legacy output"),
			Some("tap-developer-cto-abcdef"),
			false,
		);
	})
	.await;
	match receiver.try_recv().unwrap() {
		ServerMessage::Assistant(payload) => {
			assert_eq!(payload.session_id, parent);
			assert_eq!(payload.content, "legacy output");
		}
		other => panic!("expected unchanged parent output, got {other:?}"),
	}
	crate::session::context::cleanup_session(&parent);
}

#[tokio::test]
async fn disconnected_run_subscribers_are_pruned_without_restoring_parent_forwarding() {
	use crate::session::tap_runs;
	let parent = "ws-run-disconnect".to_string();
	let run_id = "tap-developer-cto-abcdef";
	let (sender, mut rx) = WsSender::channel();
	let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
	crate::session::context::with_session_id(parent.clone(), async {
		crate::session::context::init_session_services("developer:cto");
		tap_runs::enable_run_events(Some(sender.clone())).unwrap();
		crate::session::context::set_notification_sender_for_session(&parent, notify_tx);
		register_streaming_tap(run_id);
		rx.try_recv().unwrap();
		drop(rx);
		crate::mcp::agent::functions::route_child_update(&child_text("no client"), Some(run_id), true);
		assert!(notify_rx.try_recv().is_err());
		let request = super::super::protocol::RunLogMessage {
			session_id: parent.clone(),
			run_id: run_id.into(),
			request_id: None,
			summary: false,
		};
		let snapshot = tap_runs::snapshot_run_log(&request).unwrap();
		assert_eq!(snapshot.source, "live");
		assert_eq!(snapshot.seq, 2);
		assert_eq!(snapshot.events.len(), 2);
		tap_runs::unbind_run_events(&sender).unwrap();
		assert!(
			tap_runs::with_run_events(|_, _| Ok(())).unwrap().is_some(),
			"transport outlives subscriptions"
		);
	})
	.await;
	crate::session::context::cleanup_session(&parent);
}

#[tokio::test]
#[serial_test::serial]
async fn run_log_disk_reply_and_missing_run_echo_request_id() {
	use crate::websocket::protocol::RunLogMessage;
	let _data = TestDataDirGuard::new();
	let run_id = "tap-developer-cto-abcdef";
	let path = crate::session::get_sessions_dir()
		.unwrap()
		.join(format!("{run_id}.jsonl.zst"));
	let mut session = crate::session::Session::new(run_id.into(), "ollama:fake-model".into());
	session.session_file = Some(path.clone());
	session.save().unwrap();
	crate::session::append_to_session_file(
		&path,
		&serde_json::to_string(&crate::session::Message {
			role: "user".into(),
			content: "disk task".into(),
			..Default::default()
		})
		.unwrap(),
	)
	.unwrap();
	let (sender, mut receiver) = WsSender::channel();
	let request = RunLogMessage {
		session_id: "disk-parent".into(),
		run_id: run_id.into(),
		request_id: Some("disk-request".into()),
		summary: false,
	};
	handle_run_log(request.clone(), &sender).await.unwrap();
	let reply: serde_json::Value = serde_json::from_str(receiver.try_recv().unwrap().frame.to_text().unwrap()).unwrap();
	assert_eq!(
		reply,
		serde_json::json!({
			"type":"run_log","session_id":"disk-parent","run_id":run_id,"status":"unknown","source":"disk","seq":0,
			"truncated":false,"events":[{"kind":"prompt","content":"disk task"}],"request_id":"disk-request"
		})
	);
	handle_run_log(
		RunLogMessage {
			run_id: "tap-missing-000000".into(),
			..request.clone()
		},
		&sender,
	)
	.await
	.unwrap();
	let reply: serde_json::Value = serde_json::from_str(receiver.try_recv().unwrap().frame.to_text().unwrap()).unwrap();
	assert_eq!(
		reply,
		serde_json::json!({
			"type":"run_log","session_id":"disk-parent","run_id":"tap-missing-000000","status":"unknown",
			"source":"disk","seq":0,"truncated":false,"events":[],
			"error":"run not found: tap-missing-000000","request_id":"disk-request"
		})
	);
	use crate::session::tap_runs::{self, TapJob, TapJobStatus, TapLiveState};
	crate::session::context::with_session_id(request.session_id.clone(), async {
		tap_runs::init_for_session();
		tap_runs::enable_run_events(Some(sender.clone())).unwrap();
		let failed_id = "tap-missing-000000";
		let status = Arc::new(std::sync::RwLock::new(TapJobStatus::Running));
		let (cancel_tx, _) = tokio::sync::watch::channel(false);
		tap_runs::register_job(TapJob {
			id: failed_id.into(),
			role: "developer:cto".into(),
			workdir: ".".into(),
			started_at: std::time::SystemTime::now(),
			status: status.clone(),
			error: None,
			cancel_tx,
			live: Arc::new(std::sync::RwLock::new(TapLiveState::default())),
		});
		tap_runs::with_run_events(|events, parent| events.begin(parent, failed_id, "developer:cto"))
			.unwrap()
			.unwrap();
		receiver.try_recv().unwrap();
		tap_runs::finish_run(
			failed_id,
			&status,
			TapJobStatus::Failed,
			Some("child failed to start".into()),
		)
		.unwrap();
		let live: serde_json::Value =
			serde_json::from_str(receiver.try_recv().unwrap().frame.to_text().unwrap()).unwrap();
		handle_run_log(
			RunLogMessage {
				run_id: failed_id.into(),
				..request.clone()
			},
			&sender,
		)
		.await
		.unwrap();
		let reply: serde_json::Value =
			serde_json::from_str(receiver.try_recv().unwrap().frame.to_text().unwrap()).unwrap();
		assert_eq!(
			reply,
			serde_json::json!({
				"type":"run_log","session_id":"disk-parent","run_id":failed_id,"role":"developer:cto",
				"status":"failed","source":"disk","seq":2,"truncated":false,
				"events":[{"kind":"status","status":"failed","error":"child failed to start"}],
				"request_id":"disk-request"
			})
		);
		assert_eq!(reply["events"][0], live["event"]);
	})
	.await;
	tap_runs::clear_for_session(&request.session_id);
}

// ---- observer connections ----

/// Opens an observer connection and returns it past its `observing` frame.
async fn connect_observer(addr: std::net::SocketAddr, sessions: serde_json::Value) -> ClientWs {
	let mut ws = connect_ws(addr).await;
	let _welcome = read_json(&mut ws).await;
	send_json(&mut ws, serde_json::json!({"type": "observe", "sessions": sessions})).await;
	let observing = read_json(&mut ws).await;
	assert_eq!(observing["type"], "observing", "first frame: {observing}");
	assert!(
		observing["capabilities"]
			.as_array()
			.is_some_and(|caps| caps.iter().any(|cap| cap == "observe_v1")),
		"got: {observing}"
	);
	ws
}

/// The next `observed` envelope of `session_id`, skipping anything else.
async fn read_observed(ws: &mut ClientWs, session_id: &str) -> serde_json::Value {
	read_until(ws, |v| v["type"] == "observed" && v["session_id"] == session_id).await
}

/// The frames of `session_id`'s next turn, as an observer reads them: up to and
/// including its cost frame.
async fn observed_turn(ws: &mut ClientWs, session_id: &str) -> Vec<serde_json::Value> {
	let mut frames = Vec::new();
	loop {
		let envelope = read_observed(ws, session_id).await;
		let done = envelope["frame"]["type"] == "cost";
		frames.push(envelope["frame"].clone());
		if done {
			return frames;
		}
	}
}

/// The cloud's recorder holds one observer per machine: whatever any session
/// outputs reaches it as the very frames a driver reads, and so does a turn the
/// session runs by itself while no client is bound.
#[tokio::test]
#[serial_test::serial]
async fn an_observer_receives_every_session_including_a_background_turn_with_no_client() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("DRIVEN-A"), final_response("BACKGROUND-B")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut observer = connect_observer(server.addr, serde_json::json!([])).await;

	let mut driver = connect_ws(server.addr).await;
	let _welcome = read_json(&mut driver).await;
	create_session(&mut driver, Some("obs-a")).await;
	let opened = read_until(&mut observer, |v| {
		v["type"] == "stream_open" && v["session_id"] == "obs-a"
	})
	.await;
	assert_eq!(opened["last_seq"], 0, "got: {opened}");
	assert_eq!(opened["gap"], false);

	send_json(
		&mut driver,
		serde_json::json!({"type": "message", "session_id": "obs-a", "content": "hello"}),
	)
	.await;
	let driven: Vec<serde_json::Value> = frames_through_cost(&mut driver)
		.await
		.into_iter()
		.filter(|frame| frame.get("stream_seq").is_some())
		.collect();
	assert_eq!(
		observed_turn(&mut observer, "obs-a").await,
		driven,
		"the observer gets the frames the driver gets"
	);

	// A second session, bound by a driver that leaves before its work arrives.
	create_session(&mut driver, Some("obs-b")).await;
	drop(driver);
	tokio::time::sleep(Duration::from_millis(300)).await;
	push_inbox_message_for_session(
		"obs-b",
		InboxMessage {
			source: InboxSource::Inject,
			content: "background work".to_string(),
		},
	);
	let background = observed_turn(&mut observer, "obs-b").await;
	assert_eq!(background[0]["type"], "injected", "got: {background:?}");
	assert!(
		background
			.iter()
			.any(|f| f["type"] == "assistant" && f["content"].as_str().unwrap_or_default().contains("BACKGROUND-B")),
		"the background turn reaches the observer: {background:?}"
	);

	crate::session::context::cleanup_session(&"obs-a".to_string());
	crate::session::context::cleanup_session(&"obs-b".to_string());
}

/// A reconnecting observer says where it stopped: the same stream replays only
/// what follows, another one replays all that is held.
#[tokio::test]
#[serial_test::serial]
async fn an_observer_reconnecting_with_a_mark_replays_only_what_it_missed() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("MARKED")]).await;
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut driver = connect_ws(server.addr).await;
	let _welcome = read_json(&mut driver).await;
	send_json(
		&mut driver,
		serde_json::json!({"type": "session", "session_id": "obs-mark"}),
	)
	.await;
	let ack = read_json(&mut driver).await;
	assert!(
		ack["capabilities"]
			.as_array()
			.is_some_and(|caps| caps.iter().any(|cap| cap == "observe_v1")),
		"the bind ack advertises observe_v1: {ack}"
	);
	let stream = ack["stream"].as_str().expect("stream id").to_string();
	let _greeting = read_json(&mut driver).await;
	send_json(
		&mut driver,
		serde_json::json!({"type": "message", "session_id": "obs-mark", "content": "hello"}),
	)
	.await;
	let turn = frames_through_cost(&mut driver).await;
	let last_seq = turn
		.iter()
		.filter_map(|frame| frame["stream_seq"].as_u64())
		.max()
		.expect("the turn is numbered");

	let mut resumed = connect_observer(
		server.addr,
		serde_json::json!([{"session_id": "obs-mark", "stream": stream, "after": last_seq - 1}]),
	)
	.await;
	let opened = read_json(&mut resumed).await;
	assert_eq!(opened["type"], "stream_open", "got: {opened}");
	assert_eq!(opened["stream"], stream.as_str());
	assert_eq!(opened["last_seq"], last_seq);
	assert_eq!(opened["gap"], false);
	let only = read_json(&mut resumed).await;
	assert_eq!(only["type"], "observed");
	assert_eq!(only["frame"]["type"], "cost");
	assert_eq!(only["frame"]["stream_seq"], last_seq);
	assert!(stays_unanswered(&mut resumed).await, "nothing else was missed");

	let mut foreign = connect_observer(
		server.addr,
		serde_json::json!([{"session_id": "obs-mark", "stream": "0123456789abcdef", "after": last_seq - 1}]),
	)
	.await;
	let opened = read_json(&mut foreign).await;
	assert_eq!(opened["type"], "stream_open", "got: {opened}");
	assert_eq!(opened["gap"], false);
	let mut seqs = Vec::new();
	for _ in 0..last_seq {
		seqs.push(read_json(&mut foreign).await["frame"]["stream_seq"].as_u64().unwrap());
	}
	assert_eq!(
		seqs,
		(1..=last_seq).collect::<Vec<_>>(),
		"another stream: every held frame"
	);

	crate::session::context::cleanup_session(&"obs-mark".to_string());
}

/// Unloading a session closes its stream for observers, and an observer only
/// reads: every frame it sends is refused while the connection stays open.
#[tokio::test]
#[serial_test::serial]
async fn an_observer_hears_a_session_unload_and_refuses_every_frame_of_its_own() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut observer = connect_observer(server.addr, serde_json::json!([])).await;
	let mut driver = connect_ws(server.addr).await;
	let _welcome = read_json(&mut driver).await;
	create_session(&mut driver, Some("obs-close")).await;
	let opened = read_until(&mut observer, |v| {
		v["type"] == "stream_open" && v["session_id"] == "obs-close"
	})
	.await;

	send_json(
		&mut driver,
		serde_json::json!({"type": "close", "session_id": "obs-close"}),
	)
	.await;
	let _ack = read_json(&mut driver).await;
	let _closed = read_json(&mut driver).await;
	let closed = read_until(&mut observer, |v| v["type"] == "stream_closed").await;
	assert_eq!(closed["session_id"], "obs-close");
	assert_eq!(closed["stream"], opened["stream"]);

	for frame in [
		serde_json::json!({"type": "message", "session_id": "obs-close", "content": "hi"}),
		serde_json::json!({"type": "session", "session_id": "obs-close"}),
		serde_json::json!({"type": "cancel", "session_id": "obs-close"}),
		serde_json::json!({"type": "observe"}),
	] {
		send_json(&mut observer, frame.clone()).await;
		let refusal = read_json(&mut observer).await;
		assert_eq!(refusal["type"], "error", "{frame}: {refusal}");
		assert_eq!(refusal["message"], "observer connections are read-only", "{frame}");
	}

	// Still connected: the next load of the session is announced under a new stream.
	create_session(&mut driver, Some("obs-close")).await;
	let reopened = read_until(&mut observer, |v| v["type"] == "stream_open").await;
	assert_eq!(reopened["session_id"], "obs-close");
	assert_ne!(reopened["stream"], opened["stream"]);

	crate::session::context::cleanup_session(&"obs-close".to_string());
}

/// A connection that bound a session may own its output and answer its
/// questions; turning it into an observer would strand both, so `observe` is
/// refused there and the connection keeps driving the session.
#[tokio::test]
#[serial_test::serial]
async fn observe_after_a_bind_is_refused_and_the_connection_keeps_its_session() {
	let _data = TestDataDirGuard::new();
	let server = LoopbackServer::start(Arc::new(ws_fake_config())).await;
	let mut driver = connect_ws(server.addr).await;
	let _welcome = read_json(&mut driver).await;
	create_session(&mut driver, Some("obs-after-bind")).await;

	send_json(&mut driver, serde_json::json!({"type": "observe"})).await;
	let refusal = read_json(&mut driver).await;
	assert_eq!(refusal["type"], "error", "{refusal}");
	assert_eq!(
		refusal["message"],
		"observe must come before any session bind on a connection"
	);

	// Not an observer: its next frame is handled, not refused as read-only.
	send_json(
		&mut driver,
		serde_json::json!({"type": "close", "session_id": "obs-after-bind"}),
	)
	.await;
	let ack = read_json(&mut driver).await;
	assert_ne!(ack["message"], "observer connections are read-only", "{ack}");

	crate::session::context::cleanup_session(&"obs-after-bind".to_string());
}
