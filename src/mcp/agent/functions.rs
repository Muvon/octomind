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

// Agent functions - spawns ACP subprocess and drives the protocol to completion.

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::background_jobs::{BackgroundJobManager, CompletedJob, JobHandle};
use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

/// Global singleton — created once when the first async agent call arrives.
/// Used as fallback for CLI mode when not in a session context.
static JOB_MANAGER: OnceLock<Arc<BackgroundJobManager>> = OnceLock::new();

/// Get reasonable max concurrent jobs based on CPU cores (minimum 4)
fn get_max_concurrent_jobs() -> usize {
	std::thread::available_parallelism()
		.map(|p| p.get())
		.unwrap_or(4)
}

/// Initialize the job manager at session start.
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn init_job_manager() {
	// Check if we're in a session context
	if let Some(session_id) = crate::session::context::current_session_id() {
		crate::session::context::init_job_manager_for_session(&session_id);
		return;
	}

	// Fall back to global singleton for CLI mode (uses a dummy session id — no inbox)
	let manager = BackgroundJobManager::new(get_max_concurrent_jobs());
	let _ = JOB_MANAGER.set(Arc::new(manager));
}

/// Get the job manager for the current session or global fallback.
///
/// Session-aware: uses session-scoped registry when in a session context,
/// falls back to global singleton for CLI mode.
pub fn get_job_manager() -> Option<Arc<BackgroundJobManager>> {
	// Check if we're in a session context
	if let Some(manager) = crate::session::context::get_job_manager_for_session() {
		return Some(manager);
	}

	// Fall back to global singleton for CLI mode
	JOB_MANAGER.get().cloned()
}

/// Kill all running background jobs for the current context.
///
/// No-op when no job manager is registered (CLI mode pre-bootstrap, or
/// non-session contexts). Centralises the `get_job_manager() + kill_all()`
/// idiom used across exit/cancel paths.
pub fn kill_all_jobs() {
	if let Some(manager) = get_job_manager() {
		manager.kill_all();
	}
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
			Async execution:\n\
			async=false (default): blocks until complete, result returned immediately.\n\
			async=true: returns immediately, result injected as a user message when done.\n\n\
			Use async when task takes 30+ seconds, or you can continue other work while waiting.\n\
			Use sync when you need the result before your next action.\n\n\
			Result format: [Async agent 'name' completed] or [Async agent 'name' failed]\n\
			Max {} concurrent async jobs. Jobs cancelled on session exit.",
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

/// Execute an agent tool call.
/// For config-defined agents: spawns subprocess via ACP command.
/// For dynamic agents: executes in-process using ChatSession.
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

	// Check config-defined agents first (subprocess execution)
	let config_agent = config.agents.iter().find(|a| a.name == agent_name).cloned();

	// Then check dynamic agents (in-process execution)
	let dynamic_agent = crate::mcp::core::dynamic_agents::get_enabled_agent(agent_name);

	match (config_agent, dynamic_agent) {
		(Some(agent), None) => {
			// Config agent: subprocess execution
			execute_config_agent(call, &agent, task, config).await
		}
		(None, Some(agent)) => {
			// Dynamic agent: in-process execution
			execute_dynamic_agent(call, &agent, task, config).await
		}
		(None, None) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent '{agent_name}' not configured or not enabled"),
		)),
		(Some(_), Some(_)) => {
			// Should not happen - agent name conflict
			Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Agent '{agent_name}' exists in both config and dynamic agents - ambiguous"
				),
			))
		}
	}
}

/// Execute a config-defined agent via subprocess.
async fn execute_config_agent(
	call: &McpToolCall,
	agent_config: &crate::config::agents::AgentConfig,
	task: &str,
	_config: &crate::config::Config,
) -> Result<McpToolResult> {
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
		// Capture session ID before spawn — task-locals don't propagate across tokio::spawn
		let session_id = crate::session::context::current_session_id();

		// Spawn the async task
		let handle = tokio::spawn(async move {
			let run = async move {
				let mut parts = command.split_whitespace();
				let program = parts.next().unwrap_or("");
				let args: Vec<&str> = parts.collect();
				let output =
					match run_acp_command(program, &args, &task_owned, &workdir_owned, cancel_rx)
						.await
					{
						Ok(text) => text,
						Err(e) => format!("ERROR: {e:#}"),
					};
				mgr.release(CompletedJob {
					agent_name: agent_name_owned,
					output,
				});
			};
			if let Some(sid) = session_id {
				crate::session::context::with_session_id(sid, run).await;
			} else {
				run.await;
			}
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
	let mut parts = agent_config.command.split_whitespace();
	let program = parts.next().unwrap_or("");
	let args: Vec<&str> = parts.collect();
	match run_acp_command(program, &args, task, &workdir, watch::channel(false).1).await {
		Ok(output) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			output,
		)),
		Err(e) => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Agent execution failed: {e:#}"),
		)),
	}
}

/// Execute a dynamic agent in-process.
///
/// Builds a merged config from server_refs
/// (resolving both static config servers and dynamic servers), then runs
/// chat_completion_with_validation with a recursive tool call loop.
async fn execute_dynamic_agent(
	call: &McpToolCall,
	agent_config: &crate::mcp::core::dynamic_agents::DynamicAgentConfig,
	task: &str,
	config: &crate::config::Config,
) -> Result<McpToolResult> {
	let run_async = call
		.parameters
		.get("async")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Build the merged config for this agent (resolve server_refs from static + dynamic registries)
	let agent_config_owned = agent_config.clone();
	let merged_config = build_agent_config(&agent_config_owned, config);

	let tool_name = call.tool_name.clone();
	let tool_id = call.tool_id.clone();
	let task_owned = task.to_string();

	if run_async {
		let manager = match get_job_manager() {
			Some(m) => m,
			None => {
				return Ok(McpToolResult::error(
					tool_name,
					tool_id,
					"Async job manager not initialised (no active session)".to_string(),
				));
			}
		};

		if let Err(active) = manager.try_acquire() {
			return Ok(McpToolResult::error(
				tool_name,
				tool_id,
				format!(
					"Async job limit reached ({active}/{} active). Wait for existing jobs to complete.",
					get_max_concurrent_jobs()
				),
			));
		}

		let (cancel_tx, cancel_rx) = watch::channel(false);
		let mgr = Arc::clone(&manager);
		let agent_name = agent_config_owned.name.clone();
		// Capture session ID before spawn — task-locals don't propagate across tokio::spawn
		let session_id = crate::session::context::current_session_id();

		let handle = tokio::spawn(async move {
			let run = async move {
				let output = match run_dynamic_agent_in_process(
					&agent_config_owned,
					&task_owned,
					&merged_config,
					cancel_rx,
				)
				.await
				{
					Ok(text) => text,
					Err(e) => format!("ERROR: {e:#}"),
				};
				mgr.release(CompletedJob { agent_name, output });
			};
			if let Some(sid) = session_id {
				crate::session::context::with_session_id(sid, run).await;
			} else {
				run.await;
			}
		});

		manager.register_job(JobHandle {
			cancel_tx,
			task_handle: handle,
		});

		return Ok(McpToolResult::success(
			tool_name,
			tool_id,
			"Agent task started asynchronously. The result will be injected into this conversation automatically when ready.".to_string(),
		));
	}

	// Synchronous path — keep cancel_tx alive so the watch channel stays open.
	// Dropping it immediately closes the channel, which octolib treats as cancellation.
	let (_cancel_tx, cancel_rx) = watch::channel(false);
	match run_dynamic_agent_in_process(&agent_config_owned, &task_owned, &merged_config, cancel_rx)
		.await
	{
		Ok(output) => Ok(McpToolResult::success(tool_name, tool_id, output)),
		Err(e) => Ok(McpToolResult::error(
			tool_name,
			tool_id,
			format!("Agent execution failed: {e:#}"),
		)),
	}
}

/// Build a merged Config for a dynamic agent.
///
/// Resolves server_refs from both the static config registry and the dynamic
/// server registry, then overrides the model/temperature/top_p/top_k from
/// the agent config.
fn build_agent_config(
	agent: &crate::mcp::core::dynamic_agents::DynamicAgentConfig,
	base_config: &crate::config::Config,
) -> crate::config::Config {
	let mut merged = base_config.clone();

	// Resolve server_refs: check static config servers first, then dynamic servers
	if !agent.server_refs.is_empty() {
		// Collect all available servers: static + dynamic
		let dynamic_servers = crate::mcp::core::dynamic::get_all_configs();
		let mut all_servers = base_config.mcp.servers.clone();
		for ds in dynamic_servers {
			if !all_servers.iter().any(|s| s.name() == ds.name()) {
				all_servers.push(ds);
			}
		}

		// Use RoleMcpConfig to resolve server_refs with tool filtering
		// Note: auto_bind is not applied here since agent configs don't have a role context
		let role_mcp = crate::config::RoleMcpConfig {
			server_refs: agent.server_refs.clone(),
			allowed_tools: agent.allowed_tools.clone(),
		};
		let enabled_servers = role_mcp.get_enabled_servers(&all_servers, None);

		crate::log_debug!(
			"Dynamic agent '{}' enabling {} servers from server_refs: {:?}",
			agent.name,
			enabled_servers.len(),
			agent.server_refs
		);

		merged.mcp = crate::config::McpConfig {
			servers: enabled_servers,
			allowed_tools: agent.allowed_tools.clone(),
		};
	} else {
		// No server_refs — disable MCP for this agent
		merged.mcp.servers.clear();
		merged.mcp.allowed_tools.clear();
	}

	// Apply model override if specified
	if let Some(ref model) = agent.model {
		merged.model = model.clone();
	}

	merged
}

/// Core in-process execution for a dynamic agent.
///
/// Builds messages (system + user task), calls chat_completion_with_validation,
/// then handles recursive tool calls.
fn run_dynamic_agent_in_process(
	agent: &crate::mcp::core::dynamic_agents::DynamicAgentConfig,
	task: &str,
	agent_config: &crate::config::Config,
	operation_cancelled: watch::Receiver<bool>,
) -> BoxFuture<'static, Result<String>> {
	let agent = agent.clone();
	let task = task.to_string();
	let agent_config = agent_config.clone();
	Box::pin(async move {
		let agent = &agent;
		let task = task.as_str();
		let agent_config = &agent_config;
		use crate::session::{ChatCompletionWithValidationParams, Message};

		if *operation_cancelled.borrow() {
			anyhow::bail!(crate::session::cancellation::Cancelled);
		}

		let effective_model = agent
			.model
			.clone()
			.unwrap_or_else(|| agent_config.model.clone());

		let should_cache = crate::session::model_supports_caching(&effective_model);

		// Build messages: system prompt + user task
		let now = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();

		let messages = vec![
			Message {
				role: "system".to_string(),
				content: agent.system.clone(),
				timestamp: now,
				cached: should_cache,
				..Default::default()
			},
			Message {
				role: "user".to_string(),
				content: task.to_string(),
				timestamp: now,
				cached: false,
				..Default::default()
			},
		];

		// Initial API call
		let validation_params = ChatCompletionWithValidationParams::new(
			&messages,
			&effective_model,
			agent.temperature.unwrap_or(0.7),
			agent.top_p.unwrap_or(0.9),
			agent.top_k.unwrap_or(0),
			agent_config.get_effective_max_tokens(),
			agent_config,
		)
		.with_max_retries(agent_config.max_retries)
		.with_cancellation_token(operation_cancelled.clone());

		let response = crate::session::chat_completion_with_validation(validation_params).await?;

		if *operation_cancelled.borrow() {
			anyhow::bail!(crate::session::cancellation::Cancelled);
		}

		let mut current_content = response.content;
		let mut current_exchange = response.exchange;
		let mut current_tool_calls_param = response.tool_calls;

		// Recursive tool call loop
		if !agent.server_refs.is_empty() {
			// Accumulate messages for the conversation (system + user + tool rounds)
			let mut conv_messages = messages.clone();

			loop {
				if *operation_cancelled.borrow() {
					anyhow::bail!(crate::session::cancellation::Cancelled);
				}

				// Resolve tool calls for this iteration
				let current_tool_calls = if let Some(calls) = current_tool_calls_param.take() {
					if !calls.is_empty() {
						calls
					} else {
						crate::mcp::parse_tool_calls(&current_content)
					}
				} else {
					crate::mcp::parse_tool_calls(&current_content)
				};

				if current_tool_calls.is_empty() {
					break;
				}

				// Add assistant message with tool calls preserved
				let original_tool_calls =
					crate::session::chat::MessageHandler::extract_original_tool_calls(
						&current_exchange,
					);
				conv_messages.push(Message {
					role: "assistant".to_string(),
					content: current_content.clone(),
					timestamp: std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap_or_default()
						.as_secs(),
					cached: false,
					tool_calls: original_tool_calls,
					..Default::default()
				});

				// Execute tool calls in parallel
				let output_mode = crate::session::output::detect_output_mode(
					agent_config
						.runtime_output_mode
						.as_deref()
						.unwrap_or("plain"),
				);
				let layer_tool_params =
					crate::session::chat::response::tool_execution::LayerToolExecutionParams {
						tool_calls: current_tool_calls,
						session_name: format!("agent_{}", agent.name),
						layer_name: format!("agent_{}", agent.name),
						operation_cancelled: Some(operation_cancelled.clone()),
						mode: output_mode,
					};
				let (tool_results, _tool_time) =
				crate::session::chat::response::tool_execution::execute_layer_tool_calls_parallel(
					agent_config,
					layer_tool_params,
				)
				.await?;

				if *operation_cancelled.borrow() {
					anyhow::bail!(crate::session::cancellation::Cancelled);
				}

				if tool_results.is_empty() {
					break;
				}

				// Add tool result messages
				for tool_result in &tool_results {
					let raw_content = tool_result.extract_content();

					let (tool_content, _) = crate::utils::truncation::truncate_mcp_response_global(
						&raw_content,
						agent_config.mcp_response_tokens_threshold,
					);

					conv_messages.push(Message {
						role: "tool".to_string(),
						content: tool_content,
						timestamp: std::time::SystemTime::now()
							.duration_since(std::time::UNIX_EPOCH)
							.unwrap_or_default()
							.as_secs(),
						cached: false,
						tool_call_id: Some(tool_result.tool_id.clone()),
						name: Some(tool_result.tool_name.clone()),
						..Default::default()
					});
				}

				// Follow-up API call with tool results
				let follow_up_params = ChatCompletionWithValidationParams::new(
					&conv_messages,
					&effective_model,
					agent.temperature.unwrap_or(0.7),
					agent.top_p.unwrap_or(0.9),
					agent.top_k.unwrap_or(0),
					agent_config.get_effective_max_tokens(),
					agent_config,
				)
				.with_max_retries(agent_config.max_retries)
				.with_cancellation_token(operation_cancelled.clone());

				match crate::session::chat_completion_with_validation(follow_up_params).await {
					Ok(follow_up) => {
						if *operation_cancelled.borrow() {
							anyhow::bail!(crate::session::cancellation::Cancelled);
						}

						let has_tool_calls = if let Some(ref calls) = follow_up.tool_calls {
							!calls.is_empty()
						} else {
							!crate::mcp::parse_tool_calls(&follow_up.content).is_empty()
						};

						let should_continue = crate::session::chat::response::tool_result_processor::check_should_continue(
						&follow_up,
						agent_config,
						has_tool_calls,
					);

						current_content = follow_up.content;
						current_exchange = follow_up.exchange;
						current_tool_calls_param = follow_up.tool_calls;

						if !should_continue {
							break;
						}
					}
					Err(e) => {
						crate::log_error!(
							"Dynamic agent '{}' follow-up API call failed: {}",
							agent.name,
							e
						);
						return Err(e);
					}
				}
			}
		}

		Ok(current_content.trim().to_string())
	}) // Box::pin
}

/// Spawn the ACP command, drive initialize → session/new → session/prompt.
/// Used by both agents and layers to execute via ACP protocol.
///
/// `program` is the executable path; `args` are CLI arguments passed verbatim.
/// Callers that have a single "program plus space-separated args" string should
/// split it themselves (e.g. via `split_whitespace`) before calling.
pub async fn run_acp_command(
	program: &str,
	args: &[&str],
	task: &str,
	workdir: &std::path::Path,
	mut cancel_rx: watch::Receiver<bool>,
) -> Result<String> {
	let mut child = Command::new(program)
		.args(args)
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
	// Captured prompt-response error: if the subprocess returns
	// `{"id":3,"error":{...}}` we want to surface it instead of silently
	// returning an empty string. Without this the parent sees `output: ""`
	// with status `done` even when the API call inside the subprocess failed.
	let mut prompt_error: Option<Value> = None;

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

		// Forward session/update notifications to the parent's notification
		// sink so the user sees thinking, tool calls, and tool results
		// streamed live — the same shape the parent renders for its own
		// in-process tool calls.
		if msg.get("method").and_then(|m| m.as_str()) == Some("session/update") {
			if let Some(update) = msg.pointer("/params/update") {
				forward_session_update_to_parent(update);
				if update.get("sessionUpdate").and_then(|u| u.as_str())
					== Some("agent_message_chunk")
				{
					if let Some(text) = update.pointer("/content/text").and_then(|t| t.as_str()) {
						output.push_str(text);
					}
				}
			}
		}

		// Stop when we get the prompt response (id=3). Capture any error
		// payload so we can fail the call instead of returning empty output.
		if msg.get("id").and_then(|i| i.as_u64()) == Some(3) {
			if let Some(err) = msg.get("error") {
				prompt_error = Some(err.clone());
			}
			break;
		}
	}

	// Shut down the subprocess cleanly
	drop(stdin);
	let _ = child.wait().await;

	if let Some(err) = prompt_error {
		let trimmed = output.trim();
		let detail = err
			.get("message")
			.and_then(|m| m.as_str())
			.map(|s| s.to_string())
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

/// Convert an ACP `session/update` notification into a `ServerMessage` and
/// push it through the parent's notification sender. Lets `agent_*`, `tap`,
/// and layer subprocess events render on the parent's output sink (CLI
/// stream, JSONL, websocket) instead of being silently dropped.
fn forward_session_update_to_parent(update: &Value) {
	let kind = match update.get("sessionUpdate").and_then(|u| u.as_str()) {
		Some(k) => k,
		None => return,
	};
	let session_id =
		crate::session::context::current_session_id().unwrap_or_else(|| String::from("acp"));

	let msg = match kind {
		"agent_message_chunk" => {
			let text = update
				.pointer("/content/text")
				.and_then(|t| t.as_str())
				.unwrap_or("");
			if text.is_empty() {
				return;
			}
			crate::websocket::ServerMessage::Assistant(crate::websocket::AssistantPayload {
				content: text.to_string(),
				session_id,
			})
		}
		"agent_thought_chunk" => {
			let text = update
				.pointer("/content/text")
				.and_then(|t| t.as_str())
				.unwrap_or("");
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
			let title = update
				.get("title")
				.and_then(|s| s.as_str())
				.unwrap_or("")
				.to_string();
			let raw_input = update
				.get("rawInput")
				.cloned()
				.unwrap_or(serde_json::Value::Null);
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
