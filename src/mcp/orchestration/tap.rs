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

//! `tap` core tool — match a task to an agent from the configured taps and run it, message, list and stop runs, and activate capabilities.
//!
//! Exposes actions over a single tool surface:
//!
//! - `run`      — match `prompt` to a tap agent (`tap_route`) and launch it, or
//!   resume an existing run by `session` id. `prompt` is required.
//!   Returns the id immediately and pushes the result to the parent session
//!   inbox when done.
//! - `list`     — show every tap-run in the current session with id, tag,
//!   status, and start time.
//! - `stop`     — cancel a running tap-run by id (sends the cancel watch).
//! - `send`     — queue a user prompt for a running agent's next turn.
//! - `capability` — send a short intent through the same skill/capability
//!   auto-activation path used for user messages.
//!
//! Tap runs are tracked in `crate::session::tap_runs`.

use anyhow::Result;
use serde_json::json;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use tokio::sync::watch;

use crate::acp::driver::run_acp_command;
use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::context::SessionMode;
use crate::session::tap_runs::{self, TapJob, TapJobInfo, TapJobStatus, TapLiveState};

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

pub fn get_tap_function() -> McpFunction {
	McpFunction {
		name: "tap".to_string(),
		freeform: None,
		// The tapped agent may change anything its own tools can.
		annotations: Some(rmcp::model::ToolAnnotations::new().read_only(false)),
		description: r#"Delegate work to installed or session-private tap roles. A role is a pre-built agent persona — its own system prompt, model, and tool kit. Send the task, and the agent that fits it is matched and started for you; you name a role only after this tool has shown you its name. Use this tool to hand off a focused task, check its status, or stop it.

When to use:
- The current task fits an agent better than your generalist context (legal review, docker debugging, financial analysis, …).
- You want to delegate a side task while continuing other work — the agent's reply lands in your next turn on its own; do not poll, re-check or relaunch it, not even once (`list` is not a way to wait for it).
- You want to keep a focused dialog with one agent across multiple turns — keep the returned `id` and pass it back as `session` on subsequent `run` calls.

Important: Every `run` call WITHOUT a `session` id starts a completely fresh agent with zero memory of prior work. If you are continuing, following up, or building on a previous tap call, you MUST pass `session=<id>` from that prior call. Omitting it is ALWAYS wrong when there is prior context to preserve.

Matching:
- `run` without `role` reads only `prompt` to match an agent, so the prompt must name the kind of work and the deliverable. The result names the role that took the task.
- When no agent is matched, `run` returns an error, starts nothing, and lists the closest agents by name. If one of them covers the task, call `run` again with the same prompt and `role` set to that name. If none does, do the task with your own tools or report it. Do not rephrase the same task to force a match.
- Pass `role` only with a name this tool returned: from create, in that list, or as the role of an earlier run. Never guess a name. No action lists every installed agent.

Copies of yourself:
- For a tap role, `role` set to your own name starts a fresh session of you: your prompt and tools, no memory of this conversation. Start several at once, each with a self-contained `prompt`.
- Without `role`, an agent that reaches no other agent gets a copy of itself; one that reaches others matches among them, never itself.
- A copy cannot start copies: it does its part itself.

Actions:
- `create`     — define a private specialist for this session. Required: `name` (lowercase slug), `description` (one line), `system` (its system prompt). Optional: `model`, `tools` (see `tools`). Returns the role tag; start it with `run`. Creating the same name again replaces it. Do not create a role for ordinary one-off delegation.
- `persist`    — save a role created in this session to the personal tap, so later sessions can run and match it. Saving again overwrites the earlier save. Required: `role` (the tag create returned).
- `unpersist`  — delete a role you saved with persist from the personal tap; only roles in your own namespace. Required: `role`.
- `run`        — match an agent to `prompt` and launch it in the background, or resume one. Required: `prompt`. Optional: `role` (start that agent without matching), `readonly` (true: the run may only use tools that cannot change state, and cannot delegate; default false), `session` (resume that run with its original role, workdir and readonly; `readonly: true` is refused for a run started writable), `workdir` (defaults to current cwd).
- `list`       — show every run in this session: id, role, status (running|done|failed|cancelled), start time, workdir.
- `stop`       — cancel a running agent. Required: `session` (the id).
- `send`       — queue a user message for a running agent's next available turn. Required: `session` (the run id) and `prompt`. Keeps the current task running; does not start or resume a run. For a finished run, use `run` with `session` instead. Workflows do not accept prompts.
- `capability` — activate a missing TOOL or skill for yourself: `prompt` is the tool need ("query a Postgres database"). It never finds or starts an agent. Required: `prompt`.
- `workflow`   — run an installed tap workflow (a multi-step, self-verifying job) in the background. Required: `name` + `input`. Without `name`: list installed workflows with descriptions. With `name` but no `input`: nothing runs; returns the workflow definition (its steps and where `{{input}}` lands) so you can compose the input, then call again with `input`. ONLY when the user explicitly asks to run a workflow by name or clearly refers to one — never pick a workflow on your own to solve the current task; use `run` or your own tools for that. A workflow does not contribute to the current turn: it runs detached and its result arrives in a LATER turn like a `run`, so do not wait for it or assume it has finished."#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"enum": ["run", "list", "stop", "send", "capability", "workflow", "create", "persist", "unpersist"],
					"description": "Action to perform"
				},
				"prompt": {
					"type": "string",
					"description": "Prompt for run or send, or capability-need phrase for capability. This text is the delegation brief and the only input used to match an agent. Installed roles run in the same workspace can search authorized ancestor context for omitted details. Make it self-contained: the goal, the concrete facts/names/locations/constraints you already established, what to produce, and what done looks like. Never reference things the agent cannot see ('as discussed', 'the item we found')."
				},
				"role": {
					"type": "string",
					"description": "Agent to start for a new run, by the exact name this tool returned (e.g. 'developer'), or your own name to start a copy of yourself. Optional — omit it and the agent is matched from `prompt`. Ignored when resuming with `session` (the original role is kept). For persist and unpersist: the tag create returned."
				},
				"session": {
					"type": "string",
					"description": "Run id (e.g. 'tap-developer-a3f1c2'). Required for stop or send. For run, supply this to resume an existing run instead of starting fresh."
				},
				"readonly": {
					"type": "boolean",
					"description": "Start the run read-only: it may only use tools that cannot change files, processes, or external systems, and it cannot delegate. Optional, default false. A run keeps the value it started with: with `session`, `true` on a run started writable is an error; omit it."
				},
				"workdir": {
					"type": "string",
					"description": "Working directory the agent operates in. Optional — defaults to the current working directory. Useful when the agent must reason over a different repo or sub-project than the parent. Ignored when resuming with `session` (the original workdir is kept)."
				},
				"name": {
					"type": "string",
					"description": "Lowercase slug for create; the role becomes `<namespace>:<name>`. Tap workflow name for workflow (e.g. 'watch-page'). For workflow, only pass a name the user explicitly asked to run. Omit to list installed workflows; pass it without `input` to inspect the definition."
				},
				"description": {
					"type": "string",
					"description": "Single-line routing description for create."
				},
				"system": {
					"type": "string",
					"description": "System prompt for the created specialist."
				},
				"model": {
					"type": "string",
					"description": "Optional model override for create."
				},
				"tools": {
					"type": "array",
					"items": {"type": "string"},
					"description": "For create: names of tools you can call now, granted to the role. Omitted means no tools. Tools of the orchestration, runtime and local servers, and code, search_tools and context, cannot be granted."
				},
				"input": {
					"type": "string",
					"description": "Input text for workflow — everything the workflow needs (URL, question, tab list); it starts with ZERO other context."
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
		"create" | "persist" | "unpersist" => super::tap_author::execute(call, config).await,
		"stop" => handle_stop(call).await,
		"send" => handle_send(call),
		"capability" => handle_capability(call, config).await,
		"workflow" => handle_workflow(call).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Unknown action '{other}'. Use run, list, stop, send, capability, workflow, create, persist, or unpersist."),
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

/// `workflow`: list tap workflows, show one's definition, or run one as a
/// background tap-run whose result is handed back through the inbox like a
/// agent reply.
async fn handle_workflow(call: &McpToolCall) -> Result<McpToolResult> {
	let param = |key: &str| {
		call.parameters
			.get(key)
			.and_then(|v| v.as_str())
			.map(str::trim)
			.filter(|s| !s.is_empty())
			.map(str::to_string)
	};
	let Some(name) = param("name") else {
		let workflows = crate::workflow::spawn::list_tap_workflows()?;
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			json!({
				"count": workflows.len(),
				"workflows": workflows,
				"usage": "tap(action=\"workflow\", name=<name>, input=<everything the workflow needs>)",
			})
			.to_string(),
		));
	};
	let Some(input) = param("input") else {
		// Name without input = show the definition, so the caller can see
		// where `{{input}}` lands before composing it.
		return Ok(match crate::agent::taps::fetch_workflow(&name) {
			Ok((definition, source_tap)) => McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				json!({
					"name": name,
					"source_tap": source_tap,
					"definition": definition,
					"usage": "Nothing was run. Read where `{{input}}` lands in the steps, then call again with `input` to start it.",
				})
				.to_string(),
			),
			Err(e) => McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("{e:#}")),
		});
	};

	let role = format!("workflow:{name}");
	let id = tap_runs::generate_id(&role);
	let workdir = crate::mcp::get_thread_working_directory().to_string_lossy().to_string();
	let status = Arc::new(RwLock::new(TapJobStatus::Running));
	let (cancel_tx, mut cancel_rx) = watch::channel(false);
	tap_runs::register_job(TapJob {
		id: id.clone(),
		role: role.clone(),
		workdir: workdir.clone(),
		mode: SessionMode::default(),
		started_at: SystemTime::now(),
		status: Arc::clone(&status),
		error: None,
		cancel_tx,
		live: Arc::new(RwLock::new(TapLiveState::default())),
	});

	let id_owned = id.clone();
	let name_owned = name.clone();
	let role_owned = role.clone();
	let status_bg = Arc::clone(&status);
	let session_id = crate::session::context::current_session_id();
	tokio::spawn(async move {
		let run = async move {
			// A latched `stop` drops the runner future, which kills the child.
			let cancelled = async {
				loop {
					if *cancel_rx.borrow() {
						break;
					}
					if cancel_rx.changed().await.is_err() {
						std::future::pending::<()>().await;
					}
				}
			};
			let outcome = tokio::select! {
				r = crate::workflow::spawn::run_tap_workflow(&name_owned, &input) => Some(r),
				_ = cancelled => None,
			};
			let (terminal, content) = workflow_handback(&name_owned, &id_owned, outcome);
			crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
				source: crate::session::inbox::InboxSource::TapRun {
					id: id_owned,
					role: role_owned,
				},
				content,
			});
			if let Ok(mut s) = status_bg.write() {
				if *s == TapJobStatus::Running {
					*s = terminal;
				}
			}
		};
		if let Some(sid) = session_id {
			crate::session::context::with_session_id(sid, run).await;
		} else {
			run.await;
		}
	});
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({
			"id": id,
			"workflow": name,
			"workdir": workdir,
			"message": "Workflow started. Its result will be injected as a user message when ready; `stop` with this id cancels it.",
		})
		.to_string(),
	))
}

/// The handback a finished tap workflow leaves in the inbox. Also banks what the
/// run spent on the calling session: its steps bill the account either way, but
/// only banked spend reaches that session's reported cost.
fn workflow_handback(
	name: &str,
	id: &str,
	outcome: Option<Result<crate::workflow::spawn::RunOutcome>>,
) -> (TapJobStatus, String) {
	match outcome {
		Some(Ok(run)) => {
			if let Some(cost) = run.cost {
				crate::session::external_spend::record(cost);
			}
			(
				TapJobStatus::Done,
				format!(
					"[Workflow '{name}' ({id}) completed]\n\n{}",
					crate::session::inbox::strip_invented_reminders(&run.output)
				),
			)
		}
		Some(Err(e)) => (
			TapJobStatus::Failed,
			format!("[Workflow '{name}' ({id}) failed]\n\n{e:#}"),
		),
		None => (TapJobStatus::Cancelled, format!("[Workflow '{name}' ({id}) cancelled]")),
	}
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

	let activated = crate::mcp::runtime::capability::auto_activate_capabilities_for_intent(&prompt, config).await;

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

fn handle_send(call: &McpToolCall) -> Result<McpToolResult> {
	let error = |message: String| McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), message);
	let session = match call.parameters.get("session") {
		Some(serde_json::Value::String(session)) if !session.trim().is_empty() => session.trim(),
		_ => return Ok(error("session: non-empty string required".into())),
	};
	let prompt = match call.parameters.get("prompt") {
		Some(serde_json::Value::String(prompt)) if !prompt.trim().is_empty() => prompt.clone(),
		_ => return Ok(error("prompt: non-empty string required".into())),
	};
	if let Err(failure) = tap_runs::send_prompt(session, prompt) {
		return Ok(error(failure.to_string()));
	}
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({"id": session, "message": "Prompt queued for the running agent's next available turn."}).to_string(),
	))
}

async fn handle_run(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	use crate::session::delegation_context as delegation;
	let mut prompt = match call.parameters.get("prompt").and_then(|v| v.as_str()) {
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
	let readonly = match call.parameters.get("readonly") {
		None => false,
		Some(serde_json::Value::Bool(readonly)) => *readonly,
		Some(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"readonly: boolean required".to_string(),
			));
		}
	};
	// Default workdir is the parent session's current cwd. Use the thread-local
	// session working directory (not the process cwd, which is wrong under the
	// server/daemon where all sessions share one process). Resolved early so
	// resume picks up the original workdir from the existing job.
	let cwd_default = crate::mcp::get_thread_working_directory().to_string_lossy().to_string();

	// Resolve (id, role, title, workdir, status, cancel_rx) for resume vs. fresh.
	// Conversation history is persisted on disk by the ACP subprocess under
	// the session name `<id>` — we don't track messages in-memory anymore.
	let (id, role, title, workdir, status, cancel_rx, launch, mode) = if let Some(sid) = session {
		// A run keeps the mode it started with: asking for read-only on a writable run is
		// refused instead of resumed writable.
		if readonly && tap_runs::find_job(&sid).is_some_and(|job| !job.mode.readonly) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Run {sid} was started writable and cannot be resumed read-only: start a new run with `readonly` set."
				),
			));
		}
		let claim = match tap_runs::resume_run(&sid) {
			Ok(claim) => claim,
			Err(error) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					error.to_string(),
				));
			}
		};
		(
			sid,
			claim.role,
			None,
			claim.workdir,
			claim.status,
			claim.cancel_rx,
			None,
			claim.mode,
		)
	} else {
		let agent = match super::tap_route::route_task(&prompt, role_param.as_deref(), config).await {
			Ok(agent) => agent,
			Err(error) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					error,
				));
			}
		};
		let mode = SessionMode {
			readonly,
			copy: agent.copy,
		};
		let role = agent.manifest.tag.clone();
		let workdir = workdir_param.unwrap_or(cwd_default);
		let id = tap_runs::generate_id(&role);
		if let Err(error) = agent.manifest.pin(&id) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Cannot pin tap manifest: {error:#}"),
			));
		}
		let status = Arc::new(RwLock::new(TapJobStatus::Running));
		let (cancel_tx, cancel_rx) = watch::channel(false);
		tap_runs::register_job(TapJob {
			id: id.clone(),
			role: role.clone(),
			workdir: workdir.clone(),
			mode,
			started_at: SystemTime::now(),
			status: Arc::clone(&status),
			error: None,
			cancel_tx,
			live: Arc::new(RwLock::new(TapLiveState::default())),
		});
		if let Err(error) = tap_runs::with_run_events(|events, parent| events.begin(parent, &id, &role)) {
			crate::log_error!("Tap run {}: failed to start event stream: {}", id, error);
		}
		let launch = if agent.manifest.authored() {
			None // No implicit parent-context tool or log grant for an authored role.
		} else {
			delegation::DISPATCH.try_with(Clone::clone).ok().flatten()
		};
		(id, role, agent.title, workdir, status, cancel_rx, Some(launch), mode)
	};

	let workdir_path = std::path::PathBuf::from(&workdir);
	let session_id = crate::session::context::current_session_id();
	let context_grant = (|| -> Result<Option<delegation::ActiveGrant>> {
		let Some(parent) = &session_id else { return Ok(None) };
		match launch {
			Some(Some((file, request))) => {
				let workspace = crate::session::context::get_session_workdir_anchor(parent)
					.ok_or_else(|| anyhow::anyhow!("parent workspace unavailable for delegation"))?
					.canonicalize()?;
				if workdir_path.canonicalize()? != workspace {
					return Ok(None);
				}
				let source = delegation::Source::capture(parent, &file, delegation::current_grant().is_some())?;
				let grant = delegation::issue(&id, workspace, source)?;
				let mut context = String::from("\n\nParent conversation is available through the context tool. Search it for earlier instructions, decisions or evidence if needed.");
				if let Some(request) = request {
					context.push_str("\n\nOriginal user request excerpt:\n");
					context.push_str(&request);
				}
				prompt.push_str(&context);
				Ok(Some(grant))
			}
			Some(None) => Ok(None),
			None => delegation::resume(&id, parent, &workdir_path),
		}
	})();
	let context_grant = match context_grant {
		Ok(grant) => grant,
		Err(error) => {
			if let Err(finish_error) = tap_runs::finish_run(&id, &status, TapJobStatus::Failed, Some(error.to_string()))
			{
				crate::log_error!("Tap run {}: failed to publish terminal status: {}", id, finish_error);
			}
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				error.to_string(),
			));
		}
	};

	// Resolve the path to the currently-running octomind binary so the
	// subprocess uses the same code, regardless of $PATH.
	let exe = match std::env::current_exe() {
		Ok(p) => p.to_string_lossy().to_string(),
		Err(e) => {
			let error = format!("Failed to locate octomind binary: {e:#}");
			crate::log_error!("Tap run {} ({}) failed: {}", id, role, error);
			if let Err(error) = tap_runs::finish_run(&id, &status, TapJobStatus::Failed, Some(error.clone())) {
				crate::log_error!("Tap run {}: failed to publish terminal status: {}", id, error);
			}
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				error,
			));
		}
	};
	// `--name <id>` creates a fresh session if `<id>.jsonl` doesn't exist
	// and resumes it if it does — works for both the first call and
	// every subsequent turn against the same tap-run id.
	let mut acp_args: Vec<String> = vec![
		"acp".to_string(),
		role.clone(),
		"--name".to_string(),
		id.clone(),
		"--bound-tap".into(),
	];
	if mode.readonly {
		acp_args.push("--readonly".to_string());
	}
	if mode.copy {
		// Tells the child it is a copy, so it cannot start copies of itself.
		acp_args.push("--copy".to_string());
	}
	let prompts = match session_id
		.as_ref()
		.map(|_| tap_runs::open_prompt_channel(&id))
		.transpose()
	{
		Ok(prompts) => prompts,
		Err(error) => {
			if let Err(finish_error) = tap_runs::finish_run(&id, &status, TapJobStatus::Failed, Some(error.to_string()))
			{
				crate::log_error!("Tap run {}: failed to publish terminal status: {}", id, finish_error);
			}
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				error.to_string(),
			));
		}
	};

	let id_owned = id.clone();
	let role_owned = role.clone();
	let status_bg = Arc::clone(&status);
	let grant_id = context_grant.as_ref().map(|grant| grant.id);
	tokio::spawn(async move {
		let run = async move {
			let _active_grant = context_grant;
			let arg_refs: Vec<&str> = acp_args.iter().map(|s| s.as_str()).collect();
			let outcome = run_acp_command(
				&exe,
				&arg_refs,
				&prompt,
				&workdir_path,
				cancel_rx,
				prompts.map(|prompts| tap_runs::TapRunInput {
					id: &id_owned,
					prompts,
					context_grant: grant_id,
				}),
				true,
			)
			.await;
			let error = outcome
				.as_ref()
				.err()
				.filter(|e| !crate::session::cancellation::is_cancelled(e))
				.map(|e| format!("{e:#}"));
			if let Some(error) = &error {
				crate::log_error!("Tap run {} ({}) failed: {}", id_owned, role_owned, error);
			}
			let (terminal, content) = match outcome {
				Ok(text) => (
					TapJobStatus::Done,
					format!(
						"[Tap-run '{id_owned}' ({role_owned}) completed]\n\n{}",
						crate::session::inbox::strip_invented_reminders(&text)
					),
				),
				Err(e) if crate::session::cancellation::is_cancelled(&e) => (
					TapJobStatus::Cancelled,
					format!("[Tap-run '{id_owned}' ({role_owned}) cancelled]"),
				),
				Err(e) => (
					TapJobStatus::Failed,
					format!(
						"[Tap-run '{id_owned}' ({role_owned}) failed]\n\n{e:#}\n\nDo not relaunch this delegation unchanged: complete the task with the tools you have, or report the blocker."
					),
				),
			};
			crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
				source: crate::session::inbox::InboxSource::TapRun {
					id: id_owned.clone(),
					role: role_owned,
				},
				content,
			});
			// Queue the handback before clearing Running. Graceful session shutdown
			// can therefore never observe neither the job nor its result.
			if let Err(error) = tap_runs::finish_run(&id_owned, &status_bg, terminal, error) {
				crate::log_error!("Tap run {}: failed to publish terminal status: {}", id_owned, error);
			}
		};
		if let Some(sid) = session_id {
			crate::session::context::with_session_id(sid, run).await;
		} else {
			run.await;
		}
	});
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		{
			let mut started = json!({
				"id": id,
				"role": role,
				"workdir": workdir,
				"context": if grant_id.is_some() { "available" } else { "unavailable" },
				"message": "Tap-run started. Reply will be injected as a user message when ready.",
			});
			// What the matched agent does, so a wrong match is visible and can be stopped.
			if let Some(title) = title {
				started["title"] = json!(title);
			}
			if mode.readonly {
				started["readonly"] = json!(true);
			}
			if mode.copy {
				started["copy"] = json!(true);
			}
			started
		}
		.to_string(),
	))
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
	let mut info = json!({
		"id": j.id,
		"role": j.role,
		"workdir": j.workdir,
		"status": j.status.as_str(),
		"started_at": started_secs,
	});
	if let Some(error) = &j.error {
		info["error"] = json!(error);
	}
	info
}

#[cfg(test)]
#[path = "tap_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "tap_unit_tests.rs"]
mod unit_tests;
