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
use crate::session::background_jobs::BackgroundJobManager;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Global singleton for background job tracking.
/// Initialized on first agent call using the config's background_jobs settings.
static JOB_MANAGER: OnceLock<Arc<BackgroundJobManager>> = OnceLock::new();

/// Guard to ensure the periodic cleanup task is spawned only once.
static CLEANUP_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Return the global BackgroundJobManager, initializing it from config on first call.
fn get_or_init_job_manager(config: &crate::config::Config) -> Arc<BackgroundJobManager> {
	let manager = JOB_MANAGER.get_or_init(|| {
		Arc::new(BackgroundJobManager::new(
			config.background_jobs.ttl_seconds,
		))
	});

	// Spawn the periodic cleanup task exactly once
	if !CLEANUP_SPAWNED.swap(true, Ordering::SeqCst) {
		let mgr = Arc::clone(manager);
		let interval_secs = config.background_jobs.cleanup_interval_seconds;
		tokio::spawn(async move {
			let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
			ticker.tick().await; // skip the immediate first tick
			loop {
				ticker.tick().await;
				let removed = mgr.cleanup_expired_jobs();
				if removed > 0 {
					crate::log_debug!("Background job cleanup: removed {} expired jobs", removed);
				}
			}
		});
	}

	Arc::clone(manager)
}

/// Expose the job manager for the /jobs command (returns None if never initialized).
pub fn try_get_job_manager() -> Option<Arc<BackgroundJobManager>> {
	JOB_MANAGER.get().cloned()
}

/// Get all available agent functions based on config.
///
/// Each agent becomes a separate MCP tool (e.g., `agent_context_gatherer`).
pub fn get_all_functions(config: &crate::config::Config) -> Vec<McpFunction> {
	let mut functions = Vec::new();

	for agent_config in &config.agents {
		functions.push(McpFunction {
			name: format!("agent_{}", agent_config.name),
			description: format!(
				"{}\n\nSupports background execution: set `background: true` to return a job_id immediately instead of waiting.",
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
						"description": "Run agent task in background and return immediately with a job_id for polling. Default false (synchronous).",
						"default": false
					}
				},
				"required": ["task"]
			}),
		});
	}

	// Job status/result query tools — always present when agent server is enabled
	functions.push(McpFunction {
		name: "get_agent_job_status".to_string(),
		description: "Get the current status of a background agent job. Returns status (pending/running/completed/failed), timestamps, and agent name.".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"job_id": {
					"type": "string",
					"description": "The job ID returned by a background agent call"
				}
			},
			"required": ["job_id"]
		}),
	});
	functions.push(McpFunction {
		name: "get_agent_job_result".to_string(),
		description: "Retrieve the result or error of a completed/failed background agent job.".to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"job_id": {
					"type": "string",
					"description": "The job ID returned by a background agent call"
				}
			},
			"required": ["job_id"]
		}),
	});

	functions
}

/// Execute an agent tool call by spawning the configured ACP command as a subprocess
/// and driving the ACP protocol (initialize → session/new → session/prompt) over stdio.
pub async fn execute_agent_command(
	call: &McpToolCall,
	config: &crate::config::Config,
	_cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<McpToolResult> {
	// Handle job query tools first
	if call.tool_name == "get_agent_job_status" {
		return execute_get_job_status(call, config);
	}
	if call.tool_name == "get_agent_job_result" {
		return execute_get_job_result(call, config);
	}

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
		let manager = get_or_init_job_manager(config);

		let max_jobs = config.background_jobs.max_concurrent_jobs;
		if manager.active_job_count() >= max_jobs {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Background job limit reached ({max_jobs} active jobs). Wait for existing jobs to complete."),
			));
		}

		let job_id = manager.submit_job(agent_config.name.clone(), task);
		let job_id_clone = job_id.clone();
		let mgr = Arc::clone(&manager);
		let command = agent_config.command.clone();
		let task_owned = task.to_string();

		tokio::spawn(async move {
			mgr.update_job_running(&job_id_clone);
			match run_acp_command(&command, &task_owned, &workdir).await {
				Ok(result) => mgr.complete_job(&job_id_clone, result),
				Err(e) => mgr.fail_job(&job_id_clone, format!("{e:#}")),
			}
		});

		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			serde_json::to_string(&json!({
				"job_id": job_id,
				"status": "pending",
				"message": "Agent task submitted. Use get_agent_job_status / get_agent_job_result to poll."
			}))
			.unwrap_or_default(),
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

/// Return current status of a background job.
fn execute_get_job_status(
	call: &McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let job_id = match call.parameters.get("job_id").and_then(|v| v.as_str()) {
		Some(id) if !id.trim().is_empty() => id,
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing or empty 'job_id' parameter".to_string(),
			));
		}
	};
	let manager = get_or_init_job_manager(config);
	match manager.get_job(job_id) {
		None => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Job '{job_id}' not found (may have expired or never existed)"),
		)),
		Some(job) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			serde_json::to_string(&json!({
				"job_id": job.job_id,
				"agent_name": job.agent_name,
				"status": job.status,
				"task_preview": job.task_preview,
				"created_at": job.created_at,
				"updated_at": job.updated_at,
				"expires_at": job.expires_at,
			}))
			.unwrap_or_default(),
		)),
	}
}

/// Return result or error of a completed/failed background job.
fn execute_get_job_result(
	call: &McpToolCall,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let job_id = match call.parameters.get("job_id").and_then(|v| v.as_str()) {
		Some(id) if !id.trim().is_empty() => id,
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing or empty 'job_id' parameter".to_string(),
			));
		}
	};
	let manager = get_or_init_job_manager(config);
	match manager.get_job(job_id) {
		None => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Job '{job_id}' not found (may have expired or never existed)"),
		)),
		Some(job) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			serde_json::to_string(&json!({
				"job_id": job.job_id,
				"status": job.status,
				"result": job.result,
				"error": job.error,
			}))
			.unwrap_or_default(),
		)),
	}
}

/// Spawn the ACP command, drive initialize → session/new → session/prompt,
/// collect all agent_message_chunk text, return the assembled response.
async fn run_acp_command(command: &str, task: &str, workdir: &std::path::Path) -> Result<String> {
	// Split command into program + args
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
