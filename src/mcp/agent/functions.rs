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
use crate::session::background_jobs::{BackgroundJobManager, CompletedJob, JobHandle};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

/// Global singleton — created once when the first async agent call arrives.
static JOB_MANAGER: OnceLock<Arc<BackgroundJobManager>> = OnceLock::new();

/// Get reasonable max concurrent jobs based on CPU cores (minimum 4)
fn get_max_concurrent_jobs() -> usize {
	std::thread::available_parallelism()
		.map(|p| p.get())
		.unwrap_or(4)
}

/// Initialize the job manager at session start. Returns the receiver for completed jobs.
pub fn init_job_manager() -> tokio::sync::mpsc::Receiver<CompletedJob> {
	let (manager, rx) = BackgroundJobManager::new(get_max_concurrent_jobs());
	JOB_MANAGER
		.set(Arc::new(manager))
		.expect("job manager already initialized");
	rx
}

pub fn get_job_manager() -> Option<Arc<BackgroundJobManager>> {
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
				"{}\n\n\
				## Async Execution\n\n\
				**async: false** (default) — Blocks until complete. Use when you need the result immediately.\n\n\
				**async: true** — Returns immediately, runs asynchronously. Result appears as a user message when complete.\n\n\
				**When to use async:**\n\
				- Task takes 30+ seconds (large codebase analysis, multi-file refactoring)\n\
				- You can continue other work while waiting\n\
				- You don't need the result for your next immediate action\n\n\
				**When NOT to use async:**\n\
				- You need the result to make your next decision\n\
				- Quick tasks (under 30 seconds)\n\
				- Multi-step tasks where each step depends on the previous result\n\n\
				**Result format:** `[Async agent 'name' completed]` or `[Async agent 'name' failed]`\n\n\
				**Limits:** Max {} concurrent async jobs. Jobs are cancelled on session exit/interrupt.",
				agent_config.description,
				get_max_concurrent_jobs()
			),
				parameters: json!({
					"type": "object",
					"properties": {
						"task": {
							"type": "string",
							"description": "Task description in human language for the agent to process"
						},
						"async": {
							"type": "boolean",
							"description": "Run asynchronously. Result injected as user message when complete. Use for long-running tasks where you can continue other work. Default: false.",
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

	let run_async = call
		.parameters
		.get("async")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	let session_workdir = crate::mcp::get_thread_working_directory();
	let workdir = agent_config.get_resolved_workdir(&session_workdir);

	if run_async {
		let manager = match get_job_manager() {
			Some(m) => m,
			None => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Async job manager not initialised (no active session)".to_string(),
				));
			}
		};

		if let Err(active) = manager.try_acquire() {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Async job limit reached ({active}/{} active). Wait for existing jobs to complete.", get_max_concurrent_jobs()),
			));
		}

		// Create cancellation channel for this job
		let (cancel_tx, cancel_rx) = watch::channel(false);

		let mgr = Arc::clone(&manager);
		let command = agent_config.command.clone();
		let agent_name_owned = agent_config.name.clone();
		let task_owned = task.to_string();
		let workdir_owned = workdir.to_path_buf();

		// Spawn the async task
		let handle = tokio::spawn(async move {
			let output =
				match run_acp_command(&command, &task_owned, &workdir_owned, cancel_rx).await {
					Ok(text) => text,
					Err(e) => format!("ERROR: {e:#}"),
				};
			mgr.release(CompletedJob {
				agent_name: agent_name_owned,
				output,
			});
		});

		// Register the job for potential cancellation
		manager.register_job(JobHandle {
			cancel_tx,
			task_handle: handle,
		});

		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"Agent task started asynchronously. The result will be injected into this conversation automatically when ready.".to_string(),
		));
	}

	// Synchronous path (default)
	match run_acp_command(
		&agent_config.command,
		task,
		&workdir,
		watch::channel(false).1,
	)
	.await
	{
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
/// Accepts a cancellation receiver - when true, aborts the child process.
async fn run_acp_command(
	command: &str,
	task: &str,
	workdir: &std::path::Path,
	mut cancel_rx: watch::Receiver<bool>,
) -> Result<String> {
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
		// Check for cancellation before each line read
		if *cancel_rx.borrow() {
			// Kill the child process on cancellation
			let _ = child.kill().await;
			return Err(anyhow::anyhow!("Agent task cancelled"));
		}

		// Use tokio::select to handle both cancellation and line reading
		let line = tokio::select! {
			line = lines.next_line() => {
				match line? {
					Some(l) => l,
					None => break,
				}
			}
			_ = cancel_rx.changed() => {
				// Cancellation received - kill child and return
				let _ = child.kill().await;
				return Err(anyhow::anyhow!("Agent task cancelled"));
			}
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
