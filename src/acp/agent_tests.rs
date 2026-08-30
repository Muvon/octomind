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
