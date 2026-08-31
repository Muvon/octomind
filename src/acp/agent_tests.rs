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
use crate::websocket::{
	AssistantPayload, CostPayload, McpNotificationPayload, ThinkingPayload, ToolResultPayload,
	ToolUsePayload,
};
use agent_client_protocol::schema::v1::{
	AudioContent, EmbeddedResource, ImageContent, McpServerHttp, McpServerSse, McpServerStdio,
	ResourceLink,
};
use futures::AsyncReadExt;

fn progress(tool_id: Option<&str>) -> ServerMessage {
	ServerMessage::McpNotification(McpNotificationPayload {
		server: "octofs".to_string(),
		method: "notifications/progress".to_string(),
		params: serde_json::json!({
			"progressToken": 1,
			"progress": 3.0,
			"message": "command still running"
		}),
		tool_id: tool_id.map(str::to_string),
	})
}

#[test]
fn progress_patches_the_tool_call_it_belongs_to() {
	let update = translate_server_message_to_acp(progress(Some("call-1")))
		.expect("progress with a tool id is forwarded");
	match update {
		SessionUpdate::ToolCallUpdate(upd) => {
			assert_eq!(&*upd.tool_call_id.0, "call-1");
			assert_eq!(
				upd.fields.title.as_deref(),
				Some("[octofs] command still running")
			);
			// Liveness is not completion — status must stay untouched.
			assert!(upd.fields.status.is_none());
		}
		other => panic!("expected a tool call update, got {other:?}"),
	}
}

#[test]
fn progress_without_a_tool_call_is_dropped() {
	// ACP has no session-level progress surface, so an unattributable beat has
	// nowhere to go — better dropped than rendered as agent output.
	assert!(translate_server_message_to_acp(progress(None)).is_none());
}
/// The disconnect signal must fire exactly when the stream hits EOF —
/// not on ordinary reads. `serve` relies on it to shut the process down
/// once the client closes our stdin; if it stops firing, every ACP
/// subprocess outlives its parent again.
#[tokio::test]
async fn signal_on_eof_fires_exactly_at_eof() {
	let (tx, mut rx) = tokio::sync::oneshot::channel();
	let mut reader = SignalOnEof {
		inner: futures::io::Cursor::new(b"data".to_vec()),
		eof_tx: Some(tx),
	};

	let mut buf = [0u8; 4];
	let n = reader.read(&mut buf).await.unwrap();
	assert_eq!(n, 4);
	assert!(
		matches!(
			rx.try_recv(),
			Err(tokio::sync::oneshot::error::TryRecvError::Empty)
		),
		"signal must not fire before EOF"
	);

	let n = reader.read(&mut buf).await.unwrap();
	assert_eq!(n, 0);
	rx.await.expect("EOF must fire the disconnect signal");
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn graceful_shutdown_waits_for_pending_work_in_every_session() {
	let config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("default config parses");
	let agent = Rc::new(OctomindAgent::new(
		config,
		"assistant".into(),
		Default::default(),
	));
	let idle_session = "acp-idle-session".to_string();
	let busy_session = "acp-busy-session".to_string();

	agent.sessions.borrow_mut().insert(
		idle_session.clone(),
		(ChatSession::for_tests(Vec::new()), PathBuf::new()),
	);
	agent.sessions.borrow_mut().insert(
		busy_session.clone(),
		(ChatSession::for_tests(Vec::new()), PathBuf::new()),
	);

	for session_id in [&idle_session, &busy_session] {
		crate::session::context::with_session_id(session_id.clone(), async {
			crate::session::context::init_session_services("assistant");
		})
		.await;
	}
	crate::session::shell_jobs::register_for_session(
		&busy_session,
		"job://coverage",
		"cargo test --workspace",
	);

	let waiter = agent.wait_until_idle();
	tokio::pin!(waiter);
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(20), waiter.as_mut())
			.await
			.is_err(),
		"one busy session must hold ACP open"
	);

	assert!(crate::session::shell_jobs::complete_for_session(
		&busy_session,
		"job://coverage"
	));
	agent.idle_notify.notify_waiters();
	tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
		.await
		.expect("idle transition wakes graceful shutdown");

	crate::session::context::cleanup_session(&idle_session);
	crate::session::context::cleanup_session(&busy_session);
}

// ---- translate: assistant / thinking / tool lifecycle ----

#[test]
fn assistant_message_translates_to_agent_message_chunk() {
	let update = translate_server_message_to_acp(ServerMessage::Assistant(AssistantPayload {
		content: "hello".to_string(),
		session_id: "s".to_string(),
		step: None,
	}))
	.expect("assistant text maps to an ACP update");
	match update {
		SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
			ContentBlock::Text(t) => assert_eq!(t.text, "hello"),
			other => panic!("expected a text block, got {other:?}"),
		},
		other => panic!("expected an agent message chunk, got {other:?}"),
	}
}

#[test]
fn thinking_message_translates_to_agent_thought_chunk() {
	let update = translate_server_message_to_acp(ServerMessage::Thinking(ThinkingPayload {
		content: "reasoning".to_string(),
		session_id: "s".to_string(),
	}))
	.expect("thinking text maps to an ACP update");
	match update {
		SessionUpdate::AgentThoughtChunk(chunk) => match chunk.content {
			ContentBlock::Text(t) => assert_eq!(t.text, "reasoning"),
			other => panic!("expected a text block, got {other:?}"),
		},
		other => panic!("expected an agent thought chunk, got {other:?}"),
	}
}

#[test]
fn tool_use_translates_to_an_in_progress_tool_call_with_raw_input() {
	let update = translate_server_message_to_acp(ServerMessage::ToolUse(ToolUsePayload {
		tool: "search".to_string(),
		tool_id: "call-1".to_string(),
		server: "octofs".to_string(),
		params: serde_json::json!({"query": "rust"}),
		session_id: "s".to_string(),
	}))
	.expect("tool use maps to an ACP tool call");
	match update {
		SessionUpdate::ToolCall(call) => {
			assert_eq!(&*call.tool_call_id.0, "call-1");
			assert_eq!(call.title, "search");
			assert_eq!(call.status, ToolCallStatus::InProgress);
			assert_eq!(call.raw_input, Some(serde_json::json!({"query": "rust"})));
		}
		other => panic!("expected a tool call, got {other:?}"),
	}
}

#[test]
fn successful_tool_result_restores_title_and_parses_json_output() {
	let update = translate_server_message_to_acp(ServerMessage::ToolResult(ToolResultPayload {
		tool: "search".to_string(),
		tool_id: "call-1".to_string(),
		server: "octofs".to_string(),
		content: r#"{"hits": 2}"#.to_string(),
		success: true,
		session_id: "s".to_string(),
	}))
	.expect("tool result maps to an ACP update");
	match update {
		SessionUpdate::ToolCallUpdate(upd) => {
			assert_eq!(upd.fields.status, Some(ToolCallStatus::Completed));
			assert_eq!(upd.fields.title.as_deref(), Some("search"));
			assert_eq!(upd.fields.raw_output, Some(serde_json::json!({"hits": 2})));
		}
		other => panic!("expected a tool call update, got {other:?}"),
	}
}

#[test]
fn failed_tool_result_marks_failed_and_falls_back_to_string_output() {
	let update = translate_server_message_to_acp(ServerMessage::ToolResult(ToolResultPayload {
		tool: "search".to_string(),
		tool_id: "call-2".to_string(),
		server: "octofs".to_string(),
		content: "not json".to_string(),
		success: false,
		session_id: "s".to_string(),
	}))
	.expect("failed tool result still maps to an ACP update");
	match update {
		SessionUpdate::ToolCallUpdate(upd) => {
			assert_eq!(upd.fields.status, Some(ToolCallStatus::Failed));
			assert_eq!(upd.fields.title.as_deref(), Some("search"));
			assert_eq!(upd.fields.raw_output, Some(serde_json::json!("not json")));
		}
		other => panic!("expected a tool call update, got {other:?}"),
	}
}

#[test]
fn messages_without_an_acp_equivalent_are_dropped() {
	// Cost is reported through a separate channel; status/skill have no
	// session-update shape — all must translate to None, never panic.
	assert!(
		translate_server_message_to_acp(ServerMessage::Cost(CostPayload {
			session_tokens: 10,
			session_cost: 0.1,
			input_tokens: 5,
			output_tokens: 5,
			cache_read_tokens: 0,
			cache_write_tokens: 0,
			reasoning_tokens: 0,
			session_id: "s".to_string(),
		}))
		.is_none()
	);
	assert!(
		translate_server_message_to_acp(ServerMessage::status("hi".to_string(), None)).is_none()
	);
	assert!(translate_server_message_to_acp(ServerMessage::skill(
		"activate",
		"rust",
		Some("file(Cargo.toml)".to_string()),
		"s",
	))
	.is_none());
}

// ---- available commands ----

#[test]
fn available_commands_are_advertised_without_leading_slash() {
	let commands = build_available_commands();
	assert!(!commands.is_empty());

	let mut names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
	for name in &names {
		assert!(
			!name.starts_with('/'),
			"clients prepend the slash themselves: {name}"
		);
	}
	let mut sorted = names.clone();
	sorted.sort();
	sorted.dedup();
	assert_eq!(sorted.len(), names.len(), "command names must be unique");
	names.sort();
	assert!(names.contains(&"done"), "done must be advertised");
	assert!(names.contains(&"help"), "help must be advertised");

	for command in &commands {
		assert!(
			!command.description.is_empty(),
			"{} needs a description",
			command.name
		);
	}
	// Input hints are attached where a command takes arguments.
	assert!(commands
		.iter()
		.any(|c| c.name == "model" && c.input.is_some()));
}

// ---- agent construction, session locks, one-shot args ----

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

fn agent_with(options: crate::acp::AcpRunOptions) -> Rc<OctomindAgent> {
	Rc::new(OctomindAgent::new(
		template_config(),
		"assistant".to_string(),
		options,
	))
}

#[test]
fn session_lock_is_shared_per_session_id() {
	let agent = agent_with(Default::default());
	let a = agent.session_lock("s");
	let b = agent.session_lock("s");
	assert!(Rc::ptr_eq(&a, &b), "same session must reuse its lock");

	let other = agent.session_lock("other");
	assert!(
		!Rc::ptr_eq(&a, &other),
		"different sessions must not share a lock"
	);
}

#[test]
fn new_session_args_consume_one_shot_overrides_once() {
	let agent = agent_with(crate::acp::AcpRunOptions {
		name: Some("named".to_string()),
		resume: Some("old".to_string()),
		resume_recent: true,
		model: Some("openai:gpt-5".to_string()),
		hooks: vec!["hook-a".to_string()],
	});

	let first = agent.build_new_session_args();
	assert_eq!(first.name.as_deref(), Some("named"));
	assert_eq!(first.resume.as_deref(), Some("old"));
	assert!(first.resume_recent);
	assert_eq!(first.model.as_deref(), Some("openai:gpt-5"));
	assert_eq!(first.hooks, vec!["hook-a".to_string()]);
	assert_eq!(first.role, "assistant");
	assert_eq!(first.mode, "websocket");

	// The one-shot values are consumed; model/hooks persist for every session.
	let second = agent.build_new_session_args();
	assert_eq!(second.name, None);
	assert_eq!(second.resume, None);
	assert!(!second.resume_recent);
	assert_eq!(second.model.as_deref(), Some("openai:gpt-5"));
	assert_eq!(second.hooks, vec!["hook-a".to_string()]);
}

#[test]
fn load_session_args_resume_by_id_and_apply_sticky_overrides() {
	let agent = agent_with(crate::acp::AcpRunOptions {
		model: Some("openai:gpt-5".to_string()),
		hooks: vec!["hook-a".to_string()],
		..Default::default()
	});

	let args = agent.build_load_session_args("sid-9".to_string());
	assert_eq!(args.resume.as_deref(), Some("sid-9"));
	assert_eq!(
		args.name, None,
		"load_session never consumes the new-session name"
	);
	assert_eq!(args.model.as_deref(), Some("openai:gpt-5"));
	assert_eq!(args.hooks, vec!["hook-a".to_string()]);
	assert_eq!(args.role, "assistant");
}

// ---- build_config_with_injected_servers ----

#[test]
fn injected_servers_merge_stdio_and_http_but_skip_sse_and_duplicates() {
	let base = template_config();
	let servers = vec![
		McpServer::Stdio(
			McpServerStdio::new("injected-stdio", "/usr/local/bin/fs")
				.args(vec!["--stdio".to_string()]),
		),
		McpServer::Http(McpServerHttp::new(
			"injected-http",
			"https://mcp.example.com/rpc",
		)),
		McpServer::Sse(McpServerSse::new(
			"injected-sse",
			"https://mcp.example.com/sse",
		)),
	];

	let merged = build_config_with_injected_servers(&base, "assistant", &servers);
	assert!(
		merged
			.mcp
			.servers
			.iter()
			.any(|s| s.name() == "injected-stdio"),
		"stdio server must be merged"
	);
	assert!(
		merged
			.mcp
			.servers
			.iter()
			.any(|s| s.name() == "injected-http"),
		"http server must be merged"
	);
	assert!(
		!merged
			.mcp
			.servers
			.iter()
			.any(|s| s.name() == "injected-sse"),
		"SSE transport is unsupported and must be skipped"
	);

	// The same server injected twice is not duplicated.
	let again = build_config_with_injected_servers(&merged, "assistant", &servers);
	assert_eq!(
		again
			.mcp
			.servers
			.iter()
			.filter(|s| s.name() == "injected-stdio")
			.count(),
		1,
		"duplicate injection must not add a second entry"
	);

	// The base config is never mutated — injection is scoped to the snapshot.
	assert!(!base
		.mcp
		.servers
		.iter()
		.any(|s| s.name() == "injected-stdio"));
}

// ---- prompt / cancel / initialize / authenticate / ext_method ----

#[tokio::test]
async fn prompt_with_no_content_ends_the_turn_without_touching_sessions() {
	let agent = agent_with(Default::default());
	let response = agent
		.prompt(PromptRequest::new(
			"no-such-session".to_string(),
			vec!["".into()],
		))
		.await
		.expect("empty input short-circuits before session lookup");
	assert!(matches!(response.stop_reason, StopReason::EndTurn));
}

#[tokio::test]
async fn prompt_for_an_unknown_session_is_invalid_params() {
	let agent = agent_with(Default::default());
	let err = agent
		.prompt(PromptRequest::new(
			"no-such-session".to_string(),
			vec!["hello".into()],
		))
		.await
		.expect_err("a prompt for a missing session must fail, not hang");
	let detail = err
		.data
		.as_ref()
		.and_then(|d| d.as_str())
		.unwrap_or_default();
	assert!(detail.contains("session not found"), "got: {detail}");
}

#[tokio::test]
async fn prompt_with_image_only_content_still_routes_to_the_session_lookup() {
	let agent = agent_with(Default::default());
	let blocks = vec![ContentBlock::Image(ImageContent::new(
		"ZmFrZQ==",
		"image/png",
	))];
	let err = agent
		.prompt(PromptRequest::new("no-such-session".to_string(), blocks))
		.await
		.expect_err("image-only prompts proceed to the session pipeline");
	let detail = err
		.data
		.as_ref()
		.and_then(|d| d.as_str())
		.unwrap_or_default();
	assert!(detail.contains("session not found"), "got: {detail}");
}

#[tokio::test]
async fn prompt_extracts_video_resources_and_skips_audio_and_links() {
	let agent = agent_with(Default::default());
	let video = ContentBlock::Resource(EmbeddedResource::new(
		EmbeddedResourceResource::BlobResourceContents(
			BlobResourceContents::new("ZmFrZQ==", "file://clip.mp4").mime_type("video/mp4"),
		),
	));
	let blocks = vec![
		video,
		ContentBlock::Audio(AudioContent::new("ZmFrZQ==", "audio/mp3")),
		ContentBlock::ResourceLink(ResourceLink::new("doc", "file://doc.md")),
		"hi".into(),
	];
	let err = agent
		.prompt(PromptRequest::new("no-such-session".to_string(), blocks))
		.await
		.expect_err("mixed content proceeds to the session pipeline");
	let detail = err
		.data
		.as_ref()
		.and_then(|d| d.as_str())
		.unwrap_or_default();
	assert!(detail.contains("session not found"), "got: {detail}");
}

#[tokio::test]
async fn cancel_for_an_unknown_session_is_acknowledged() {
	let agent = agent_with(Default::default());
	agent
		.cancel(CancelNotification::new("no-such-session".to_string()))
		.await
		.expect("cancelling an unknown session is a no-op, not an error");
}

#[tokio::test]
async fn initialize_advertises_agent_info_and_capabilities() {
	let agent = agent_with(Default::default());
	let request = InitializeRequest::new(ProtocolVersion::LATEST)
		.client_info(Implementation::new("test-client", "1.0"));
	let response = agent
		.initialize(request)
		.await
		.expect("initialize succeeds");
	assert_eq!(response.protocol_version, ProtocolVersion::LATEST);
	let info = response.agent_info.as_ref().expect("agent info advertised");
	assert_eq!(info.name, "octomind");
	assert!(
		response.agent_capabilities.load_session,
		"load_session is supported"
	);
}

#[tokio::test]
async fn authenticate_returns_the_default_response() {
	let agent = agent_with(Default::default());
	let response = agent
		.authenticate(AuthenticateRequest::new("local"))
		.await
		.expect("local auth needs no interaction");
	assert_eq!(response, AuthenticateResponse::default());
}

#[tokio::test]
async fn ext_method_rejects_foreign_namespaces() {
	let agent = agent_with(Default::default());
	let raw = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw params");
	let request = ExtRequest::new("other/thing", std::sync::Arc::from(raw));
	let result = agent.ext_method(request).await;
	assert!(
		result.is_err(),
		"only the octomind/command namespace is handled"
	);
}

// ---- record_telemetry ----

#[tokio::test]
#[serial_test::serial]
async fn record_telemetry_drains_sessions_without_panicking() {
	// Zero sessions: the loop body never runs.
	let empty = agent_with(Default::default());
	empty.record_telemetry();

	// One in-memory session: record_session is invoked per session.
	let with_session = agent_with(Default::default());
	with_session.sessions.borrow_mut().insert(
		"s1".to_string(),
		(
			ChatSession::for_tests(Vec::new()),
			std::env::current_dir().expect("cwd"),
		),
	);
	with_session.record_telemetry();
}

// ---- run_actor ----

#[tokio::test(flavor = "current_thread")]
async fn run_actor_dispatches_initialize_cancel_and_idle() {
	let agent = agent_with(Default::default());
	let (tx, rx) = mpsc::unbounded_channel();

	let local = tokio::task::LocalSet::new();
	local.spawn_local(run_actor(agent, rx));

	tokio::time::timeout(
		std::time::Duration::from_secs(5),
		local.run_until(async move {
			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::Initialize(
				Box::new(InitializeRequest::new(ProtocolVersion::LATEST)),
				reply,
			))
			.expect("actor alive");
			let response = rx_reply.await.expect("reply").expect("initialize ok");
			assert!(response.agent_info.is_some());

			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::Authenticate(
				AuthenticateRequest::new("local"),
				reply,
			))
			.expect("actor alive");
			rx_reply.await.expect("reply").expect("authenticate ok");

			// Cancel runs inline in the actor loop.
			tx.send(Command::Cancel(CancelNotification::new(
				"ghost".to_string(),
			)))
			.expect("actor alive");

			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::WaitUntilIdle(reply)).expect("actor alive");
			rx_reply.await.expect("idle reply");

			drop(tx); // ends the actor loop
		}),
	)
	.await
	.expect("actor commands must complete within the timeout");

	tokio::time::timeout(std::time::Duration::from_secs(5), local)
		.await
		.expect("actor loop must stop after its sender is dropped");
}

// ---- full session lifecycle against the scripted provider ----

use std::time::Duration;

use crate::session::chat::test_support::{final_response, spawn_stub_with_status, ENV_LOCK};
use crate::session::context;
use crate::session::inbox::{push_inbox_message_for_session, InboxMessage, InboxSource};

/// Points OCTOMIND_DATA_DIR at a unique temp dir and restores the previous
/// value on drop. Session storage and the evolution registry live under it.
struct TestDataDirGuard {
	previous: Option<String>,
	_dir: tempfile::TempDir,
}

impl TestDataDirGuard {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("tempdir");
		let previous = std::env::var("OCTOMIND_DATA_DIR").ok();
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
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

/// A stub that delays its response, so a prompt is genuinely in flight when
/// the client cancels.
async fn spawn_slow_stub(delay_ms: u64) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
		.await
		.expect("bind slow stub");
	let addr = listener.local_addr().expect("addr");
	tokio::spawn(async move {
		while let Ok((mut sock, _)) = listener.accept().await {
			tokio::spawn(async move {
				use tokio::io::{AsyncReadExt, AsyncWriteExt};
				let mut buf = vec![0u8; 65536];
				let mut total = 0usize;
				loop {
					if total >= buf.len() {
						return;
					}
					let n = sock.read(&mut buf[total..]).await.unwrap_or(0);
					if n == 0 {
						return;
					}
					total += n;
					let head_end = buf[..total].windows(4).position(|w| w == b"\r\n\r\n");
					let Some(head_end) = head_end else {
						continue;
					};
					let head = String::from_utf8_lossy(&buf[..total]).to_string();
					let content_length = head
						.split("content-length:")
						.nth(1)
						.and_then(|s| s.split(['\r', '\n']).next())
						.and_then(|s| s.trim().parse::<usize>().ok())
						.unwrap_or(0);
					let body_start = head_end + 4;
					while total - body_start < content_length {
						let n = sock.read(&mut buf[total..]).await.unwrap_or(0);
						if n == 0 {
							break;
						}
						total += n;
					}
					break;
				}
				tokio::time::sleep(Duration::from_millis(delay_ms)).await;
				let body = final_response("SLOW-TURN").to_string();
				let response = format!(
					"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
					body.len(),
					body
				);
				let _ = sock.write_all(response.as_bytes()).await;
				let _ = sock.shutdown().await;
			});
		}
	});
	format!("http://{addr}/v1/chat/completions")
}

fn acp_fake_config() -> Config {
	let mut config = template_config();
	config.model = "ollama:fake-model".to_string();
	config.supervisor.enabled = false;
	config.compression.decision.model = "ollama:fake-model".to_string();
	config
}

fn acp_agent() -> Rc<OctomindAgent> {
	Rc::new(OctomindAgent::new(
		acp_fake_config(),
		"assistant".to_string(),
		Default::default(),
	))
}

fn msg(role: &str, content: &str) -> crate::session::Message {
	crate::session::Session::build_message(role, content)
}

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

/// Inserts a memory-only session and initializes its session-scoped services.
async fn install_session(agent: &Rc<OctomindAgent>, session_id: &str, session: ChatSession) {
	agent.sessions.borrow_mut().insert(
		session_id.to_string(),
		(session, std::env::current_dir().expect("cwd")),
	);
	context::with_session_id(session_id.to_string(), async {
		context::init_session_services("assistant");
	})
	.await;
}

async fn inbox_has_messages(session_id: &str) -> bool {
	context::with_session_id(session_id.to_string(), async {
		crate::session::inbox::has_inbox_messages()
	})
	.await
}

// ---- idle / config injection ----

#[tokio::test]
async fn wait_until_idle_treats_lock_only_sessions_as_idle() {
	let agent = agent_with(Default::default());
	// A session known only to the lock map (e.g. a prompt in flight elsewhere)
	// must still be waited on, but with no services it has no pending work.
	agent.session_lock("ghost-session");
	tokio::time::timeout(Duration::from_secs(2), agent.wait_until_idle())
		.await
		.expect("lock-only session must count as idle");
}

#[test]
fn injected_servers_are_added_to_the_role_entry_server_refs() {
	let base = template_config();
	let servers = vec![McpServer::Stdio(
		McpServerStdio::new("injected-stdio", "/usr/local/bin/fs")
			.args(vec!["--stdio".to_string()]),
	)];

	let merged = build_config_with_injected_servers(&base, "assistant", &servers);
	let entry = merged
		.role_map
		.get("assistant")
		.expect("assistant role exists in the template");
	assert!(
		entry.mcp.server_refs.iter().any(|r| r == "injected-stdio"),
		"the injected server must be bound to the role, got: {:?}",
		entry.mcp.server_refs
	);
}

// ---- new_session / prompt / load_session ----

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn new_session_creates_and_registers_a_session_with_cancellation() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	let session_id = local
		.run_until(async {
			let response = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = response.session_id.to_string();
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the session is registered"
			);
			assert!(
				agent.cancellations.borrow().contains_key(&session_id),
				"a cancellation handle is registered"
			);
			session_id
		})
		.await;

	context::cleanup_session(&session_id);
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_runs_a_full_turn_against_the_scripted_provider() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("ACP-TURN-OK")]).await;
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();

			let response = agent
				.prompt(PromptRequest::new(session_id.clone(), vec!["hello".into()]))
				.await
				.expect("prompt completes");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));

			let sessions = agent.sessions.borrow();
			let (session, _) = sessions
				.get(session_id.as_str())
				.expect("session returned to the map");
			let last = session.session.messages.last().expect("assistant message");
			assert_eq!(last.role, "assistant");
			assert!(
				last.content.contains("ACP-TURN-OK"),
				"got: {}",
				last.content
			);
			drop(sessions);
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_streams_tool_lifecycle_updates() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![
		crate::session::chat::test_support::tool_call_response(
			"no_such_tool_zzz",
			serde_json::json!({}),
		),
		final_response("TOOL-DONE"),
	])
	.await;
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();

			let response = agent
				.prompt(PromptRequest::new(
					session_id.clone(),
					vec!["use the tool".into()],
				))
				.await
				.expect("prompt completes after the tool round");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));

			let sessions = agent.sessions.borrow();
			let (session, _) = sessions
				.get(session_id.as_str())
				.expect("session returned to the map");
			assert!(
				session.session.messages.iter().any(|m| m.role == "tool"),
				"the unknown tool must have produced a tool result message"
			);
			drop(sessions);
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_done_with_instructions_compression_then_turn() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("AFTER-DONE")]).await;
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();

			let response = agent
				.prompt(PromptRequest::new(
					session_id.clone(),
					vec!["/done continue with this".into()],
				))
				.await
				.expect("prompt completes");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));

			let sessions = agent.sessions.borrow();
			let (session, _) = sessions
				.get(session_id.as_str())
				.expect("session returned to the map");
			let last = session.session.messages.last().expect("assistant message");
			assert!(
				last.role == "assistant" && last.content.contains("AFTER-DONE"),
				"the trailing instructions must run as a user turn, got: {} {}",
				last.role,
				last.content
			);
			drop(sessions);
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_done_compresses_a_loaded_session() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![
		final_response(&xml_summary_body()),
		final_response(&xml_summary_body()),
	])
	.await;
	let agent = acp_agent();

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			install_session(&agent, "done-c-1", compressible_session()).await;
			let response = agent
				.prompt(PromptRequest::new("done-c-1", vec!["/done".into()]))
				.await
				.expect("plain /done completes");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));

			let sessions = agent.sessions.borrow();
			let (session, _) = sessions
				.get("done-c-1")
				.expect("session returned to the map after /done");
			let contents = session
				.session
				.messages
				.iter()
				.map(|m| m.content.clone())
				.collect::<String>();
			assert!(
				contents.contains("COMPRESS-E2E-CONTEXT"),
				"the compressed summary must replace the old turns, got: {contents}"
			);
			assert!(
				session.session.messages.len() < 5,
				"compression must shrink the session, got {} messages",
				session.session.messages.len()
			);
			drop(sessions);
			context::cleanup_session(&"done-c-1".to_string());
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_slash_help_is_handled_without_a_model_call() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			install_session(&agent, "slash-1", ChatSession::for_tests(Vec::new())).await;
			let response = agent
				.prompt(PromptRequest::new("slash-1", vec!["/help".into()]))
				.await
				.expect("/help completes");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));
			let sessions = agent.sessions.borrow();
			let (session, _) = sessions
				.get("slash-1")
				.expect("session returned to the map");
			assert!(
				session.session.messages.is_empty(),
				"a handled command must not add messages"
			);
			drop(sessions);
			context::cleanup_session(&"slash-1".to_string());
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_slash_exit_ends_the_turn() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			install_session(&agent, "slash-2", ChatSession::for_tests(Vec::new())).await;
			let response = agent
				.prompt(PromptRequest::new("slash-2", vec!["/exit".into()]))
				.await
				.expect("/exit completes");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));
			assert!(
				agent.sessions.borrow().contains_key("slash-2"),
				"the session map entry is kept by the ACP path"
			);
			context::cleanup_session(&"slash-2".to_string());
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_unknown_slash_command_is_answered_not_forwarded() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			install_session(&agent, "slash-3", ChatSession::for_tests(Vec::new())).await;
			let response = agent
				.prompt(PromptRequest::new(
					"slash-3",
					vec!["/definitely-not-a-command".into()],
				))
				.await
				.expect("unknown command completes");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));
			let sessions = agent.sessions.borrow();
			let (session, _) = sessions
				.get("slash-3")
				.expect("session returned to the map");
			assert!(
				session.session.messages.is_empty(),
				"an unknown command must not be treated as user input"
			);
			drop(sessions);
			context::cleanup_session(&"slash-3".to_string());
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_failing_slash_command_still_ends_the_turn() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			install_session(&agent, "slash-4", ChatSession::for_tests(Vec::new())).await;
			let response = agent
				.prompt(PromptRequest::new(
					"slash-4",
					vec!["/learning evolution show no-such-record".into()],
				))
				.await
				.expect("a failing command still ends the turn");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));
			assert!(
				agent.sessions.borrow().contains_key("slash-4"),
				"the session is returned even when the command fails"
			);
			context::cleanup_session(&"slash-4".to_string());
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn load_session_resumes_a_saved_session() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("SAVE-ME")]).await;
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd.clone()))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();
			agent
				.prompt(PromptRequest::new(session_id.clone(), vec!["hello".into()]))
				.await
				.expect("prompt saves the session");

			// Simulate a restart: the in-memory copy is gone, the file remains.
			agent.sessions.borrow_mut().remove(&session_id);

			agent
				.load_session(LoadSessionRequest::new(session_id.clone(), cwd))
				.await
				.expect("load session");
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the loaded session is registered"
			);
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn cancel_during_a_prompt_returns_a_cancelled_stop_reason() {
	let _data = TestDataDirGuard::new();
	let guard = ENV_LOCK.lock().await;
	let url = spawn_slow_stub(400).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	drop(guard);
	struct EnvRestore;
	impl Drop for EnvRestore {
		fn drop(&mut self) {
			std::env::remove_var("OLLAMA_API_URL");
		}
	}
	let _restore = EnvRestore;

	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id;

			let prompt_agent = agent.clone();
			let sid_for_task = session_id.clone();
			let task = tokio::task::spawn_local(async move {
				prompt_agent
					.prompt(PromptRequest::new(sid_for_task, vec!["slow please".into()]))
					.await
			});
			// Let the prompt reach the in-flight provider call.
			tokio::time::sleep(Duration::from_millis(100)).await;
			agent
				.cancel(CancelNotification::new(session_id.clone()))
				.await
				.expect("cancel acknowledged");

			let response = tokio::time::timeout(Duration::from_secs(15), task)
				.await
				.expect("prompt must finish after cancel")
				.expect("prompt task must not panic")
				.expect("prompt must still succeed");
			assert!(
				matches!(response.stop_reason, StopReason::Cancelled),
				"got: {:?}",
				response.stop_reason
			);
			context::cleanup_session(&session_id.to_string());
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn prompt_api_failure_maps_to_an_internal_error() {
	let _data = TestDataDirGuard::new();
	let failures: Vec<(u16, serde_json::Value)> = (0..6)
		.map(|_| (500u16, serde_json::json!({"error": {"message": "boom"}})))
		.collect();
	let _env = StubEnv::with_status(failures).await;
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();
			let error = agent
				.prompt(PromptRequest::new(session_id.clone(), vec!["hello".into()]))
				.await
				.expect_err("persistent provider failures must fail the turn");
			assert!(
				!error.to_string().is_empty(),
				"the error must carry the failure"
			);
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the session is returned even when the API call fails"
			);
			context::cleanup_session(&session_id);
		})
		.await;
}

// ---- inbox monitor ----

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn acp_monitor_processes_inbox_messages_end_to_end() {
	let _data = TestDataDirGuard::new();
	let _env = StubEnv::new(vec![final_response("ACP-MON-DONE")]).await;
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();
			// Let the monitor park.
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
					let done = agent
						.sessions
						.borrow()
						.get(&session_id)
						.and_then(|(s, _)| s.session.messages.last().cloned())
						.is_some_and(|m| {
							m.role == "assistant" && m.content.contains("ACP-MON-DONE")
						});
					if done {
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
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn acp_monitor_breaks_when_the_session_is_removed_mid_drain() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();
			tokio::time::sleep(Duration::from_millis(100)).await;

			// Hold the per-session lock BEFORE waking the monitor: it parks on
			// lock().await inside the drain loop.
			let lock = agent.session_lock(&session_id);
			let guard = lock.lock().await;
			push_inbox_message_for_session(
				&session_id,
				InboxMessage {
					source: InboxSource::Inject,
					content: "orphaned work".to_string(),
				},
			);
			tokio::time::sleep(Duration::from_millis(100)).await;
			// The session vanishes while the monitor is parked on the lock.
			agent.sessions.borrow_mut().remove(&session_id);
			drop(guard);

			tokio::time::sleep(Duration::from_millis(300)).await;
			assert!(
				inbox_has_messages(&session_id).await,
				"the monitor must break before popping, leaving the message queued"
			);
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn acp_monitor_breaks_when_the_inbox_is_emptied_by_another_consumer() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();
			tokio::time::sleep(Duration::from_millis(100)).await;

			let lock = agent.session_lock(&session_id);
			let guard = lock.lock().await;
			push_inbox_message_for_session(
				&session_id,
				InboxMessage {
					source: InboxSource::Inject,
					content: "stolen work".to_string(),
				},
			);
			tokio::time::sleep(Duration::from_millis(100)).await;
			// Another consumer drains the inbox while the monitor is parked.
			let popped = context::with_session_id(session_id.clone(), async {
				crate::session::inbox::try_pop_inbox_message()
			})
			.await;
			assert!(popped.is_some(), "the test must consume the message");
			drop(guard);

			tokio::time::sleep(Duration::from_millis(300)).await;
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the session must stay in the map untouched"
			);
			assert!(
				!inbox_has_messages(&session_id).await,
				"nothing may be requeued"
			);
			context::cleanup_session(&session_id);
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn acp_monitor_exits_when_the_session_inbox_is_destroyed() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();
	let cwd = std::env::current_dir().expect("cwd");

	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let new = agent
				.new_session(NewSessionRequest::new(cwd))
				.await
				.expect("new session");
			let session_id = new.session_id.to_string();
			tokio::time::sleep(Duration::from_millis(100)).await;

			let lock = agent.session_lock(&session_id);
			let guard = lock.lock().await;
			push_inbox_message_for_session(
				&session_id,
				InboxMessage {
					source: InboxSource::Inject,
					content: "doomed work".to_string(),
				},
			);
			tokio::time::sleep(Duration::from_millis(100)).await;
			// cleanup_session destroys the inbox while the monitor is parked.
			context::cleanup_session(&session_id);
			drop(guard);

			tokio::time::sleep(Duration::from_millis(300)).await;
			let notify = context::with_session_id(session_id.clone(), async {
				crate::session::inbox::get_inbox_notify()
			})
			.await;
			assert!(notify.is_none(), "the inbox must be destroyed");
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the monitor must not remove or resurrect the session"
			);
		})
		.await;
}

// ---- run_actor dispatch ----

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn run_actor_dispatches_session_lifecycle_commands() {
	let _data = TestDataDirGuard::new();
	let agent = acp_agent();
	let (tx, rx) = mpsc::unbounded_channel();

	let local = tokio::task::LocalSet::new();
	let actor = local.spawn_local(run_actor(agent.clone(), rx));

	tokio::time::timeout(
		Duration::from_secs(20),
		local.run_until(async move {
			let cwd = std::env::current_dir().expect("cwd");

			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::NewSession(
				NewSessionRequest::new(cwd.clone()),
				reply,
			))
			.expect("actor alive");
			let response = rx_reply.await.expect("reply").expect("new session ok");
			let session_id = response.session_id.to_string();
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the actor registers the session"
			);

			// Empty input short-circuits before the session lookup: the
			// dispatch arm itself is exercised without a provider call.
			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::Prompt(
				PromptRequest::new(session_id.clone(), vec!["".into()]),
				reply,
			))
			.expect("actor alive");
			let response = rx_reply.await.expect("reply").expect("prompt ok");
			assert!(matches!(response.stop_reason, StopReason::EndTurn));

			// Simulate a restart, then load the saved session through the actor.
			agent.sessions.borrow_mut().remove(&session_id);
			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::LoadSession(
				LoadSessionRequest::new(session_id.clone(), cwd),
				reply,
			))
			.expect("actor alive");
			rx_reply.await.expect("reply").expect("load session ok");
			assert!(
				agent.sessions.borrow().contains_key(&session_id),
				"the actor re-registers the loaded session"
			);

			let raw = serde_json::value::RawValue::from_string(
				serde_json::json!({"session_id": session_id, "command": "/help"}).to_string(),
			)
			.expect("raw params");
			let (reply, rx_reply) = oneshot::channel();
			tx.send(Command::Ext(
				ExtRequest::new("foreign/namespace", std::sync::Arc::from(raw)),
				reply,
			))
			.expect("actor alive");
			assert!(
				rx_reply.await.expect("reply").is_err(),
				"foreign ext namespaces must be rejected"
			);

			context::cleanup_session(&session_id);
			drop(tx); // ends the actor loop

			tokio::time::timeout(Duration::from_secs(5), actor)
				.await
				.expect("actor loop must stop after its sender is dropped")
				.expect("actor task must complete cleanly");
		}),
	)
	.await
	.expect("actor commands must complete within the timeout");
}
