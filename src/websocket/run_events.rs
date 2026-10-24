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

use super::protocol::{RunEvent, RunEventPayload, RunLogMessage, RunLogPayload, ServerMessage};
use super::server::WsSender;
use crate::session::tap_runs::{self, TapJobStatus};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use tokio_tungstenite::tungstenite::Message;

const MAX_RUN_CONTENT: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct EventBuffer {
	events: VecDeque<RunEvent>,
	bytes: usize,
	truncated: bool,
	/// Latest `Usage`, kept apart from `events` so truncation never drops it.
	usage: Option<RunEvent>,
}

impl EventBuffer {
	fn push(&mut self, event: RunEvent) {
		if matches!(event, RunEvent::Usage { .. }) {
			self.usage = Some(event.clone());
		}
		self.bytes += content_size(&event);
		match (self.events.back_mut(), &event) {
			(Some(RunEvent::Assistant { content }), RunEvent::Assistant { content: chunk })
			| (Some(RunEvent::Thinking { content }), RunEvent::Thinking { content: chunk }) => content.push_str(chunk),
			_ => self.events.push_back(event),
		}
		while self.bytes > MAX_RUN_CONTENT {
			// bytes is the sum of queued content sizes, so positive bytes imply a nonempty queue.
			self.bytes -= content_size(&self.events.pop_front().expect("buffer contains oversized content"));
			self.truncated = true;
		}
	}
}

fn content_size(event: &RunEvent) -> usize {
	match event {
		RunEvent::Prompt { content } | RunEvent::Assistant { content } | RunEvent::Thinking { content } => {
			content.len()
		}
		RunEvent::ToolUse { tool, tool_id, params } => tool.len() + tool_id.len() + params.to_string().len(),
		RunEvent::ToolResult { tool_id, content, .. } => tool_id.len() + content.len(),
		RunEvent::Usage { .. } => std::mem::size_of::<f64>() + 2 * std::mem::size_of::<u64>(),
		RunEvent::Status { status, error } => status.as_str().len() + error.as_ref().map_or(0, String::len),
	}
}

struct RunState {
	role: String,
	seq: u64,
	buffer: Option<EventBuffer>,
}

/// Owned by the session's tap registry. Its lock serializes emission and snapshots.
#[derive(Default)]
pub(crate) struct RunEvents {
	clients: Vec<WsSender>,
	runs: HashMap<String, RunState>,
}

impl RunEvents {
	pub(crate) fn bind(&mut self, sender: WsSender) {
		if !self.clients.iter().any(|client| client.same_channel(&sender)) {
			self.clients.push(sender);
		}
	}

	pub(crate) fn unbind(&mut self, sender: &WsSender) {
		self.clients.retain(|client| !client.same_channel(sender));
	}

	pub(crate) fn begin(&mut self, session_id: &str, run_id: &str, role: &str) -> Result<()> {
		anyhow::ensure!(!self.is_running(run_id), "run {run_id} is already streaming");
		let run = self.runs.entry(run_id.to_string()).or_insert_with(|| RunState {
			role: role.to_string(),
			seq: 0,
			buffer: None,
		});
		run.buffer = Some(EventBuffer::default());
		self.emit(
			session_id,
			run_id,
			RunEvent::Status {
				status: TapJobStatus::Running,
				error: None,
			},
		)
	}

	pub(crate) fn emit(&mut self, session_id: &str, run_id: &str, event: RunEvent) -> Result<()> {
		let run = self
			.runs
			.get_mut(run_id)
			.with_context(|| format!("run {run_id} has no event stream"))?;
		let buffer = run
			.buffer
			.as_mut()
			.with_context(|| format!("run {run_id} is not streaming"))?;
		let seq = run.seq.checked_add(1).context("run event sequence exhausted")?;
		let terminal = matches!(event, RunEvent::Status { status, .. } if status != TapJobStatus::Running);
		let payload = ServerMessage::RunEvent(RunEventPayload {
			session_id: session_id.to_string(),
			run_id: run_id.to_string(),
			role: run.role.clone(),
			seq,
			event: event.clone(),
		});
		let text = serde_json::to_string(&payload)?;
		buffer.push(event);
		run.seq = seq;
		self.clients
			.retain(|client| client.send(Message::text(text.clone())).is_ok());
		if terminal {
			run.buffer = None;
		}
		Ok(())
	}

	pub(crate) fn snapshot(&self, request: &RunLogMessage) -> Option<RunLogPayload> {
		let run = self.runs.get(&request.run_id)?;
		let buffer = run.buffer.as_ref()?;
		Some(RunLogPayload {
			session_id: request.session_id.clone(),
			run_id: request.run_id.clone(),
			role: Some(run.role.clone()),
			status: "running".to_string(),
			source: "live".to_string(),
			seq: run.seq,
			truncated: buffer.truncated && !request.summary,
			events: if request.summary {
				buffer.usage.iter().cloned().collect()
			} else {
				buffer.events.iter().cloned().collect()
			},
			error: None,
			request_id: request.request_id.clone(),
		})
	}

	pub(crate) fn is_running(&self, run_id: &str) -> bool {
		self.runs.get(run_id).is_some_and(|run| run.buffer.is_some())
	}

	pub(crate) fn last_seq(&self, run_id: &str) -> u64 {
		self.runs.get(run_id).map_or(0, |run| run.seq)
	}
}

pub(crate) async fn replay(request: RunLogMessage) -> Result<RunLogPayload> {
	tap_runs::validate_run_id(&request.run_id).map_err(anyhow::Error::msg)?;
	let snapshot = tap_runs::snapshot_run_log(&request)?;
	if snapshot.source == "live" {
		return Ok(snapshot);
	}
	let dir = crate::session::get_sessions_dir()?;
	tokio::task::spawn_blocking(move || replay_disk(&request, snapshot, || disk_events(&dir, &request.run_id))).await?
}

fn replay_disk(
	request: &RunLogMessage,
	mut snapshot: RunLogPayload,
	mut read: impl FnMut() -> Result<Option<Vec<RunEvent>>>,
) -> Result<RunLogPayload> {
	loop {
		let events = read();
		let current = tap_runs::snapshot_run_log(request)?;
		if current.source == "live" {
			return Ok(current);
		}
		if current.seq != snapshot.seq || current.status != snapshot.status {
			snapshot = current;
			continue;
		}
		snapshot = current;
		let Some(mut events) = events? else {
			anyhow::ensure!(snapshot.role.is_some(), "run not found: {}", request.run_id);
			return Ok(snapshot);
		};
		if request.summary {
			// disk_events appends the session totals last, when known.
			events = events
				.into_iter()
				.rfind(|event| matches!(event, RunEvent::Usage { .. }))
				.into_iter()
				.collect();
		}
		snapshot.events = events;
		return Ok(snapshot);
	}
}

pub(crate) fn disk_events(dir: &Path, run_id: &str) -> Result<Option<Vec<RunEvent>>> {
	tap_runs::validate_run_id(run_id).map_err(anyhow::Error::msg)?;
	let path = dir.join(format!("{run_id}.jsonl.zst"));
	if !path.try_exists()? {
		return Ok(None);
	}
	let session = crate::session::load_session(&path)?;
	let mut events = Vec::new();
	for message in session.messages {
		match message.role.as_str() {
			// Only the task the specialist was given (and any resume turn). The log also
			// holds system-managed user messages — AGENTS.md `<instructions>`, inbox
			// notes, continuations — which the live stream never shows as a prompt.
			"user" if crate::session::is_real_user_task_message(&message) => events.push(RunEvent::Prompt {
				content: message.content,
			}),
			"assistant" => {
				if let Some(thinking) = crate::session::message_thinking_content(&message) {
					events.push(RunEvent::Thinking {
						content: thinking.to_string(),
					});
				}
				if !message.content.is_empty() {
					events.push(RunEvent::Assistant {
						content: message.content,
					});
				}
				if let Some(calls) = message.tool_calls {
					// Same reader the provider path uses: the log stores octolib's unified
					// GenericToolCall JSON, older logs the OpenAI `function` shape.
					for call in crate::providers::convert_to_generic_tool_calls(&calls)
						.context("stored assistant tool_calls")?
					{
						events.push(RunEvent::ToolUse {
							tool: call.name,
							tool_id: call.id,
							params: call.arguments,
						});
					}
				}
			}
			"tool" => events.push(RunEvent::ToolResult {
				tool_id: message.tool_call_id.context("tool message tool_call_id missing")?,
				content: message.content,
				success: None,
			}),
			_ => {}
		}
	}
	// Same totals the live stream's `octomind.usage` carries: total_cost includes the
	// external spend (the run's own sub-delegations, supervisor) folded in before the
	// last STATS/SUMMARY write. All-zero totals mean nothing was recorded, and a $0
	// usage event would claim a cost that is not known.
	let info = &session.info;
	if info.total_cost > 0.0 || info.input_tokens > 0 || info.output_tokens > 0 {
		events.push(RunEvent::Usage {
			cost: info.total_cost,
			input_tokens: info.input_tokens,
			output_tokens: info.output_tokens,
		});
	}
	Ok(Some(events))
}

fn required_string(value: &Value, key: &str) -> Result<String> {
	Ok(value
		.get(key)
		.and_then(Value::as_str)
		.with_context(|| format!("{key}: string required"))?
		.to_string())
}

pub(crate) fn acp_events(msg: &Value) -> Result<Vec<RunEvent>> {
	let mut events = Vec::new();
	if let Some(usage) = msg.pointer("/params/_meta/octomind.usage") {
		events.push(RunEvent::Usage {
			cost: usage
				.get("session_cost")
				.and_then(Value::as_f64)
				.context("octomind.usage.session_cost missing")?,
			input_tokens: usage
				.get("input_tokens")
				.and_then(Value::as_u64)
				.context("octomind.usage.input_tokens missing")?,
			output_tokens: usage
				.get("output_tokens")
				.and_then(Value::as_u64)
				.context("octomind.usage.output_tokens missing")?,
		});
	}
	if let Some(update) = msg.pointer("/params/update") {
		let event = match update.get("sessionUpdate").and_then(Value::as_str) {
			Some(kind @ ("agent_message_chunk" | "agent_thought_chunk")) => {
				// ACP chunks may contain media rather than text.
				update
					.pointer("/content/text")
					.and_then(Value::as_str)
					.filter(|s| !s.is_empty())
					.map(|text| {
						let content = text.to_string();
						if kind == "agent_message_chunk" {
							RunEvent::Assistant { content }
						} else {
							RunEvent::Thinking { content }
						}
					})
			}
			Some("tool_call") => Some(RunEvent::ToolUse {
				tool: required_string(update, "title")?,
				tool_id: required_string(update, "toolCallId")?,
				// ACP allows absent rawInput for calls without exposed parameters.
				params: update.get("rawInput").cloned().unwrap_or(Value::Null),
			}),
			Some("tool_call_update") => match update.get("status").and_then(Value::as_str) {
				Some(status @ ("completed" | "failed")) => Some(RunEvent::ToolResult {
					tool_id: required_string(update, "toolCallId")?,
					// ACP permits a terminal status without rawOutput.
					content: match update.get("rawOutput") {
						Some(Value::String(text)) => text.clone(),
						Some(value) => value.to_string(),
						None => String::new(),
					},
					success: Some(status == "completed"),
				}),
				_ => None,
			},
			_ => None,
		};
		if let Some(event) = event {
			events.push(event);
		}
	}
	Ok(events)
}

#[cfg(test)]
#[path = "run_events_tests.rs"]
mod tests;
