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

//! `tap` core tool — run, list, stop, and discover agents from configured taps.
//!
//! Exposes four actions over a single tool surface:
//!
//! - `run`      — launch a fresh tap-tag (e.g. `developer:general`) or resume
//!   an existing run by `session` id. `prompt` is required.
//!   `background=false` (default) waits for the assistant turn
//!   and returns inline; `background=true` returns the id
//!   immediately and pushes the result to the parent session
//!   inbox when done.
//! - `list`     — show every tap-run in the current session with id, tag,
//!   status, and start time.
//! - `stop`     — cancel a running tap-run by id (sends the cancel watch).
//! - `discover` — semantic match a free-text intent against installed tap
//!   agents' `# Title:` / `# Description:` header lines.
//!   Same matcher pipeline as `capability discover`.
//!
//! Tap-runs are tracked in `crate::session::tap_runs` — a registry that is
//! intentionally separate from `BackgroundJobManager` (which tracks
//! `agent_*`). The two subsystems share only generic primitives (tokio
//! tasks, `watch::Sender` cancellation, the embedding matcher).

use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::watch;

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::tap_runs::{self, TapJob, TapJobInfo, TapJobStatus};
use crate::session::{ChatCompletionWithValidationParams, Message};

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

pub fn get_tap_function() -> McpFunction {
	McpFunction {
		name: "tap".to_string(),
		description: r#"Delegate work to specialist roles installed via taps. A role is a pre-built agent persona — its own system prompt, model, and tool kit — identified by `category:variant` (e.g. `developer:general`, `lawyer:us`, `security:owasp`). Use this tool to hand off a focused task, watch its progress, stop it, or browse the catalog.

When to use:
- The current task fits a specialist better than your generalist context (legal review, docker debugging, financial analysis, …).
- You want a long-running side task while continuing other work — call `run` with `background=true`; the specialist's reply lands in your next turn.
- You want to keep a focused dialog with one specialist across multiple turns — keep the returned `session` id and pass it back on subsequent `run` calls.

Discovery flow:
- If you know the role: `tap(action="run", role="developer:general", prompt="…")`.
- If you don't: `tap(action="discover", intent="<plain-English need>")` returns the closest 5 roles ranked by semantic match. Pick one, then `run`.

Actions:
- `run`      — launch a role. Required: `role` (for new runs) OR `session` (to resume), plus `prompt`. Optional: `workdir` (defaults to current cwd), `background` (default false; true = return immediately, reply injected later).
- `list`     — show every run in this session: id, role, status (running|done|failed|cancelled), start time, workdir.
- `stop`     — cancel a running specialist. Required: `session` (the id).
- `discover` — find roles matching free-text intent. Required: `intent`. Returns top matches with title, description, and source tap."#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"enum": ["run", "list", "stop", "discover"],
					"description": "Action to perform"
				},
				"role": {
					"type": "string",
					"description": "Specialist role to launch, e.g. 'developer:general'. Required for run when `session` is not given. Use `discover` first if unsure which role fits."
				},
				"prompt": {
					"type": "string",
					"description": "User message to send to the specialist. Required for run."
				},
				"session": {
					"type": "string",
					"description": "Run id (e.g. 'tap-developer-general-a3f1c2'). Required for stop. For run, supply this to resume an existing run instead of starting fresh."
				},
				"workdir": {
					"type": "string",
					"description": "Working directory the specialist operates in. Optional — defaults to the current working directory. Useful when the specialist must reason over a different repo or sub-project than the parent."
				},
				"background": {
					"type": "boolean",
					"description": "When true, return immediately and inject the reply into this conversation when ready. Default: false (wait inline).",
					"default": false
				},
				"intent": {
					"type": "string",
					"description": "Free-text intent for discover (e.g., 'review a Singapore employment contract', 'debug a Kubernetes pod crash')."
				}
			},
			"required": ["action"]
		}),
	}
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

pub async fn execute_tap_command(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let action = match call.parameters.get("action").and_then(|v| v.as_str()) {
		Some(a) if !a.trim().is_empty() => a.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'action'".to_string(),
			));
		}
	};
	match action.as_str() {
		"list" => handle_list(call).await,
		"run" => handle_run(call, config).await,
		"stop" => handle_stop(call).await,
		"discover" => handle_discover(call).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Unknown action '{other}'. Use run, list, stop, or discover."),
		)),
	}
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_list(call: &McpToolCall) -> Result<McpToolResult> {
	let jobs = tap_runs::list_jobs();
	if jobs.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No tap-runs in this session.".to_string(),
		));
	}
	let entries: Vec<serde_json::Value> = jobs.iter().map(format_job_info).collect();
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({
			"count": entries.len(),
			"runs": entries,
		})
		.to_string(),
	))
}

async fn handle_stop(call: &McpToolCall) -> Result<McpToolResult> {
	let session = match call.parameters.get("session").and_then(|v| v.as_str()) {
		Some(s) if !s.trim().is_empty() => s.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'session' (run id).".to_string(),
			));
		}
	};
	match tap_runs::cancel_job(&session) {
		Some(status) => Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			json!({
				"id": session,
				"status": status.as_str(),
			})
			.to_string(),
		)),
		None => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("No tap-run with id '{session}' in this session."),
		)),
	}
}

async fn handle_discover(call: &McpToolCall) -> Result<McpToolResult> {
	let intent = match call.parameters.get("intent").and_then(|v| v.as_str()) {
		Some(i) if !i.trim().is_empty() => i.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'intent'.".to_string(),
			));
		}
	};
	let agents = match crate::agent::registry::list_all_tap_agents() {
		Ok(a) => a,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to enumerate tap agents: {e:#}"),
			));
		}
	};
	if agents.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No tap agents installed.".to_string(),
		));
	}
	if !crate::embeddings::is_ready() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"tap discover requires the embedding model. Init failed or not ready yet.".to_string(),
		));
	}

	let intent_vec = match crate::embeddings::embed(&intent).await {
		Ok(v) => v,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("tap discover embedding failed: {e:#}"),
			));
		}
	};
	let corpus: Vec<String> = agents
		.iter()
		.map(|a| format!("{}. {}", a.meta.title, a.meta.description))
		.collect();
	let corpus_vecs = match crate::embeddings::embed_many(&corpus).await {
		Ok(v) => v,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("tap discover embedding failed: {e:#}"),
			));
		}
	};

	let mut scored: Vec<(f32, &crate::agent::registry::TapAgent)> = agents
		.iter()
		.zip(corpus_vecs.iter())
		.map(|(a, v)| (crate::embeddings::cosine(&intent_vec, v), a))
		.filter(|(score, _)| *score > 0.2)
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	let top: Vec<_> = scored.into_iter().take(5).collect();

	if top.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("No tap agents matched intent '{intent}'."),
		));
	}

	let entries: Vec<serde_json::Value> = top
		.into_iter()
		.map(|(score, a)| {
			json!({
				"role": a.role,
				"title": a.meta.title,
				"description": a.meta.description,
				"source_tap": a.source_tap,
				"score": (score * 100.0).round() / 100.0,
			})
		})
		.collect();
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({
			"intent": intent,
			"matches": entries,
		})
		.to_string(),
	))
}

async fn handle_run(call: &McpToolCall, _config: &Config) -> Result<McpToolResult> {
	let prompt = match call.parameters.get("prompt").and_then(|v| v.as_str()) {
		Some(p) if !p.trim().is_empty() => p.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'prompt'.".to_string(),
			));
		}
	};
	let session = call
		.parameters
		.get("session")
		.and_then(|v| v.as_str())
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty());
	let role_param = call
		.parameters
		.get("role")
		.and_then(|v| v.as_str())
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty());
	let workdir_param = call
		.parameters
		.get("workdir")
		.and_then(|v| v.as_str())
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty());
	let background = call
		.parameters
		.get("background")
		.and_then(|v| v.as_bool())
		.unwrap_or(false);

	// Default workdir is the parent session's current cwd. Resolved early
	// so resume picks up the original workdir from the existing job.
	let cwd_default = std::env::current_dir()
		.map(|p| p.to_string_lossy().to_string())
		.unwrap_or_else(|_| ".".to_string());

	// Tap-runs always resolve the manifest against a freshly-loaded base
	// config — never the parent session's already-merged config. The parent
	// may have its own tap-merged role baked in, which would cause the
	// resolver's "must define a new role" check to skip the manifest's role
	// during merge and then fail.
	let base_config = match crate::config::Config::load() {
		Ok(c) => c,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to load base config for tap-run: {e:#}"),
			));
		}
	};

	// Resolve (id, role, workdir, history, status, cancel_rx) for either resume or fresh run.
	let (id, role, workdir, history, status, cancel_rx) = if let Some(sid) = session {
		let (history, status, cancel_rx) = match tap_runs::get_handles(&sid) {
			Some(h) => h,
			None => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("No tap-run with id '{sid}' in this session."),
				));
			}
		};
		let info = match tap_runs::find_job(&sid) {
			Some(i) => i,
			None => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("No tap-run with id '{sid}' in this session."),
				));
			}
		};
		// Reject if a turn is already running for this job.
		{
			let s = match status.read() {
				Ok(s) => *s,
				Err(_) => {
					return Ok(McpToolResult::error(
						call.tool_name.clone(),
						call.tool_id.clone(),
						"Tap-run status lock poisoned.".to_string(),
					));
				}
			};
			if s == TapJobStatus::Running {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Tap-run '{sid}' is busy with a previous turn — wait or call stop."),
				));
			}
		}
		// Mark running for the new turn.
		if let Ok(mut s) = status.write() {
			*s = TapJobStatus::Running;
		}
		(sid, info.role, info.workdir, history, status, cancel_rx)
	} else {
		let role = match role_param {
			Some(t) => t,
			None => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Missing 'role' for new run (or supply 'session' to resume).".to_string(),
				));
			}
		};
		let workdir = workdir_param.unwrap_or(cwd_default);
		let id = tap_runs::generate_id(&role);
		let history = Arc::new(RwLock::new(Vec::<Message>::new()));
		let status = Arc::new(RwLock::new(TapJobStatus::Running));
		let (cancel_tx, cancel_rx) = watch::channel(false);
		tap_runs::register_job(TapJob {
			id: id.clone(),
			role: role.clone(),
			workdir: workdir.clone(),
			started_at: SystemTime::now(),
			status: Arc::clone(&status),
			history: Arc::clone(&history),
			cancel_tx,
		});
		(id, role, workdir, history, status, cancel_rx)
	};

	if background {
		let id_owned = id.clone();
		let role_owned = role.clone();
		let workdir_owned = workdir.clone();
		let prompt_owned = prompt.clone();
		let base_config_owned = base_config.clone();
		let history_bg = Arc::clone(&history);
		let status_bg = Arc::clone(&status);
		let session_id = crate::session::context::current_session_id();
		tokio::spawn(async move {
			let run = async move {
				let outcome = run_tap_role(
					role_owned.clone(),
					prompt_owned,
					workdir_owned,
					history_bg,
					cancel_rx,
					base_config_owned,
				)
				.await;
				let (terminal, content) = match outcome {
					Ok(text) => (
						TapJobStatus::Done,
						format!("[Tap-run '{id_owned}' ({role_owned}) completed]\n\n{text}"),
					),
					Err(e) => (
						TapJobStatus::Failed,
						format!("[Tap-run '{id_owned}' ({role_owned}) failed]\n\n{e:#}"),
					),
				};
				if let Ok(mut s) = status_bg.write() {
					if *s == TapJobStatus::Running {
						*s = terminal;
					}
				}
				crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
					source: crate::session::inbox::InboxSource::TapRun {
						id: id_owned,
						role: role_owned,
					},
					content,
				});
			};
			if let Some(sid) = session_id {
				crate::session::context::with_session_id(sid, run).await;
			} else {
				run.await;
			}
		});
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			json!({
				"id": id,
				"role": role,
				"workdir": workdir,
				"background": true,
				"message": "Tap-run started. Reply will be injected as a user message when ready.",
			})
			.to_string(),
		));
	}

	// Foreground — block until the LLM round completes.
	let outcome = run_tap_role(
		role.clone(),
		prompt.clone(),
		workdir.clone(),
		Arc::clone(&history),
		cancel_rx,
		base_config,
	)
	.await;
	match outcome {
		Ok(text) => {
			if let Ok(mut s) = status.write() {
				if *s == TapJobStatus::Running {
					*s = TapJobStatus::Done;
				}
			}
			Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				json!({
					"id": id,
					"role": role,
					"workdir": workdir,
					"background": false,
					"output": text,
				})
				.to_string(),
			))
		}
		Err(e) => {
			if let Ok(mut s) = status.write() {
				if *s == TapJobStatus::Running {
					*s = TapJobStatus::Failed;
				}
			}
			Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Tap-run '{id}' ({role}) failed: {e:#}"),
			))
		}
	}
}

// ---------------------------------------------------------------------------
// Runner — single-turn LLM loop against a resolved tap role.
//
// Mirrors the *technique* of `run_dynamic_agent_in_process` (system prompt +
// user message → API call → tool call recursion → terminal text) but is a
// separate code path: the tap subsystem doesn't share runtime types with
// `agent_*`. Persists conversation in the supplied `history` Arc so resuming
// the same id picks up where the prior turn left off.
// ---------------------------------------------------------------------------

fn run_tap_role(
	role: String,
	prompt: String,
	workdir: String,
	history: Arc<RwLock<Vec<Message>>>,
	cancel_rx: watch::Receiver<bool>,
	base_config: Config,
) -> BoxFuture<'static, Result<String>> {
	// `with_non_interactive` flips a task-local flag so any INPUT/ENV
	// resolution that would otherwise prompt stdin returns a structured
	// "missing input" error instead. Without this the parent agent's tool
	// call deadlocks waiting for a user prompt that's unreachable from
	// inside an MCP dispatch.
	Box::pin(crate::agent::inputs::with_non_interactive(
		run_tap_role_inner(role, prompt, workdir, history, cancel_rx, base_config),
	))
}

async fn run_tap_role_inner(
	role: String,
	prompt: String,
	workdir: String,
	history: Arc<RwLock<Vec<Message>>>,
	cancel_rx: watch::Receiver<bool>,
	base_config: Config,
) -> Result<String> {
	if *cancel_rx.borrow() {
		anyhow::bail!("Operation cancelled");
	}

	// Resolve role tag → merged config + role name. This pulls the tap
	// manifest, resolves capabilities/inputs/deps, and merges into a
	// runnable Config. `base_config` here is freshly loaded from disk by
	// the caller — never the parent session's already-merged config.
	let (resolved_config, role_name) =
		crate::agent::resolver::resolve_config_and_role(Some(&role), &base_config, None).await?;

	// `merge_agent_toml` does a *union* of mcp.servers (base + manifest), so
	// `resolved_config.mcp.servers` still includes the user's local servers
	// (e.g. http_test, github) that the role doesn't actually want. Run the
	// merged config through `get_merged_config_for_role` to filter
	// `mcp.servers` down to only what this role's `server_refs` enable —
	// same shape `octomind run` uses for the standard flow.
	let mut resolved_config = resolved_config.get_merged_config_for_role(&role_name);

	// Apply per-run workdir override so filesystem tools target the right
	// directory. Stored on Config so downstream tool execution honors it.
	resolved_config.set_working_directory(std::path::PathBuf::from(&workdir));

	let role_struct = resolved_config.get_role_config_struct(&role_name).clone();
	let effective_model = role_struct
		.model
		.clone()
		.unwrap_or_else(|| resolved_config.model.clone());
	let temperature = role_struct.temperature;
	let top_p = role_struct.top_p;
	let top_k = role_struct.top_k;
	let should_cache = crate::session::model_supports_caching(&effective_model);

	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// Append system (only on first turn) + user. Borrow guard scoped tight
	// so we don't hold the lock across awaits.
	{
		let mut h = history.write().unwrap();
		if h.is_empty() {
			h.push(Message {
				role: "system".to_string(),
				content: role_struct.system.clone(),
				timestamp: now,
				cached: should_cache,
				..Default::default()
			});
		}
		h.push(Message {
			role: "user".to_string(),
			content: prompt.clone(),
			timestamp: now,
			cached: false,
			..Default::default()
		});
	}

	// Snapshot the history into an owned Vec — the guard's lifetime is
	// confined to this block so it never spans a subsequent `.await`.
	let mut conv_messages: Vec<Message> = {
		let h = history.read().unwrap();
		h.clone()
	};

	let validation_params = ChatCompletionWithValidationParams::new(
		&conv_messages,
		&effective_model,
		temperature,
		top_p,
		top_k,
		resolved_config.get_effective_max_tokens(),
		&resolved_config,
	)
	.with_max_retries(resolved_config.max_retries)
	.with_cancellation_token(cancel_rx.clone());

	let response = crate::session::chat_completion_with_validation(validation_params).await?;

	if *cancel_rx.borrow() {
		anyhow::bail!("Operation cancelled");
	}

	let mut current_content = response.content;
	let mut current_exchange = response.exchange;
	let mut current_tool_calls_param = response.tool_calls;

	loop {
		if *cancel_rx.borrow() {
			anyhow::bail!("Operation cancelled");
		}

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

		let original_tool_calls =
			crate::session::chat::MessageHandler::extract_original_tool_calls(&current_exchange);
		let assistant_msg = Message {
			role: "assistant".to_string(),
			content: current_content.clone(),
			timestamp: std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap_or_default()
				.as_secs(),
			cached: false,
			tool_calls: original_tool_calls,
			..Default::default()
		};
		conv_messages.push(assistant_msg.clone());
		{
			let mut h = history.write().unwrap();
			h.push(assistant_msg);
		}

		let output_mode = crate::session::output::detect_output_mode(
			resolved_config
				.runtime_output_mode
				.as_deref()
				.unwrap_or("plain"),
		);
		let layer_params =
			crate::session::chat::response::tool_execution::LayerToolExecutionParams {
				tool_calls: current_tool_calls,
				session_name: format!("tap_{role_name}"),
				layer_name: format!("tap_{role_name}"),
				operation_cancelled: Some(cancel_rx.clone()),
				mode: output_mode,
			};
		let (tool_results, _) =
			crate::session::chat::response::tool_execution::execute_layer_tool_calls_parallel(
				&resolved_config,
				layer_params,
			)
			.await?;

		if *cancel_rx.borrow() {
			anyhow::bail!("Operation cancelled");
		}
		if tool_results.is_empty() {
			break;
		}

		for tool_result in &tool_results {
			let raw_content = tool_result.extract_content();
			let (tool_content, _) = crate::utils::truncation::truncate_mcp_response_global(
				&raw_content,
				resolved_config.mcp_response_tokens_threshold,
			);
			let tool_msg = Message {
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
			};
			conv_messages.push(tool_msg.clone());
			{
				let mut h = history.write().unwrap();
				h.push(tool_msg);
			}
		}

		let follow_up_params = ChatCompletionWithValidationParams::new(
			&conv_messages,
			&effective_model,
			temperature,
			top_p,
			top_k,
			resolved_config.get_effective_max_tokens(),
			&resolved_config,
		)
		.with_max_retries(resolved_config.max_retries)
		.with_cancellation_token(cancel_rx.clone());

		let follow_up = crate::session::chat_completion_with_validation(follow_up_params).await?;
		if *cancel_rx.borrow() {
			anyhow::bail!("Operation cancelled");
		}

		let has_tool_calls = if let Some(ref calls) = follow_up.tool_calls {
			!calls.is_empty()
		} else {
			!crate::mcp::parse_tool_calls(&follow_up.content).is_empty()
		};
		let should_continue =
			crate::session::chat::response::tool_result_processor::check_should_continue(
				&follow_up,
				&resolved_config,
				has_tool_calls,
			);

		current_content = follow_up.content;
		current_exchange = follow_up.exchange;
		current_tool_calls_param = follow_up.tool_calls;

		if !should_continue {
			break;
		}
	}

	// Append the final assistant message to history so resume sees it.
	let final_msg = Message {
		role: "assistant".to_string(),
		content: current_content.clone(),
		timestamp: std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs(),
		cached: false,
		..Default::default()
	};
	{
		let mut h = history.write().unwrap();
		h.push(final_msg);
	}

	Ok(current_content.trim().to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_job_info(j: &TapJobInfo) -> serde_json::Value {
	let started_secs = j
		.started_at
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_secs())
		.unwrap_or(0);
	json!({
		"id": j.id,
		"role": j.role,
		"workdir": j.workdir,
		"status": j.status.as_str(),
		"started_at": started_secs,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn schema_has_required_action() {
		let f = get_tap_function();
		assert_eq!(f.name, "tap");
		let required = f
			.parameters
			.get("required")
			.and_then(|v| v.as_array())
			.expect("required array");
		assert!(required.iter().any(|v| v.as_str() == Some("action")));
	}

	#[test]
	fn schema_advertises_all_actions() {
		let f = get_tap_function();
		let actions = f
			.parameters
			.get("properties")
			.and_then(|p| p.get("action"))
			.and_then(|a| a.get("enum"))
			.and_then(|e| e.as_array())
			.expect("action enum");
		let names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
		assert!(names.contains(&"run"));
		assert!(names.contains(&"list"));
		assert!(names.contains(&"stop"));
		assert!(names.contains(&"discover"));
	}
}
