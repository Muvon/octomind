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
}

impl EventBuffer {
	fn push(&mut self, event: RunEvent) {
		self.bytes += content_size(&event);
		match (self.events.back_mut(), &event) {
			(Some(RunEvent::Assistant { content }), RunEvent::Assistant { content: chunk })
			| (Some(RunEvent::Thinking { content }), RunEvent::Thinking { content: chunk }) => content.push_str(chunk),
			_ => self.events.push_back(event),
		}
		while self.bytes > MAX_RUN_CONTENT {
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

	pub(crate) fn begin(&mut self, session_id: &str, run_id: &str, role: &str) {
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
		);
	}

	pub(crate) fn emit(&mut self, session_id: &str, run_id: &str, event: RunEvent) {
		let run = self.runs.get_mut(run_id).expect("tap run event stream started");
		run.seq += 1;
		let terminal = matches!(event, RunEvent::Status { status, .. } if status != TapJobStatus::Running);
		run.buffer.as_mut().expect("tap run is emitting").push(event.clone());
		let payload = ServerMessage::RunEvent(RunEventPayload {
			session_id: session_id.to_string(),
			run_id: run_id.to_string(),
			role: run.role.clone(),
			seq: run.seq,
			event,
		});
		let text = serde_json::to_string(&payload).expect("run event is serializable");
		self.clients
			.retain(|client| client.send(Message::text(text.clone())).is_ok());
		if terminal {
			run.buffer = None;
		}
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
			truncated: buffer.truncated,
			events: buffer.events.iter().cloned().collect(),
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
	let mut snapshot = tap_runs::snapshot_run_log(&request);
	if snapshot.source == "live" {
		return Ok(snapshot);
	}
	let dir = crate::session::get_sessions_dir()?;
	tokio::task::spawn_blocking(move || {
		snapshot.events = disk_events(&dir, &request.run_id)?;
		Ok(snapshot)
	})
	.await?
}

pub(crate) fn disk_events(dir: &Path, run_id: &str) -> Result<Vec<RunEvent>> {
	tap_runs::validate_run_id(run_id).map_err(anyhow::Error::msg)?;
	let path = dir.join(format!("{run_id}.jsonl.zst"));
	if !path.try_exists()? {
		anyhow::bail!("run not found: {run_id}");
	}
	let session = crate::session::load_session(&path)?;
	let mut events = Vec::new();
	for message in session.messages {
		match message.role.as_str() {
			"user" => events.push(RunEvent::Prompt {
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
					for call in calls.as_array().context("assistant tool_calls must be an array")? {
						let function = call.get("function").context("tool call function missing")?;
						let arguments = function.get("arguments").context("tool call arguments missing")?;
						let params = match arguments {
							Value::String(text) => serde_json::from_str(text).context("invalid tool call arguments")?,
							Value::Object(_) => arguments.clone(),
							_ => anyhow::bail!("tool call arguments must be an object or JSON string"),
						};
						events.push(RunEvent::ToolUse {
							tool: required_string(function, "name")?,
							tool_id: required_string(call, "id")?,
							params,
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
	Ok(events)
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
