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

//! `tap` core tool — run, list, stop, discover agents, and request runtime capability activation from configured taps.
//!
//! Exposes five actions over a single tool surface:
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
//! - `capability` — send a short intent through the same skill/capability
//!   auto-activation path used for user messages.
//!
//! Tap-runs are tracked in `crate::session::tap_runs` — a registry that is
//! intentionally separate from `BackgroundJobManager` (which tracks
//! `agent_*`). The two subsystems share only generic primitives (tokio
//! tasks, `watch::Sender` cancellation, the embedding matcher).

use anyhow::Result;
use serde_json::json;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::watch;

use crate::config::Config;
use crate::mcp::agent::functions::run_acp_command;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::tap_runs::{self, TapJob, TapJobInfo, TapJobStatus};

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

Important: Every `run` call WITHOUT a `session` id starts a completely fresh agent with zero memory of prior work. If you are continuing, following up, or building on a previous tap call, you MUST pass `session=<id>` from that prior call. Omitting it is ALWAYS wrong when there is prior context to preserve.

Discovery flow:
- ALWAYS start with `tap(action="discover", intent="<plain-English need>")` unless the role was already returned by a previous `discover` or `list` call in this session. Never guess a role name from context, documentation, or examples — role names are only valid after `discover` confirms they exist.
- After `discover` returns matches, pick the best-fit role from the results, then call `run` with that exact role string.
- If needed tools, skills, or capabilities are missing: `tap(action="capability", prompt="<underlying capability need>")` triggers the same auto-activation checks used for user messages.

Actions:
- `run`        — launch a role. Required: `role` (for new runs) OR `session` (to resume), plus `prompt`. Optional: `workdir` (defaults to current cwd), `background` (default false; true = return immediately, reply injected later). **Always supply `session` when continuing an existing run — omitting it discards all prior context.**
- `list`       — show every run in this session: id, role, status (running|done|failed|cancelled), start time, workdir.
- `stop`       — cancel a running specialist. Required: `session` (the id).
- `discover`   — find roles matching free-text intent. Required: `intent`. Returns top matches with title, description, and source tap.
- `capability` — trigger skill/capability auto-activation. Required: `prompt`."#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"enum": ["run", "list", "stop", "discover", "capability"],
					"description": "Action to perform"
				},
				"role": {
					"type": "string",
					"description": "Specialist role to launch, e.g. 'developer:general'. Required for run when `session` is not given. Use `discover` first if unsure which role fits."
				},
				"prompt": {
					"type": "string",
					"description": "Prompt for run, or capability-need phrase for capability."
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
		"capability" => handle_capability(call, config).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Unknown action '{other}'. Use run, list, stop, discover, or capability."),
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

async fn handle_capability(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
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

	let activated =
		crate::mcp::core::capability::auto_activate_capabilities_for_intent(&prompt, config).await;

	let content = if activated.is_empty() {
		json!({
			"activated_capabilities": [],
			"message": "No capability matched the prompt."
		})
		.to_string()
	} else {
		json!({
			"activated_capabilities": activated,
			"message": "Capability auto-activation completed."
		})
		.to_string()
	};

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		content,
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

	// Resolve (id, role, workdir, status, cancel_rx) for resume vs. fresh.
	// Conversation history is persisted on disk by the ACP subprocess under
	// the session name `<id>` — we don't track messages in-memory anymore.
	let (id, role, workdir, status, cancel_rx) = if let Some(sid) = session {
		let (status, cancel_rx) = match tap_runs::get_status_and_cancel(&sid) {
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
		(sid, info.role, info.workdir, status, cancel_rx)
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
		let status = Arc::new(RwLock::new(TapJobStatus::Running));
		let (cancel_tx, cancel_rx) = watch::channel(false);
		tap_runs::register_job(TapJob {
			id: id.clone(),
			role: role.clone(),
			workdir: workdir.clone(),
			started_at: SystemTime::now(),
			status: Arc::clone(&status),
			cancel_tx,
		});
		(id, role, workdir, status, cancel_rx)
	};

	// Resolve the path to the currently-running octomind binary so the
	// subprocess uses the same code, regardless of $PATH.
	let exe = match std::env::current_exe() {
		Ok(p) => p.to_string_lossy().to_string(),
		Err(e) => {
			if let Ok(mut s) = status.write() {
				if *s == TapJobStatus::Running {
					*s = TapJobStatus::Failed;
				}
			}
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to locate octomind binary: {e:#}"),
			));
		}
	};
	// `--name <id>` creates a fresh session if `<id>.jsonl` doesn't exist
	// and resumes it if it does — works for both the first call and
	// every subsequent turn against the same tap-run id.
	let acp_args: Vec<String> = vec![
		"acp".to_string(),
		role.clone(),
		"--name".to_string(),
		id.clone(),
	];
	let workdir_path = std::path::PathBuf::from(&workdir);

	if background {
		let id_owned = id.clone();
		let role_owned = role.clone();
		let workdir_owned = workdir_path.clone();
		let prompt_owned = prompt.clone();
		let exe_owned = exe.clone();
		let args_owned = acp_args.clone();
		let status_bg = Arc::clone(&status);
		let session_id = crate::session::context::current_session_id();
		tokio::spawn(async move {
			let run = async move {
				let arg_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();
				let outcome = run_acp_command(
					&exe_owned,
					&arg_refs,
					&prompt_owned,
					&workdir_owned,
					cancel_rx,
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

	// Foreground — block until the ACP subprocess completes the prompt.
	let arg_refs: Vec<&str> = acp_args.iter().map(|s| s.as_str()).collect();
	let outcome = run_acp_command(&exe, &arg_refs, &prompt, &workdir_path, cancel_rx).await;
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
		assert!(names.contains(&"capability"));
	}
}
