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

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

/// ACP children are local protocol servers and must answer each startup
/// request promptly. Without a deadline, a silent or malformed child can keep
/// a tap run or layer waiting forever while its stdout stays open.
const ACP_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `clientInfo.name` the ACP `initialize` of a delegated child carries. The child cannot tell an
/// agent client from an IDE client otherwise, and it matches on this to learn it was delegated to.
pub const AGENT_CLIENT_NAME: &str = "octomind-agent-tool";

/// System-prompt section for a child started by another agent. Its whole reply lands in the
/// parent's context, so it must answer as a tool result, not as a chat with a person.
pub const DELEGATED_RUN_INSTRUCTION: &str = "<delegated_run>\nYou were started by another agent, not by a person, and your reply is consumed by that agent as a tool result. No greeting, no questions back: the caller cannot answer mid-run; when something required is missing, state exactly what is missing and stop. Lead with the outcome, then the evidence as `path:line` references and exact commands, then what you did not do. Omit narration of your process and anything the caller can open by path. Report failures as failures, never as partial success.\n</delegated_run>";

/// Write one newline-terminated JSON-RPC request to the ACP child. A broken
/// pipe means the child already exited — not fatal here: the read side
/// consumes whatever the child emitted before dying and reports the
/// definitive outcome (its buffered response, or "Subprocess closed before
/// response" on EOF). Surfacing the raw EPIPE instead would race the child's
/// exit against our write.
async fn write_acp_request(stdin: &mut tokio::process::ChildStdin, msg: Value) -> Result<()> {
	match stdin.write_all(format!("{}\n", msg).as_bytes()).await {
		Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
		_ => Ok(()),
	}
}

/// Spawn the ACP command, drive initialize → session/new → session/prompt.
/// Used by both taps and layers to execute via ACP protocol.
///
/// `program` is the executable path; `args` are CLI arguments passed verbatim.
/// Callers that have a single "program plus space-separated args" string should
/// split it themselves (e.g. via `split_whitespace`) before calling.
///
/// `tap_input`, when set, mirrors streamed updates (tool calls, usage) into
/// the tap-run live registry so `/status agents` can show them while the run works.
///
/// `handback` marks this run as a SUBAGENT HANDOFF whose verification verdict
/// the parent folds into its own round (see [`crate::acp::handback`]).
/// Layers pass `false`: they post-process the parent's answer rather than doing
/// delegated work, so their verdict says nothing about the parent's tree.
pub async fn run_acp_command(
	program: &str,
	args: &[&str],
	task: &str,
	workdir: &std::path::Path,
	mut cancel_rx: watch::Receiver<bool>,
	mut tap_input: Option<crate::session::tap_runs::TapRunInput<'_>>,
	handback: bool,
) -> Result<String> {
	let tap_run_id = tap_input.as_ref().map(|input| input.id);
	let websocket_tap = tap_run_id.is_some() && crate::session::tap_runs::with_run_events(|_, _| Ok(()))?.is_some();
	// Reported on drop so every exit path — success, prompt error, cancellation,
	// a child that dies mid-stream — lands in the parent's tally exactly once.
	// A run that never reports stays `false`: unverified is the safe default.
	struct Handback {
		verified: bool,
	}
	impl Drop for Handback {
		fn drop(&mut self) {
			crate::acp::handback::note_handback(self.verified);
		}
	}
	let mut handback_guard = handback.then(|| Handback { verified: false });

	// Plugins of this session hear about the child before it exists and may hand
	// it a value; the child's id is known only after `session/new`.
	let plugin_parent = crate::session::context::current_session_id().filter(|id| crate::plugins::active(id));
	let plugin_inherited = match &plugin_parent {
		Some(parent) => crate::plugins::lifecycle::subagent_starting(parent, None, task, None).await,
		None => Vec::new(),
	};
	let mut child_session_id: Option<String> = None;
	// A block, so every way out of the run reaches `subagent.end` below.
	let result: Result<String> = async {

	let mut command = Command::new(program);
	command
		.args(args)
		.current_dir(workdir)
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::null())
		// Every error path below must own the subprocess lifetime. Without this,
		// a handshake error/cancellation drops `Child` but leaves octomind running.
		.kill_on_drop(true);
	// Give the ACP child its own process group so Unix cancellation and timeout
	// also terminate helper processes spawned by wrapper scripts.
	#[cfg(unix)]
	command.process_group(0);
	#[cfg(windows)]
	command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
	let mut child = command.spawn()?;

	let mut stdin = Some(child.stdin.take().ok_or_else(|| anyhow::anyhow!("No stdin"))?);
	let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("No stdout"))?;
	let mut lines = BufReader::new(stdout).lines();

	// 1. initialize
	// stdin was captured above and is not taken until the prompt response; all three handshake writes have it.
	write_acp_request(
		stdin.as_mut().expect("child stdin is open during initialize"),
		json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": "initialize",
			"params": acp_initialize_params()
		}),
	)
	.await?;
	if let Err(error) = wait_for_response(&mut lines, 1, &mut cancel_rx, ACP_HANDSHAKE_TIMEOUT).await {
		terminate_acp_child(&mut child).await;
		return Err(error);
	}

	let mut new_session_params = acp_new_session_params(workdir);
	if let Some(parent) = &plugin_parent {
		crate::plugins::lifecycle::write_child_meta(&mut new_session_params, parent, &plugin_inherited);
	}
	if let Some(grant) = tap_input.as_ref().and_then(|input| input.context_grant) {
		new_session_params["_meta"][crate::session::delegation_context::ACP_META] = json!(grant.to_string());
	}
	// 2. session/new
	write_acp_request(
		stdin.as_mut().expect("child stdin is open during session creation"),
		json!({
			"jsonrpc": "2.0",
			"id": 2,
			"method": "session/new",
			"params": new_session_params
		}),
	)
	.await?;

	let session_resp = match wait_for_response(&mut lines, 2, &mut cancel_rx, ACP_HANDSHAKE_TIMEOUT).await {
		Ok(response) => response,
		Err(error) => {
			terminate_acp_child(&mut child).await;
			return Err(error);
		}
	};
	let Some(session_id) = session_resp
		.get("result")
		.and_then(|r| r.get("sessionId"))
		.and_then(|s| s.as_str())
	else {
		terminate_acp_child(&mut child).await;
		return Err(anyhow::anyhow!("No sessionId in session/new response"));
	};
	let session_id = session_id.to_string();
	child_session_id = Some(session_id.clone());

	// Tap input stays open for queued prompts and background work. Other callers
	// close stdin after the prompt and collect the child's remaining updates.
	write_acp_request(
		stdin.as_mut().expect("child stdin is open during prompt"),
		json!({
			"jsonrpc": "2.0",
			"id": 3,
			"method": "session/prompt",
			"params": acp_prompt_params(&session_id, task)
		}),
	)
	.await?;

	if let Some(run_id) = tap_run_id.filter(|_| websocket_tap) {
		if let Err(error) = crate::session::tap_runs::with_run_events(|events, parent| {
			events.emit(
				parent,
				run_id,
				crate::websocket::protocol::RunEvent::Prompt {
					content: task.to_string(),
				},
			)
		}) {
			crate::log_error!("Tap run {}: failed to publish prompt: {}", run_id, error);
		}
	}

	let mut output = String::new();
	// Captured prompt-response error: if the subprocess returns
	// `{"id":3,"error":{...}}` we want to surface it instead of silently
	// returning an empty string. Without this the parent sees `output: ""`
	// with status `done` even when the API call inside the subprocess failed.
	let mut prompt_error: Option<Value> = None;
	let mut prompt_response_received = false;
	let mut prompt_id = 3;
	let mut prompt_in_flight = true;
	let mut next_prompt: Option<String> = None;
	let mut pending_work = false;
	let mut shutdown_deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
	// Last cost the child reported, so repeated cumulative notifications bank
	// deltas. A tap run resumes the same child session (`--name <id>`), so its
	// reported total already covers earlier turns — resume from what we banked.
	let mut child_cost = tap_run_id
		.and_then(crate::session::tap_runs::find_job)
		.and_then(|j| j.live.usage)
		.map(|u| u.cost)
		.unwrap_or(0.0);

	loop {
		// Check for cancellation before each line read
		if *cancel_rx.borrow() {
			// Kill the child process on cancellation
			terminate_acp_child(&mut child).await;
			return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
		}
		if let Some(prompt) = next_prompt.take() {
			prompt_id += 1;
			write_acp_request(
				stdin.as_mut().expect("queued prompts keep child stdin open"),
				json!({"jsonrpc": "2.0", "id": prompt_id, "method": "session/prompt", "params": acp_prompt_params(&session_id, &prompt)}),
			).await?;
			prompt_in_flight = true;
			if !output.is_empty() && !output.ends_with("\n\n") {
				output.push_str("\n\n");
			}
			if let Some(run_id) = tap_run_id.filter(|_| websocket_tap) {
				crate::session::tap_runs::with_run_events(|events, parent| {
					events.emit(
						parent,
						run_id,
						crate::websocket::protocol::RunEvent::Prompt { content: prompt },
					)
				})?;
			}
		}

		// Use tokio::select to handle both cancellation and line reading
		let line = tokio::select! {
			prompt = async { tap_input.as_mut().expect("tap input is enabled").prompts.recv().await },
				if tap_input.is_some() && !prompt_in_flight && stdin.is_some() => {
				let Some(prompt) = prompt else {
					terminate_acp_child(&mut child).await;
					return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
				};
				next_prompt = Some(prompt);
				continue;
			}
			line = lines.next_line() => {
				match line? {
					Some(l) => l,
					None => break,
				}
			}
			_ = cancel_rx.changed() => {
				// Cancellation received - kill child and return
				terminate_acp_child(&mut child).await;
				return Err(anyhow::Error::new(
					crate::session::cancellation::Cancelled,
				));
			}
			_ = async {
				match shutdown_deadline.as_mut() {
					Some(deadline) => deadline.as_mut().await,
					None => std::future::pending::<()>().await,
				}
			} => {
				// A child that declared no finite work should close promptly after
				// stdin EOF. Preserve the existing wedged-child safety guard.
				terminate_acp_child(&mut child).await;
				break;
			}
		};

		if line.trim().is_empty() {
			continue;
		}
		let msg: Value = match serde_json::from_str(&line) {
			Ok(v) => v,
			Err(_) => continue,
		};

		if let Some(value) = msg
			.pointer("/params/_meta/octomind.pending_work")
			.or_else(|| msg.pointer("/result/_meta/octomind.pending_work"))
			.and_then(Value::as_bool)
		{
			pending_work = value;
		}

		// The child's end-of-turn verification verdict rides in `_meta` next to
		// usage (see acp/agent.rs) — the last thing it sends before the prompt
		// response, so it is always in hand by the time this loop breaks.
		if let (Some(g), Some(v)) = (
			handback_guard.as_mut(),
			msg.pointer("/params/_meta/octomind.verified").and_then(|x| x.as_bool()),
		) {
			g.verified = v;
		}

		// The child reports its own running session cost in `_meta` (which already
		// includes anything IT delegated), so bank only the increment. Applies to
		// every child — tap run or layer — otherwise their spend never
		// reaches the parent's total.
		if let Some(cost) = msg
			.pointer("/params/_meta/octomind.usage/session_cost")
			.and_then(|v| v.as_f64())
		{
			crate::session::external_spend::record(cost - child_cost);
			child_cost = cost;
		}

		// WebSocket tap runs have their own stream, independent of the parent's turn.
		// Other subprocesses and transports retain the parent notification sink.
		if msg.get("method").and_then(|m| m.as_str()) == Some("session/update") {
			route_child_update(&msg, tap_run_id, websocket_tap);
			if let Some(update) = msg.pointer("/params/update") {
				let update_kind = update.get("sessionUpdate").and_then(|u| u.as_str());
				if update_kind == Some("user_message_chunk")
					&& prompt_response_received
					&& !output.is_empty()
					&& !output.ends_with("\n\n")
				{
					output.push_str("\n\n");
				}
				if update_kind == Some("agent_message_chunk") {
					if let Some(text) = update.pointer("/content/text").and_then(|t| t.as_str()) {
						output.push_str(text);
					}
				}
			}
		}

		if msg.get("id").and_then(|i| i.as_u64()) == Some(prompt_id) {
			prompt_response_received = true;
			prompt_in_flight = false;
			if let Some(err) = msg.get("error") {
				prompt_error = Some(err.clone());
			}
		}
		if !prompt_in_flight && stdin.is_some() {
			if let Some(input) = tap_input.as_mut() {
				if pending_work {
					continue;
				}
				next_prompt = crate::session::tap_runs::take_prompt_or_close(input)?;
			}
			if next_prompt.is_none() {
				drop(stdin.take());
				if !pending_work {
					shutdown_deadline = Some(Box::pin(tokio::time::sleep(std::time::Duration::from_secs(5))));
				}
			}
		}
	}

	// Ensure stdin is closed on pre-response EOF/error too. A clean post-response
	// stdout EOF now means the child reached session idle.
	drop(stdin.take());
	if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
		.await
		.is_err()
	{
		terminate_acp_child(&mut child).await;
	}

	if prompt_in_flight {
		let partial = output.trim();
		if partial.is_empty() {
			return Err(anyhow::anyhow!(
				"ACP subprocess closed before the session/prompt response"
			));
		}
		return Err(anyhow::anyhow!(
			"ACP subprocess closed before the session/prompt response\n\nPartial output:\n{partial}"
		));
	}

	if let Some(err) = prompt_error {
		let trimmed = output.trim();
		// `data` carries the real cause: the ACP server builds prompt failures as
		// `Error::internal_error().data(e.to_string())`, so `message` is always the
		// fixed JSON-RPC text ("Internal error") and only `data` names what broke.
		let detail = err
			.get("data")
			.map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string()))
			.or_else(|| err.get("message").and_then(|m| m.as_str()).map(str::to_string))
			.unwrap_or_else(|| err.to_string());
		if trimmed.is_empty() {
			return Err(anyhow::anyhow!("ACP prompt failed: {detail}"));
		}
		return Err(anyhow::anyhow!(
			"ACP prompt failed: {detail}\n\nPartial output:\n{trimmed}"
		));
	}

	Ok(output.trim().to_string())
	}
	.await;
	if let Some(parent) = &plugin_parent {
		crate::plugins::lifecycle::subagent_ended(parent, child_session_id.as_deref()).await;
	}
	result
}

pub(crate) fn route_child_update(msg: &Value, tap_run_id: Option<&str>, websocket_tap: bool) {
	if let Some(run_id) = tap_run_id {
		record_tap_live(run_id, msg);
		if websocket_tap {
			let updates = match crate::websocket::run_events::acp_events(msg) {
				Ok(updates) => updates,
				Err(error) => {
					crate::log_error!("Tap run {}: skipping unmappable child update: {}", run_id, error);
					return;
				}
			};
			if let Err(error) = crate::session::tap_runs::with_run_events(|events, parent| {
				for event in updates {
					events.emit(parent, run_id, event)?;
				}
				Ok(())
			}) {
				crate::log_error!("Tap run {}: failed to publish child update: {}", run_id, error);
			}
			return;
		}
	}
	if let Some(update) = msg.pointer("/params/update") {
		forward_session_update_to_parent(update);
	}
}

/// Convert an ACP `session/update` notification into a `ServerMessage` and
/// push it through the parent's notification sender. Lets tap
/// and layer subprocess events render on the parent's output sink (CLI
/// stream, JSONL, websocket) instead of being silently dropped.
fn forward_session_update_to_parent(update: &Value) {
	let kind = match update.get("sessionUpdate").and_then(|u| u.as_str()) {
		Some(k) => k,
		None => return,
	};
	let session_id = crate::session::context::current_session_id().unwrap_or_else(|| String::from("acp"));

	let msg = match kind {
		"agent_message_chunk" => {
			let text = update.pointer("/content/text").and_then(|t| t.as_str()).unwrap_or("");
			if text.is_empty() {
				return;
			}
			crate::websocket::ServerMessage::Assistant(crate::websocket::AssistantPayload {
				content: text.to_string(),
				session_id,
				step: None,
			})
		}
		"agent_thought_chunk" => {
			let text = update.pointer("/content/text").and_then(|t| t.as_str()).unwrap_or("");
			if text.is_empty() {
				return;
			}
			crate::websocket::ServerMessage::Thinking(crate::websocket::ThinkingPayload {
				content: text.to_string(),
				session_id,
			})
		}
		"tool_call" => {
			let tool_id = update
				.get("toolCallId")
				.and_then(|s| s.as_str())
				.unwrap_or("")
				.to_string();
			let title = update.get("title").and_then(|s| s.as_str()).unwrap_or("").to_string();
			let raw_input = update.get("rawInput").cloned().unwrap_or(serde_json::Value::Null);
			crate::websocket::ServerMessage::ToolUse(crate::websocket::ToolUsePayload {
				tool: title,
				tool_id,
				server: String::new(),
				params: raw_input,
				session_id,
			})
		}
		"tool_call_update" => {
			let tool_id = update
				.get("toolCallId")
				.and_then(|s| s.as_str())
				.unwrap_or("")
				.to_string();
			let status = update.get("status").and_then(|s| s.as_str()).unwrap_or("");
			// Only surface terminal updates as ToolResult — intermediate
			// status flips would otherwise emit duplicate "result" rows.
			let success = match status {
				"completed" => true,
				"failed" => false,
				_ => return,
			};
			let raw_output = update
				.get("rawOutput")
				.map(|v| match v {
					Value::String(s) => s.clone(),
					other => other.to_string(),
				})
				.unwrap_or_default();
			crate::websocket::ServerMessage::ToolResult(crate::websocket::ToolResultPayload {
				tool: String::new(),
				tool_id,
				server: String::new(),
				content: raw_output,
				success,
				session_id,
			})
		}
		_ => return,
	};
	crate::mcp::process::send_notification_message(msg);
}

/// Mirror a subprocess `session/update` into the tap-run live registry so
/// `/status agents` shows what the run is doing right now — the on-disk snapshot
/// only flushes after each completed message, which lags long calls.
fn record_tap_live(run_id: &str, msg: &Value) {
	// Usage rides in `_meta` next to a SessionInfoUpdate (see acp/agent.rs).
	if let Some(usage) = msg.pointer("/params/_meta/octomind.usage") {
		let n = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
		crate::session::tap_runs::record_live_usage(
			run_id,
			crate::session::tap_runs::TapLiveUsage {
				input_tokens: n("input_tokens"),
				output_tokens: n("output_tokens"),
				cache_read_tokens: n("cache_read_tokens"),
				cost: usage.get("session_cost").and_then(|v| v.as_f64()).unwrap_or(0.0),
			},
		);
		return;
	}
	let Some(update) = msg.pointer("/params/update") else {
		return;
	};
	let action = match update.get("sessionUpdate").and_then(|u| u.as_str()) {
		Some("tool_call") => {
			let title = update.get("title").and_then(|s| s.as_str()).unwrap_or("");
			if title.is_empty() {
				return;
			}
			match update.get("rawInput").and_then(tool_arg_hint) {
				Some(hint) => Some(format!("{title} {hint}")),
				None => Some(title.to_string()),
			}
		}
		Some("agent_message_chunk") => update
			.pointer("/content/text")
			.and_then(|t| t.as_str())
			.map(str::trim)
			.filter(|t| !t.is_empty())
			.map(|t| truncate_action(t, 60)),
		_ => None,
	};
	if let Some(action) = action {
		crate::session::tap_runs::record_live_action(run_id, action);
	}
}

/// Most descriptive scalar argument of a tool call (path, command, query, …).
fn tool_arg_hint(args: &Value) -> Option<String> {
	for key in [
		"file_path",
		"path",
		"command",
		"pattern",
		"query",
		"url",
		"intent",
		"prompt",
		"name",
	] {
		if let Some(s) = args.get(key).and_then(|x| x.as_str()) {
			let s = s.trim();
			if !s.is_empty() {
				return Some(truncate_action(s, 48));
			}
		}
	}
	None
}

use crate::utils::text::truncate as truncate_action;

/// Outgoing ACP request params, kept as functions so tests can validate them
/// against the `agent-client-protocol` schema types the octomind ACP server
/// deserializes with.
fn acp_initialize_params() -> Value {
	json!({
		"protocolVersion": 1,
		"clientInfo": {"name": AGENT_CLIENT_NAME, "version": "1.0"}
	})
}

fn acp_new_session_params(workdir: &std::path::Path) -> Value {
	json!({"cwd": workdir.to_string_lossy(), "mcpServers": []})
}

fn acp_prompt_params(session_id: &str, task: &str) -> Value {
	json!({
		"sessionId": session_id,
		"prompt": [{"type": "text", "text": task}]
	})
}

/// Read lines until we find a JSON-RPC response with the given id, return it.
async fn wait_for_response<R>(
	lines: &mut tokio::io::Lines<R>,
	id: u64,
	cancel_rx: &mut watch::Receiver<bool>,
	timeout: std::time::Duration,
) -> Result<Value>
where
	R: tokio::io::AsyncBufRead + Unpin,
{
	let response = tokio::time::timeout(timeout, async {
		loop {
			if *cancel_rx.borrow() {
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}
			let line = tokio::select! {
				line = lines.next_line() => match line? {
					Some(line) => line,
					None => return Err(anyhow::anyhow!("Subprocess closed before response id={id}")),
				},
				changed = cancel_rx.changed() => {
					// A dropped sender has the same terminal meaning as an explicit
					// cancellation: nobody can resume ownership of this request.
					if changed.is_err() || *cancel_rx.borrow() {
						return Err(anyhow::Error::new(
							crate::session::cancellation::Cancelled,
						));
					}
					continue;
				}
			};
			if line.trim().is_empty() {
				continue;
			}
			let msg: Value = match serde_json::from_str(&line) {
				Ok(v) => v,
				Err(_) => continue,
			};
			if msg.get("id").and_then(|i| i.as_u64()) == Some(id) {
				if let Some(err) = msg.get("error") {
					// `data` carries the agent's own explanation (e.g. which keys a
					// agent is missing); `message` is only the JSON-RPC category.
					let detail = err
						.get("data")
						.and_then(Value::as_str)
						.or_else(|| err.get("message").and_then(Value::as_str));
					return Err(match detail {
						Some(detail) => anyhow::anyhow!("ACP error: {detail}"),
						None => anyhow::anyhow!("ACP error: {err}"),
					});
				}
				return Ok(msg);
			}
		}
	})
	.await;

	response.map_err(|_| {
		anyhow::anyhow!(
			"Timed out waiting for ACP response id={id} after {:.0}s",
			timeout.as_secs_f64()
		)
	})?
}

/// Terminate the ACP process group on Unix, or the direct child elsewhere,
/// and reap the direct child before returning.
async fn terminate_acp_child(child: &mut tokio::process::Child) {
	#[cfg(unix)]
	if let Some(pid) = child.id() {
		// The command is spawned with process_group(0), so its pid is also the
		// process-group id. Negative pid targets the complete group.
		unsafe {
			libc::kill(-(pid as i32), libc::SIGKILL);
		}
		let _ = child.wait().await;
		return;
	}

	let _ = child.kill().await;
	let _ = child.wait().await;
}

/// Lifecycle tests for `run_acp_command` — drive it against a fake ACP server
/// (a `sh` script emitting canned JSON-RPC lines) to pin the contract that the
/// ACP runner must never hang: collect streamed output, surface prompt
/// errors, and kill a child that fails to exit after the response (the bug
/// that left tap-runs in `running` forever).
#[cfg(all(test, unix))]
#[path = "driver_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "driver_updates_tests.rs"]
mod command_tests;

#[cfg(test)]
#[path = "driver_runtime_tests.rs"]
mod runtime_tests;
