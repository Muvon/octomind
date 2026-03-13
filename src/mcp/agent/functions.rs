// Copyright 2025 Muvon Un Limited
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

// Agent functions - spawns ACP subprocess and drives the protocol to completion.

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::background_jobs::{BackgroundJobManager, CompletedJob};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Global singleton — created once when the first background agent call arrives.
static JOB_MANAGER: OnceLock<Arc<BackgroundJobManager>> = OnceLock::new();

/// Register (or return) the global BackgroundJobManager.
/// Called by the session on startup so the channel receiver is wired before any agent runs.
pub fn init_job_manager(max_concurrent: usize) -> tokio::sync::mpsc::Receiver<CompletedJob> {
	let (mgr, rx) = BackgroundJobManager::new(max_concurrent);
	// If already initialised (e.g. two sessions in same process) just return a dummy receiver.
	if JOB_MANAGER.set(Arc::new(mgr)).is_err() {
		let (_tx, rx2) = tokio::sync::mpsc::channel(1);
		return rx2;
	}
	rx
}

fn get_job_manager() -> Option<Arc<BackgroundJobManager>> {
	JOB_MANAGER.get().cloned()
}

/// Get all available agent functions based on config.
///
/// Each agent becomes a separate MCP tool (e.g., `agent_context_gatherer`).
pub fn get_all_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	config
		.agents
		.iter()
		.map(|agent_config| McpFunction {
			name: format!("agent_{}", agent_config.name),
			description: format!(
				"{}\n\nSet `background: true` to run asynchronously — the result will be injected into the conversation automatically when ready.",
				agent_config.description
			),
			parameters: json!({
				"type": "object",
				"properties": {
					"task": {
						"type": "string",
						"description": "Task description in human language for the agent to process"
					},
					"background": {
						"type": "boolean",
						"description": "Run in background and return immediately. Result is pushed back into the session automatically. Default: false.",
						"default": false
					}
				},
				"required": ["task"]
			}),
		})
		.collect()
}

/// Execute an agent tool call by spawning the configured ACP command as a subprocess
pub async fn execute_agent_command(
	call: &McpToolCall,
	config: &crate::config::Config,
	_cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	let agent_name = match call.tool_name.strip_prefix("agent_") {
		Some(name) => name,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Invalid agent tool name: {}", call.tool_name),
			));
		}
	};

	let task = match call.parameters.get("task").and_then(|v| v.as_str()) {
		Some(t) if !t.trim().is_empty() => t,
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Agent tool requires a non-empty 'task' parameter".to_string(),
			));
		}
	};

	let agent_config = match config.agents.iter().find(|a| a.name == agent_name) {
		Some(c) => c,
		None => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Agent '{agent_name}' not configured"),
			));
		}
	};

	let background = call
		.parameters
		.get("background")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	let session_workdir = crate::mcp::get_thread_working_directory();
	let workdir = agent_config.get_resolved_workdir(&session_workdir);

	if background {
		let manager = match get_job_manager() {
			Some(m) => m,
			None => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Background job manager not initialised (no active session)".to_string(),
				));
			}
		};

		if let Err(active) = manager.try_acquire() {
			let max = config.background_jobs.max_concurrent_jobs;
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Background job limit reached ({active}/{max} active). Wait for existing jobs to complete."),
			));
		}

		let mgr = Arc::clone(&manager);
		let command = agent_config.command.clone();
		let agent_name_owned = agent_config.name.clone();
		let task_owned = task.to_string();

		tokio::spawn(async move {
			let output = match run_acp_command(&command, &task_owned, &workdir).await {
				Ok(text) => text,
				Err(e) => format!("ERROR: {e:#}"),
			};
			mgr.release(CompletedJob {
				agent_name: agent_name_owned,
				output,
			});
		});

		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"Agent task started in background. The result will be injected into this conversation automatically when ready.".to_string(),
		));
	}

	// Synchronous path (default)
	match run_acp_command(&agent_config.command, task, &workdir).await {
		Ok(output) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			output,
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent failed: {e}"),
		)),
	}
}
/// Spawn the ACP command, drive initialize → session/new → session/prompt,
/// collect all agent_message_chunk text, return the assembled response.
async fn run_acp_command(command: &str, task: &str, workdir: &std::path::Path) -> Result<String> {
	let mut parts = command.split_whitespace();
	let program = parts
		.next()
		.ok_or_else(|| anyhow::anyhow!("Empty command"))?;
	let args: Vec<&str> = parts.collect();

	let mut child = Command::new(program)
		.args(&args)
		.current_dir(workdir)
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::null())
		.spawn()?;

	let mut stdin = child
		.stdin
		.take()
		.ok_or_else(|| anyhow::anyhow!("No stdin"))?;
	let stdout = child
		.stdout
		.take()
		.ok_or_else(|| anyhow::anyhow!("No stdout"))?;
	let mut lines = BufReader::new(stdout).lines();

	// Helper: serialize a JSON-RPC message to a newline-terminated string.
	let msg_line = |msg: Value| format!("{}\n", msg);

	// 1. initialize
	stdin
		.write_all(
			msg_line(json!({
				"jsonrpc": "2.0",
				"id": 1,
				"method": "initialize",
				"params": {
					"protocolVersion": "0.1.0",
					"clientInfo": {"name": "octomind-agent-tool", "version": "1.0"}
				}
			}))
			.as_bytes(),
		)
		.await?;
	wait_for_response(&mut lines, 1).await?;

	// 2. session/new
	let cwd_str = workdir.to_string_lossy();
	stdin
		.write_all(
			msg_line(json!({
				"jsonrpc": "2.0",
				"id": 2,
				"method": "session/new",
				"params": {"cwd": cwd_str, "mcpServers": []}
			}))
			.as_bytes(),
		)
		.await?;

	let session_resp = wait_for_response(&mut lines, 2).await?;
	let session_id = session_resp
		.get("result")
		.and_then(|r| r.get("sessionId"))
		.and_then(|s| s.as_str())
		.ok_or_else(|| anyhow::anyhow!("No sessionId in session/new response"))?
		.to_string();

	// 3. session/prompt — collect chunks until we get the response (id=3)
	stdin
		.write_all(
			msg_line(json!({
				"jsonrpc": "2.0",
				"id": 3,
				"method": "session/prompt",
				"params": {
					"sessionId": session_id,
					"prompt": [{"type": "text", "text": task}]
				}
			}))
			.as_bytes(),
		)
		.await?;

	let mut output = String::new();

	loop {
		let line = match lines.next_line().await? {
			Some(l) => l,
			None => break,
		};
		if line.trim().is_empty() {
			continue;
		}
		let msg: Value = match serde_json::from_str(&line) {
			Ok(v) => v,
			Err(_) => continue,
		};

		// Collect agent_message_chunk text from notifications
		if msg.get("method").and_then(|m| m.as_str()) == Some("session/update") {
			if let Some(update) = msg.pointer("/params/update") {
				if update.get("sessionUpdate").and_then(|u| u.as_str())
					== Some("agent_message_chunk")
				{
					if let Some(text) = update.pointer("/content/text").and_then(|t| t.as_str()) {
						output.push_str(text);
					}
				}
			}
		}

		// Stop when we get the prompt response (id=3)
		if msg.get("id").and_then(|i| i.as_u64()) == Some(3) {
			break;
		}
	}

	// Shut down the subprocess cleanly
	drop(stdin);
	let _ = child.wait().await;

	Ok(output.trim().to_string())
}

/// Read lines until we find a JSON-RPC response with the given id, return it.
async fn wait_for_response(
	lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
	id: u64,
) -> Result<Value> {
	loop {
		let line = match lines.next_line().await? {
			Some(l) => l,
			None => return Err(anyhow::anyhow!("Subprocess closed before response id={id}")),
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
				return Err(anyhow::anyhow!("ACP error: {err}"));
			}
			return Ok(msg);
		}
	}
}
