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

//! In-memory MCP peer tests for the client registry.
//!
//! Each test serves `OctoClientHandler` over a futures mpsc transport via
//! `rmcp::service::serve_directly` — no initialize handshake, no child
//! process — then pushes server→client messages into the channel and
//! asserts on the notifications the handler forwards (observed through the
//! CLI notification sender) and the responses it sends back. Tool-call
//! rounds run against a service pre-registered under the server's name, so
//! `get_or_connect` reuses the in-memory connection instead of spawning.

#![allow(deprecated)] // LoggingMessageNotificationParam is SEP-2577-deprecated; the handler still forwards it

use super::*;
use futures::channel::mpsc;
use futures::StreamExt;
use rmcp::model::{
	CallToolResult, ClientNotification, ClientResult, CreateTaskResult, CustomNotification,
	DetailedTask, ElicitRequestParams, GetTaskResult, InputRequiredResult, JsonRpcMessage,
	JsonRpcNotification, JsonRpcVersion2_0, LoggingLevel, LoggingMessageNotificationParam,
	Notification, NotificationNoParam, ProgressNotificationParam, Request, RequestId,
	ResourceUpdatedNotificationParam, ServerRequest, SubscriptionsAcknowledgedNotificationParams,
	Task, TaskPayload, TaskStatus, TaskStatusNotificationParams,
};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc as StdArc;

/// Upper bound for any single await in these tests — a hang fails fast
/// instead of stalling the whole test binary.
const WAIT: Duration = Duration::from_secs(2);

fn unique_server(tag: &str) -> String {
	format!("octomind-test-client-{tag}")
}

/// A client service served over an in-memory transport. `incoming` carries
/// fake-server → client messages; `outgoing` carries client → fake-server
/// messages (requests the client sends and notifications it emits).
struct InMemoryPeer {
	service: McpService,
	incoming: mpsc::UnboundedSender<RxJsonRpcMessage<RoleClient>>,
	outgoing: mpsc::UnboundedReceiver<TxJsonRpcMessage<RoleClient>>,
}

fn serve_in_memory(server_name: &str) -> InMemoryPeer {
	let (in_tx, in_rx) = mpsc::unbounded::<RxJsonRpcMessage<RoleClient>>();
	let (out_tx, out_rx) = mpsc::unbounded::<TxJsonRpcMessage<RoleClient>>();
	let service =
		rmcp::service::serve_directly(OctoClientHandler::new(server_name), (out_tx, in_rx), None);
	InMemoryPeer {
		service,
		incoming: in_tx,
		outgoing: out_rx,
	}
}

fn push_notification(peer: &InMemoryPeer, notification: ServerNotification) {
	peer.incoming
		.unbounded_send(JsonRpcMessage::Notification(JsonRpcNotification {
			jsonrpc: JsonRpcVersion2_0,
			notification,
		}))
		.expect("fake server channel must stay open");
}

/// Observes what `OctoClientHandler` forwards via `process::emit_notification`
/// by registering the CLI-mode notification sender.
struct NotificationTap {
	rx: tokio::sync::mpsc::UnboundedReceiver<crate::websocket::ServerMessage>,
}

impl NotificationTap {
	fn install() -> Self {
		let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
		crate::mcp::process::set_notification_sender(None, tx);
		Self { rx }
	}

	/// Next notification for `server`/`method`, skipping stale messages other
	/// tests may have buffered before this tap was installed.
	async fn next(
		&mut self,
		server: &str,
		method: &str,
	) -> crate::websocket::McpNotificationPayload {
		tokio::time::timeout(WAIT, async {
			loop {
				let message = self
					.rx
					.recv()
					.await
					.expect("notification sender must stay open");
				if let crate::websocket::ServerMessage::McpNotification(payload) = message {
					if payload.server == server && payload.method == method {
						return payload;
					}
				}
			}
		})
		.await
		.unwrap_or_else(|_| panic!("timed out waiting for {method} from {server}"))
	}
}

impl Drop for NotificationTap {
	fn drop(&mut self) {
		crate::mcp::process::clear_notification_sender(None);
	}
}

#[serial_test::serial]
#[tokio::test]
async fn progress_notification_carries_bound_tool_id() {
	let name = unique_server("progress");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	let token = ProgressToken(rmcp::model::NumberOrString::Number(41));
	let binding = ProgressTokenBinding::new(&token, "call-41");
	push_notification(
		&peer,
		ServerNotification::ProgressNotification(Notification::new(
			ProgressNotificationParam::new(token.clone(), 0.5),
		)),
	);
	let payload = tap.next(&name, "notifications/progress").await;
	assert_eq!(payload.tool_id.as_deref(), Some("call-41"));
	assert_eq!(
		payload.params.get("progress"),
		Some(&serde_json::json!(0.5))
	);

	// Once the binding is gone the same token forwards without a tool id.
	drop(binding);
	push_notification(
		&peer,
		ServerNotification::ProgressNotification(Notification::new(
			ProgressNotificationParam::new(token, 1.0),
		)),
	);
	let payload = tap.next(&name, "notifications/progress").await;
	assert_eq!(payload.tool_id, None);
}

#[serial_test::serial]
#[tokio::test]
async fn logging_message_notification_is_forwarded() {
	let name = unique_server("logging");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	push_notification(
		&peer,
		ServerNotification::LoggingMessageNotification(Notification::new(
			LoggingMessageNotificationParam::new(
				LoggingLevel::Info,
				serde_json::json!({"line": 7}),
			),
		)),
	);
	let payload = tap.next(&name, "notifications/message").await;
	assert_eq!(
		payload.params.get("level"),
		Some(&serde_json::json!("info"))
	);
	assert_eq!(payload.params.get("line"), Some(&serde_json::json!(7)));
}

#[serial_test::serial]
#[tokio::test]
async fn list_changed_notifications_are_forwarded() {
	let name = unique_server("list-changed");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	push_notification(
		&peer,
		ServerNotification::ToolListChangedNotification(NotificationNoParam::default()),
	);
	let payload = tap.next(&name, "notifications/tools/list_changed").await;
	assert_eq!(payload.params, serde_json::Value::Null);

	push_notification(
		&peer,
		ServerNotification::ResourceListChangedNotification(NotificationNoParam::default()),
	);
	tap.next(&name, "notifications/resources/list_changed")
		.await;

	push_notification(
		&peer,
		ServerNotification::PromptListChangedNotification(NotificationNoParam::default()),
	);
	tap.next(&name, "notifications/prompts/list_changed").await;
}

#[serial_test::serial]
#[tokio::test]
async fn cancelled_notification_is_forwarded_with_request_id() {
	let name = unique_server("cancelled");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	push_notification(
		&peer,
		ServerNotification::CancelledNotification(Notification::new(
			CancelledNotificationParam::new(
				Some(RequestId::Number(5)),
				Some("done waiting".to_string()),
			),
		)),
	);
	let payload = tap.next(&name, "notifications/cancelled").await;
	assert_eq!(payload.params.get("requestId"), Some(&serde_json::json!(5)));
	assert_eq!(
		payload.params.get("reason").and_then(|r| r.as_str()),
		Some("done waiting")
	);
}

#[serial_test::serial]
#[tokio::test]
async fn resource_update_without_session_emits_and_stops() {
	// Outside a session the handler has no session_id, so delivery is the
	// forwarded notification only — no inbox push, no resource read.
	let name = unique_server("resource-updated");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	push_notification(
		&peer,
		ServerNotification::ResourceUpdatedNotification(Notification::new(
			ResourceUpdatedNotificationParam::new("file:///tmp/watched".to_string()),
		)),
	);
	let payload = tap.next(&name, "notifications/resources/updated").await;
	assert_eq!(
		payload.params.get("uri").and_then(|u| u.as_str()),
		Some("file:///tmp/watched")
	);
}

#[serial_test::serial]
#[tokio::test]
async fn subscriptions_acknowledged_and_task_status_are_forwarded() {
	let name = unique_server("subs-tasks");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	push_notification(
		&peer,
		ServerNotification::SubscriptionsAcknowledgedNotification(Notification::new(
			SubscriptionsAcknowledgedNotificationParams::new(SubscriptionFilter::new()),
		)),
	);
	tap.next(&name, "notifications/subscriptions/acknowledged")
		.await;

	let task = DetailedTask::new(
		Task::new(
			"task-9",
			TaskStatus::Working,
			"2026-01-01T00:00:00Z",
			"2026-01-01T00:00:00Z",
		),
		TaskPayload::Working,
	);
	push_notification(
		&peer,
		ServerNotification::TaskStatusNotification(Notification::new(
			TaskStatusNotificationParams::new(task),
		)),
	);
	let payload = tap.next(&name, "notifications/tasks/status").await;
	assert_eq!(
		payload.params.get("taskId").and_then(|t| t.as_str()),
		Some("task-9")
	);
}

#[serial_test::serial]
#[tokio::test]
async fn custom_notification_keeps_its_method_and_params() {
	let name = unique_server("custom");
	let peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	push_notification(
		&peer,
		ServerNotification::CustomNotification(CustomNotification::new(
			"notifications/vendor-thing",
			Some(serde_json::json!({"hello": "world"})),
		)),
	);
	let payload = tap.next(&name, "notifications/vendor-thing").await;
	assert_eq!(
		payload.params.get("hello").and_then(|h| h.as_str()),
		Some("world")
	);
}

#[serial_test::serial]
#[tokio::test]
async fn elicitation_requests_are_surfaced_and_declined() {
	let name = unique_server("elicit");
	let mut peer = serve_in_memory(&name);
	let mut tap = NotificationTap::install();

	let params = ElicitRequestParams::UrlElicitationParams {
		meta: None,
		message: "Sign in at this URL".to_string(),
		url: "https://example.com/oauth".to_string(),
		elicitation_id: "elicit-1".to_string(),
	};
	peer.incoming
		.unbounded_send(JsonRpcMessage::request(
			ServerRequest::ElicitRequest(Request::new(params)),
			RequestId::Number(77),
		))
		.expect("fake server channel must stay open");

	let payload = tap.next(&name, "elicitation/requested").await;
	assert_eq!(
		payload.params.get("message").and_then(|m| m.as_str()),
		Some("Sign in at this URL")
	);

	let response = tokio::time::timeout(WAIT, peer.outgoing.recv())
		.await
		.expect("client must answer the elicitation request")
		.expect("fake server channel must stay open");
	match response {
		JsonRpcMessage::Response(resp) => {
			assert_eq!(resp.id, RequestId::Number(77));
			match resp.result {
				ClientResult::ElicitResult(result) => {
					assert_eq!(result.action, ElicitationAction::Decline);
				}
				other => panic!("expected ElicitResult, got {other:?}"),
			}
		}
		other => panic!("expected a response, got {other:?}"),
	}
}

#[serial_test::serial]
#[tokio::test]
async fn registry_tracks_disconnects_per_server() {
	let alpha = unique_server("registry-alpha");
	let beta = unique_server("registry-beta");
	assert!(
		!is_connected(&alpha),
		"unknown server must not be connected"
	);

	let peer_a = serve_in_memory(&alpha);
	let peer_b = serve_in_memory(&beta);
	let _registered_a = register(&alpha, peer_a.service);
	let _registered_b = register(&beta, peer_b.service);

	assert!(is_connected(&alpha));
	assert!(is_connected(&beta));
	let names = connected_names();
	assert!(names.contains(&alpha) && names.contains(&beta));

	disconnect(&alpha);
	assert!(get(&alpha).is_none());
	assert!(
		!is_connected(&alpha),
		"disconnect must remove the connection"
	);
	assert!(is_connected(&beta), "sibling connection must survive");

	disconnect_all();
	assert!(!connected_names().contains(&alpha));
	assert!(!connected_names().contains(&beta));
}

fn stdin_config(name: &str) -> McpServerConfig {
	McpServerConfig::Stdin {
		name: name.to_string(),
		// Never spawned: the in-memory service registered under `name` is
		// reused by `get_or_connect` before the command matters.
		command: "unused-in-memory".to_string(),
		args: vec![],
		timeout_seconds: 30,
		tools: vec![],
		env: HashMap::new(),
		cwd: None,
		auto_bind: None,
	}
}

fn tool_call(tool_id: &str) -> McpToolCall {
	McpToolCall {
		tool_name: "echo".to_string(),
		parameters: serde_json::json!({"x": 1}),
		tool_id: tool_id.to_string(),
	}
}

#[serial_test::serial]
#[tokio::test]
async fn call_tool_completes_over_registered_service() {
	let name = unique_server("call-complete");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name);

	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				if matches!(request.request, ClientRequest::CallToolRequest(_)) {
					incoming
						.unbounded_send(JsonRpcMessage::response(
							ServerResult::CallToolResult(CallToolResult::success(vec![])),
							request.id,
						))
						.expect("fake server channel must stay open");
				}
			}
		}
	});

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("call-c1"), None))
		.await
		.expect("tool call must not hang")
		.expect("tool call must succeed");
	assert_eq!(result.is_error, Some(false));

	responder.abort();
	disconnect(&name);
}

#[serial_test::serial]
#[tokio::test]
async fn call_tool_retries_after_state_only_input_required() {
	let name = unique_server("call-input-required");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name);

	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let seen_states = StdArc::new(std::sync::Mutex::new(Vec::new()));
	let captured = seen_states.clone();
	let responder = tokio::spawn(async move {
		let mut rounds = 0;
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				if let ClientRequest::CallToolRequest(call) = request.request {
					rounds += 1;
					captured
						.lock()
						.expect("state lock")
						.push(call.params.request_state.clone());
					let response = if rounds == 1 {
						ServerResult::InputRequiredResult(InputRequiredResult::from_request_state(
							"opaque-state-1",
						))
					} else {
						ServerResult::CallToolResult(CallToolResult::success(vec![]))
					};
					incoming
						.unbounded_send(JsonRpcMessage::response(response, request.id))
						.expect("fake server channel must stay open");
				}
			}
		}
	});

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("call-ir"), None))
		.await
		.expect("input-required retry must not hang")
		.expect("retry round must succeed");
	assert_eq!(result.is_error, Some(false));

	// The retry echoes the opaque request state back to the server.
	assert_eq!(
		*seen_states.lock().expect("state lock"),
		vec![None, Some("opaque-state-1".to_string())]
	);

	responder.abort();
	disconnect(&name);
}

#[serial_test::serial]
#[tokio::test]
async fn call_tool_polls_async_task_to_completion() {
	let name = unique_server("call-task");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name);

	let mut outgoing = peer.outgoing;
	let incoming = peer.incoming;
	let responder = tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				let response = match request.request {
					ClientRequest::CallToolRequest(_) => {
						let task = Task::new(
							"task-t1",
							TaskStatus::Working,
							"2026-01-01T00:00:00Z",
							"2026-01-01T00:00:00Z",
						)
						.with_poll_interval_ms(50);
						ServerResult::CreateTaskResult(CreateTaskResult::new(task))
					}
					ClientRequest::GetTaskRequest(_) => {
						let completed = serde_json::to_value(CallToolResult::success(vec![]))
							.expect("serialize completed result");
						let task = DetailedTask::new(
							Task::new(
								"task-t1",
								TaskStatus::Completed,
								"2026-01-01T00:00:00Z",
								"2026-01-01T00:00:01Z",
							),
							TaskPayload::Completed {
								result: completed.as_object().expect("object").clone(),
							},
						);
						ServerResult::GetTaskResult(GetTaskResult::new(task))
					}
					_ => continue,
				};
				incoming
					.unbounded_send(JsonRpcMessage::response(response, request.id))
					.expect("fake server channel must stay open");
			}
		}
	});

	let result = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("call-task"), None))
		.await
		.expect("task polling must not hang")
		.expect("completed task must resolve");
	assert_eq!(result.is_error, Some(false));

	responder.abort();
	disconnect(&name);
}

#[serial_test::serial]
#[tokio::test]
async fn cancelled_tool_call_notifies_the_server() {
	let name = unique_server("call-cancel");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name);

	let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
	let mut outgoing = peer.outgoing;
	let saw_cancel = StdArc::new(AtomicBool::new(false));
	let flag = saw_cancel.clone();
	// Never answers tools/call, so only the cancellation branch can resolve
	// the round; records the notifications/cancelled the client sends.
	tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Notification(JsonRpcNotification {
				notification: ClientNotification::CancelledNotification(_),
				..
			}) = message
			{
				flag.store(true, Ordering::SeqCst);
			}
		}
	});

	cancel_tx.send(true).expect("cancel send must succeed");
	let error = tokio::time::timeout(
		WAIT,
		call_tool(&server, &tool_call("call-cx"), Some(cancel_rx)),
	)
	.await
	.expect("cancelled call must not hang")
	.expect_err("cancelled call must fail");
	assert!(
		crate::session::cancellation::is_cancelled(&error),
		"unexpected error: {error}"
	);

	let notified = tokio::time::timeout(WAIT, async {
		while !saw_cancel.load(Ordering::SeqCst) {
			tokio::time::sleep(Duration::from_millis(5)).await;
		}
	})
	.await;
	assert!(notified.is_ok(), "client must send notifications/cancelled");

	disconnect(&name);
}
