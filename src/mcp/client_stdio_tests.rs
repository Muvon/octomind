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

//! Real-transport tests for the MCP client, complementing the in-memory peer
//! tests in `client_tests.rs`: stdio spawn/timeout/stderr paths against
//! short-lived local stub processes, HTTP failure mapping against a refused
//! loopback port (never an external endpoint), connection reuse in
//! `get_or_connect`, `list_tools`, and the `call_tool` timeout / MRTR / task
//! error arms the in-memory tests do not reach.
//!
//! Every test is `#[serial]`: the client registry and the CLI notification
//! sender are process globals.

use super::*;
use futures::channel::mpsc;
use futures::StreamExt;
use rmcp::model::{
	CallToolResult, ClientRequest, ContentBlock, CreateTaskResult, DetailedTask, GetTaskResult,
	InputRequiredResult, JsonRpcMessage, ListToolsResult, Resource, ServerResult, Task,
	TaskPayload, TaskStatus,
};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use serial_test::serial;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc as StdArc;

/// Upper bound for any single await — a hang fails fast instead of stalling
/// the whole test binary.
const WAIT: Duration = Duration::from_secs(2);

fn unique_server(tag: &str) -> String {
	format!("octomind-test-stdio-{tag}")
}

/// A client service served over an in-memory transport (same shape as the
/// fake peer in `client_tests.rs`; the helpers are module-private there).
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

fn stdin_config(
	name: &str,
	command: &str,
	args: Vec<String>,
	timeout_seconds: u64,
) -> McpServerConfig {
	McpServerConfig::Stdin {
		name: name.to_string(),
		// For registered in-memory services the command is never spawned.
		command: command.to_string(),
		args,
		timeout_seconds,
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

/// Answer every client request with `respond(request)`; `None` stays silent
/// (no response is ever sent for that request).
fn spawn_responder(
	mut outgoing: mpsc::UnboundedReceiver<TxJsonRpcMessage<RoleClient>>,
	incoming: mpsc::UnboundedSender<RxJsonRpcMessage<RoleClient>>,
	respond: impl Fn(ClientRequest) -> Option<ServerResult> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		while let Some(message) = outgoing.next().await {
			if let JsonRpcMessage::Request(request) = message {
				if let Some(response) = respond(request.request) {
					incoming
						.unbounded_send(JsonRpcMessage::response(response, request.id))
						.expect("fake server channel must stay open");
				}
			}
		}
	})
}

// ---------------------------------------------------------------------------
// connect_stdio — real child processes
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn connect_stdio_reports_spawn_failure_from_both_lifecycle_attempts() {
	let name = unique_server("spawn-fail");
	let server = stdin_config(&name, "octomind-test-no-such-binary", Vec::new(), 5);

	let error = connect_stdio(&server)
		.await
		.err()
		.expect("missing binary must fail both attempts");
	let msg = error.to_string();
	assert!(
		msg.contains("Failed to start MCP server"),
		"unexpected error: {msg}"
	);
	assert!(
		msg.contains("modern:") && msg.contains("legacy:"),
		"combined error must include both attempts: {msg}"
	);
	assert!(!is_connected(&name));
}

#[serial]
#[tokio::test]
async fn connect_stdio_times_out_against_silent_stub_and_caps_stderr() {
	let name = unique_server("timeout-stderr");
	// One blank stderr line (skipped by the drain), 60 numbered lines (the
	// diagnostic buffer caps at 50), then silence: the handshake can never
	// complete, so each lifecycle attempt runs into its 1s timeout. The
	// transport drop kills the `sleep` child.
	let server = stdin_config(
		&name,
		"sh",
		vec![
			"-c".to_string(),
			"printf '\\n' >&2; seq 1 60 >&2; exec sleep 30".to_string(),
		],
		1,
	);

	let start = std::time::Instant::now();
	let error = connect_stdio(&server)
		.await
		.err()
		.expect("silent stub must fail both attempts");
	assert!(
		start.elapsed() >= Duration::from_millis(1900),
		"both lifecycle attempts must run their timeout, took {:?}",
		start.elapsed()
	);
	let msg = error.to_string();
	assert!(
		msg.contains("Timed out establishing MCP connection"),
		"unexpected error: {msg}"
	);
	assert!(
		msg.contains("modern:") && msg.contains("legacy:"),
		"combined error must include both attempts: {msg}"
	);

	// The stderr drain task keeps only the last 50 lines per server.
	let buffer = crate::mcp::process::stderr_buffer_for(&name);
	let capped = tokio::time::timeout(WAIT, async {
		while buffer.lock().expect("stderr lock").len() != 50 {
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await;
	assert!(capped.is_ok(), "stderr buffer must cap at 50 lines");
	assert!(
		buffer
			.lock()
			.expect("stderr lock")
			.iter()
			.any(|line| line == "60"),
		"the newest lines must survive the cap"
	);

	assert!(!is_connected(&name));
	let _ = crate::mcp::process::cleanup_server_process(&name);
}

// ---------------------------------------------------------------------------
// connect_http — refused loopback endpoint, never an external one
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn connect_http_fails_fast_against_refused_loopback_endpoint() {
	let name = unique_server("http-refused");
	// Port 1 on loopback has no listener: connection refused, immediately.
	// The static Authorization header keeps OAuth discovery out of the path.
	let mut server = McpServerConfig::http(&name, "http://127.0.0.1:1/mcp", 2, Vec::new());
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("Authorization".to_string(), "Bearer static".to_string());
	}

	let error = connect_http(&server)
		.await
		.err()
		.expect("refused endpoint must fail");
	let msg = error.to_string();
	assert!(
		msg.contains("Failed to initialize MCP server"),
		"unexpected error: {msg}"
	);
	assert!(
		msg.contains("modern:") && msg.contains("legacy:"),
		"combined error must include both attempts: {msg}"
	);
	assert!(!is_connected(&name));
}

// ---------------------------------------------------------------------------
// get_or_connect — reuse and reconnect arms
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn get_or_connect_reuses_live_http_connection_without_reconnecting() {
	let name = unique_server("http-reuse");
	let peer = serve_in_memory(&name);
	let registered = register(&name, peer.service);

	let mut server = McpServerConfig::http(&name, "http://127.0.0.1:1/mcp", 2, Vec::new());
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("Authorization".to_string(), "Bearer static".to_string());
	}

	let service = get_or_connect(&server)
		.await
		.expect("live connection must be reused, not reconnected");
	assert!(
		Arc::ptr_eq(&service, &registered),
		"must return the registered connection"
	);

	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn get_or_connect_discards_closed_http_connection_and_reconnects() {
	let name = unique_server("http-stale");
	let peer = serve_in_memory(&name);
	let registered = register(&name, peer.service);
	registered.cancellation_token().cancel();
	assert!(
		!is_connected(&name),
		"cancelled service must read as closed"
	);

	let mut server = McpServerConfig::http(&name, "http://127.0.0.1:1/mcp", 2, Vec::new());
	if let McpServerConfig::Http { headers, .. } = &mut server {
		headers.insert("Authorization".to_string(), "Bearer static".to_string());
	}
	let error = get_or_connect(&server)
		.await
		.err()
		.expect("reconnect against a refused endpoint must fail");
	assert!(
		error
			.to_string()
			.contains("Failed to initialize MCP server"),
		"unexpected error: {error}"
	);
	assert!(get(&name).is_none(), "stale connection must be discarded");
}

#[serial]
#[tokio::test]
async fn get_or_connect_reuses_live_stdio_connection() {
	let name = unique_server("stdio-reuse");
	let peer = serve_in_memory(&name);
	let registered = register(&name, peer.service);

	// No pgid is registered for the name, so liveness is unknown and the
	// open service is trusted — the command is never spawned.
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 30);
	let service = get_or_connect(&server)
		.await
		.expect("live stdio connection must be reused");
	assert!(Arc::ptr_eq(&service, &registered));

	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn get_or_connect_starts_stdio_servers_through_the_process_manager() {
	let name = unique_server("stdio-start");
	let server = stdin_config(&name, "octomind-test-no-such-binary", Vec::new(), 5);

	let error = get_or_connect(&server)
		.await
		.err()
		.expect("spawn failure must propagate through ensure_server_running");
	assert!(
		error.to_string().contains("Failed to start server"),
		"unexpected error: {error}"
	);
}

// ---------------------------------------------------------------------------
// list_tools
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn list_tools_returns_the_advertised_tool_list() {
	let name = unique_server("list-tools");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 5);

	let responder = spawn_responder(outgoing, incoming, |request| match request {
		ClientRequest::ListToolsRequest(_) => Some(ServerResult::ListToolsResult(
			ListToolsResult::with_all_items(Vec::new()),
		)),
		_ => None,
	});
	let tools = list_tools(&server).await.expect("tools/list must succeed");
	assert!(tools.is_empty());

	responder.abort();
	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn list_tools_times_out_when_the_server_stays_silent() {
	let name = unique_server("list-timeout");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 1);

	let error = list_tools(&server)
		.await
		.expect_err("silent server must time out");
	assert!(
		error.to_string().contains("tools/list timed out"),
		"unexpected error: {error}"
	);

	disconnect(&name);
}

// ---------------------------------------------------------------------------
// call_tool — timeout, MRTR and response-shape error arms
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn call_tool_maps_idle_timeout_to_the_actionable_error() {
	let name = unique_server("idle-timeout");
	let peer = serve_in_memory(&name);
	register(&name, peer.service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 1);

	// No responder: the round never hears back, not even progress, so the
	// idle (not absolute) timeout fires.
	let error = tokio::time::timeout(WAIT, call_tool(&server, &tool_call("call-idle"), None))
		.await
		.expect("idle timeout must fire within the harness wait")
		.expect_err("silent server must fail the round");
	let msg = error.to_string();
	assert!(
		msg.contains("timed out after PT1S idle"),
		"unexpected error: {msg}"
	);
	assert!(
		msg.contains("check for side effects"),
		"unexpected error: {msg}"
	);

	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn call_tool_rejects_input_required_without_requests_or_state() {
	let name = unique_server("input-bare");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 5);

	let responder = spawn_responder(outgoing, incoming, |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::InputRequiredResult(
			InputRequiredResult::new(None, None),
		)),
		_ => None,
	});
	let error = call_tool(&server, &tool_call("call-bare"), None)
		.await
		.expect_err("bare input_required must fail");
	assert!(
		error
			.to_string()
			.contains("without inputRequests or requestState"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn call_tool_enforces_the_mrtr_round_limit_for_state_only_rounds() {
	let name = unique_server("mrtr-limit");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 5);

	// Every round answers state-only input_required: the client echoes the
	// state and backs off, until the round limit aborts the call.
	let responder = spawn_responder(outgoing, incoming, |request| match request {
		ClientRequest::CallToolRequest(_) => Some(ServerResult::InputRequiredResult(
			InputRequiredResult::from_request_state("opaque-forever"),
		)),
		_ => None,
	});
	let error = tokio::time::timeout(WAIT * 4, call_tool(&server, &tool_call("call-mrtr"), None))
		.await
		.expect("round limit must be reached within the harness wait")
		.expect_err("runaway input_required must fail");
	assert!(
		error
			.to_string()
			.contains("exceeded the MCP input-required round limit (10)"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn call_tool_rejects_unexpected_tools_call_response_type() {
	let name = unique_server("wrong-result");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 5);

	let responder = spawn_responder(outgoing, incoming, |request| match request {
		ClientRequest::CallToolRequest(_) => {
			Some(ServerResult::ListToolsResult(ListToolsResult::default()))
		}
		_ => None,
	});
	let error = call_tool(&server, &tool_call("call-wrong"), None)
		.await
		.expect_err("wrong response shape must fail");
	assert!(
		error
			.to_string()
			.contains("Unexpected response type for tools/call"),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

// ---------------------------------------------------------------------------
// drive_task — terminal task payloads and upfront cancellation
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn drive_task_surfaces_failed_and_cancelled_task_payloads() {
	let name = unique_server("task-endings");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 5);

	let rounds = StdArc::new(AtomicUsize::new(0));
	let counter = rounds.clone();
	let responder = spawn_responder(outgoing, incoming, move |request| match request {
		ClientRequest::CallToolRequest(_) => {
			let task = Task::new(
				"task-f1",
				TaskStatus::Working,
				"2026-01-01T00:00:00Z",
				"2026-01-01T00:00:00Z",
			)
			.with_poll_interval_ms(50);
			Some(ServerResult::CreateTaskResult(CreateTaskResult::new(task)))
		}
		ClientRequest::GetTaskRequest(_) => {
			let round = counter.fetch_add(1, Ordering::SeqCst);
			let (status, payload) = if round == 0 {
				(
					TaskStatus::Failed,
					TaskPayload::Failed {
						error: serde_json::json!({"code": 13, "message": "boom"})
							.as_object()
							.expect("error object")
							.clone(),
					},
				)
			} else {
				(TaskStatus::Cancelled, TaskPayload::Cancelled)
			};
			let task = DetailedTask::new(
				Task::new(
					"task-f1",
					status,
					"2026-01-01T00:00:00Z",
					"2026-01-01T00:00:01Z",
				),
				payload,
			);
			Some(ServerResult::GetTaskResult(GetTaskResult::new(task)))
		}
		_ => None,
	});

	let failed = call_tool(&server, &tool_call("call-fail"), None)
		.await
		.expect_err("failed task must error");
	assert!(
		failed.to_string().contains("MCP task 'task-f1' failed"),
		"unexpected error: {failed}"
	);

	let cancelled = call_tool(&server, &tool_call("call-task-cancel"), None)
		.await
		.expect_err("cancelled task must error");
	assert!(
		crate::session::cancellation::is_cancelled(&cancelled),
		"unexpected error: {cancelled}"
	);

	responder.abort();
	disconnect(&name);
}

#[serial]
#[tokio::test]
async fn drive_task_cancels_the_server_task_when_the_call_is_cancelled() {
	let name = unique_server("task-cancel-upfront");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	// 1s keeps the cooperative tasks/cancel wait short when the responder
	// stays silent for it.
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 1);

	let responder = spawn_responder(outgoing, incoming, |request| match request {
		ClientRequest::CallToolRequest(_) => {
			let task = Task::new(
				"task-c1",
				TaskStatus::Working,
				"2026-01-01T00:00:00Z",
				"2026-01-01T00:00:00Z",
			)
			.with_poll_interval_ms(50);
			Some(ServerResult::CreateTaskResult(CreateTaskResult::new(task)))
		}
		_ => None,
	});

	let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
	cancel_tx.send(true).expect("cancel send must succeed");
	let error = tokio::time::timeout(
		WAIT * 4,
		call_tool(&server, &tool_call("call-ct"), Some(cancel_rx)),
	)
	.await
	.expect("upfront cancellation must not hang")
	.expect_err("cancelled task call must fail");
	assert!(
		crate::session::cancellation::is_cancelled(&error),
		"unexpected error: {error}"
	);

	responder.abort();
	disconnect(&name);
}

// ---------------------------------------------------------------------------
// watch_resource_links — no-session early return
// ---------------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn completed_result_with_resource_links_is_returned_outside_sessions() {
	let name = unique_server("resource-link");
	let InMemoryPeer {
		service,
		incoming,
		outgoing,
	} = serve_in_memory(&name);
	register(&name, service);
	let server = stdin_config(&name, "unused-in-memory", Vec::new(), 5);

	let responder = spawn_responder(outgoing, incoming, |request| match request {
		ClientRequest::CallToolRequest(_) => {
			Some(ServerResult::CallToolResult(CallToolResult::success(vec![
				ContentBlock::resource_link(Resource::new(
					"octofs://jobs/link-1",
					"shell: make test",
				)),
			])))
		}
		_ => None,
	});
	let result = call_tool(&server, &tool_call("call-link"), None)
		.await
		.expect("linked result must complete normally");
	assert_eq!(result.content.len(), 1);
	// Outside a session no watcher can be established — the link is noted
	// nowhere and the result is still returned.

	responder.abort();
	disconnect(&name);
}
