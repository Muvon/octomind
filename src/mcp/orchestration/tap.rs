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
//!   Returns the id immediately and pushes the result to the parent session
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
use crate::session::tap_runs::{self, TapJob, TapJobInfo, TapJobStatus, TapLiveState};

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

pub fn get_tap_function() -> McpFunction {
	McpFunction {
		name: "tap".to_string(),
		description: r#"Delegate work to specialist roles installed via taps. A role is a pre-built agent persona — its own system prompt, model, and tool kit — identified by `category:variant` (e.g. `developer:general`, `lawyer:us`, `security:owasp`). Use this tool to hand off a focused task, watch its progress, stop it, or browse the catalog.

When to use:
- The current task fits a specialist better than your generalist context (legal review, docker debugging, financial analysis, …).
- You want to delegate a side task while continuing other work — the specialist's reply lands in your next turn.
- You want to keep a focused dialog with one specialist across multiple turns — keep the returned `session` id and pass it back on subsequent `run` calls.

Important: Every `run` call WITHOUT a `session` id starts a completely fresh agent with zero memory of prior work. If you are continuing, following up, or building on a previous tap call, you MUST pass `session=<id>` from that prior call. Omitting it is ALWAYS wrong when there is prior context to preserve.

Discovery flow:
- ALWAYS start with `tap(action="discover", intent="<plain-English need>")` unless the role was already returned by a previous `discover` or `list` call in this session. Never guess a role name from context, documentation, or examples — role names are only valid after `discover` confirms they exist.
- After `discover` returns matches, pick the best-fit role from the results, then call `run` with that exact role string.
- If needed tools, skills, or capabilities are missing: `tap(action="capability", prompt="<underlying capability need>")` triggers the same auto-activation checks used for user messages.

Actions:
- `run`        — launch a role in the background. Required: `role` (for new runs) OR `session` (to resume), plus `prompt`. Optional: `workdir` (defaults to current cwd). **Always supply `session` when continuing an existing run — omitting it discards all prior context.**
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
					"description": "Prompt for run, or capability-need phrase for capability. A new run starts with ZERO context — the specialist sees ONLY this text, none of your conversation or findings. Make it self-contained: the goal, the concrete facts/names/locations/constraints you already established, what to produce, and what done looks like. Never reference things the specialist cannot see ('as discussed', 'the item we found')."
				},
				"session": {
					"type": "string",
					"description": "Run id (e.g. 'tap-developer-general-a3f1c2'). Required for stop. For run, supply this to resume an existing run instead of starting fresh."
				},
				"workdir": {
					"type": "string",
					"description": "Working directory the specialist operates in. Optional — defaults to the current working directory. Useful when the specialist must reason over a different repo or sub-project than the parent."
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
		crate::mcp::runtime::capability::auto_activate_capabilities_for_intent(&prompt, config)
			.await;

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
	// Default workdir is the parent session's current cwd. Use the thread-local
	// session working directory (not the process cwd, which is wrong under the
	// server/daemon where all sessions share one process). Resolved early so
	// resume picks up the original workdir from the existing job.
	let cwd_default = crate::mcp::get_thread_working_directory()
		.to_string_lossy()
		.to_string();

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
		// Fresh cancel channel: a prior `stop` latched the old one to `true`, so
		// the subscribed receiver would cancel this resumed turn immediately.
		let cancel_rx = tap_runs::reset_cancel(&sid).unwrap_or(cancel_rx);
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
			live: Arc::new(RwLock::new(TapLiveState::default())),
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

	let id_owned = id.clone();
	let role_owned = role.clone();
	let status_bg = Arc::clone(&status);
	let session_id = crate::session::context::current_session_id();
	tokio::spawn(async move {
		let run = async move {
			let arg_refs: Vec<&str> = acp_args.iter().map(|s| s.as_str()).collect();
			let outcome = run_acp_command(
				&exe,
				&arg_refs,
				&prompt,
				&workdir_path,
				cancel_rx,
				Some(&id_owned),
				true,
			)
			.await;
			let (terminal, content) = match outcome {
				Ok(text) => (
					TapJobStatus::Done,
					format!("[Tap-run '{id_owned}' ({role_owned}) completed]\n\n{text}"),
				),
				Err(e) if crate::session::cancellation::is_cancelled(&e) => (
					TapJobStatus::Cancelled,
					format!("[Tap-run '{id_owned}' ({role_owned}) cancelled]"),
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
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		json!({
			"id": id,
			"role": role,
			"workdir": workdir,
			"message": "Tap-run started. Reply will be injected as a user message when ready.",
		})
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
	use serial_test::serial;

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

	#[test]
	fn schema_does_not_expose_background_choice() {
		let f = get_tap_function();
		let properties = f
			.parameters
			.get("properties")
			.and_then(|p| p.as_object())
			.expect("properties object");
		assert!(!properties.contains_key("background"));
	}

	// -------------------------------------------------------------------------
	// Command dispatch, parameter validation, and job-registry lifecycle
	// -------------------------------------------------------------------------

	fn tap_call(params: serde_json::Value) -> McpToolCall {
		McpToolCall {
			tool_name: "tap".to_string(),
			parameters: params,
			tool_id: "t-tap".to_string(),
		}
	}

	fn test_config() -> Config {
		let mut config: Config =
			toml::from_str(include_str!("../../../config-templates/default.toml"))
				.expect("parse default config template");
		config.build_role_map();
		config
	}

	/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir so tap enumeration sees an
	/// empty tap set. Tests using it must be `#[serial]` (env is process-global).
	struct DataDirGuard {
		previous: Option<std::ffi::OsString>,
		_dir: tempfile::TempDir,
	}

	impl DataDirGuard {
		fn new() -> Self {
			let previous = std::env::var_os("OCTOMIND_DATA_DIR");
			let dir = tempfile::tempdir().expect("failed to create tempdir");
			std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
			Self {
				previous,
				_dir: dir,
			}
		}
	}

	impl Drop for DataDirGuard {
		fn drop(&mut self) {
			match self.previous.take() {
				Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
				None => std::env::remove_var("OCTOMIND_DATA_DIR"),
			}
		}
	}

	/// Register a job for the CURRENT session — must be called inside a
	/// `with_session_id` scope, matching how `handle_run` registers real jobs.
	fn register_test_job(id: &str, role: &str, status: TapJobStatus, started_at: SystemTime) {
		let (cancel_tx, _cancel_rx) = watch::channel(false);
		tap_runs::register_job(TapJob {
			id: id.to_string(),
			role: role.to_string(),
			workdir: "/tmp".to_string(),
			started_at,
			status: Arc::new(RwLock::new(status)),
			cancel_tx,
			live: Arc::new(RwLock::new(TapLiveState::default())),
		});
	}

	#[test]
	fn schema_description_documents_session_resume() {
		let f = get_tap_function();
		assert!(!f.description.is_empty());
		// The resume contract ("always pass `session` back") must stay documented.
		assert!(f.description.contains("session"));
	}

	#[tokio::test]
	async fn dispatch_missing_action_is_error() {
		let config = test_config();
		let result = execute_tap_command(&tap_call(json!({})), &config)
			.await
			.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing required parameter 'action'"));
	}

	#[tokio::test]
	async fn dispatch_blank_or_non_string_action_is_error() {
		let config = test_config();
		for params in [json!({"action": "   "}), json!({"action": 42})] {
			let result = execute_tap_command(&tap_call(params), &config)
				.await
				.expect("dispatch");
			assert!(result.is_error());
			assert!(result
				.extract_content()
				.contains("Missing required parameter 'action'"));
		}
	}

	#[tokio::test]
	async fn dispatch_unknown_action_is_error() {
		let config = test_config();
		let result = execute_tap_command(&tap_call(json!({"action": "explode"})), &config)
			.await
			.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Unknown action 'explode'"));
	}

	#[tokio::test]
	async fn list_without_session_reports_no_runs() {
		let config = test_config();
		let result = execute_tap_command(&tap_call(json!({"action": "list"})), &config)
			.await
			.expect("dispatch");
		assert!(!result.is_error());
		assert_eq!(result.extract_content(), "No tap-runs in this session.");
	}

	#[tokio::test]
	#[serial]
	async fn list_returns_registered_jobs_newest_first() {
		let config = test_config();
		let sid = "__taptest_list".to_string();
		let out = crate::session::context::with_session_id(sid.clone(), async {
			register_test_job(
				"tap-list-older",
				"developer:general",
				TapJobStatus::Failed,
				SystemTime::UNIX_EPOCH,
			);
			register_test_job(
				"tap-list-newer",
				"lawyer:us",
				TapJobStatus::Done,
				SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10),
			);
			execute_tap_command(&tap_call(json!({"action": "list"})), &config)
				.await
				.expect("dispatch")
		})
		.await;
		tap_runs::clear_for_session(&sid);

		assert!(!out.is_error());
		let payload: serde_json::Value =
			serde_json::from_str(&out.extract_content()).expect("list payload is JSON");
		assert_eq!(payload["count"].as_u64(), Some(2));
		let runs = payload["runs"].as_array().expect("runs array");
		assert_eq!(runs.len(), 2);
		// Newest first, with the full job-info shape.
		assert_eq!(runs[0]["id"].as_str(), Some("tap-list-newer"));
		assert_eq!(runs[0]["role"].as_str(), Some("lawyer:us"));
		assert_eq!(runs[0]["status"].as_str(), Some("done"));
		assert_eq!(runs[1]["id"].as_str(), Some("tap-list-older"));
		assert_eq!(runs[1]["status"].as_str(), Some("failed"));
	}

	#[tokio::test]
	async fn stop_requires_session_param() {
		let config = test_config();
		for params in [
			json!({"action": "stop"}),
			json!({"action": "stop", "session": "  "}),
		] {
			let result = execute_tap_command(&tap_call(params), &config)
				.await
				.expect("dispatch");
			assert!(result.is_error());
			assert!(result
				.extract_content()
				.contains("Missing required parameter 'session'"));
		}
	}

	#[tokio::test]
	async fn stop_unknown_session_is_error() {
		let config = test_config();
		let result = execute_tap_command(
			&tap_call(json!({"action": "stop", "session": "tap-ghost-000000"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("No tap-run with id 'tap-ghost-000000'"));
	}

	#[tokio::test]
	#[serial]
	async fn stop_running_job_cancels_it() {
		let config = test_config();
		let sid = "__taptest_stop_running".to_string();
		let out = crate::session::context::with_session_id(sid.clone(), async {
			register_test_job(
				"tap-stop-me",
				"developer:general",
				TapJobStatus::Running,
				SystemTime::UNIX_EPOCH,
			);
			execute_tap_command(
				&tap_call(json!({"action": "stop", "session": "tap-stop-me"})),
				&config,
			)
			.await
			.expect("dispatch")
		})
		.await;
		tap_runs::clear_for_session(&sid);

		assert!(!out.is_error());
		let payload: serde_json::Value =
			serde_json::from_str(&out.extract_content()).expect("stop payload is JSON");
		assert_eq!(payload["id"].as_str(), Some("tap-stop-me"));
		assert_eq!(payload["status"].as_str(), Some("cancelled"));
	}

	#[tokio::test]
	#[serial]
	async fn stop_finished_job_reports_terminal_status() {
		let config = test_config();
		let sid = "__taptest_stop_done".to_string();
		let out = crate::session::context::with_session_id(sid.clone(), async {
			register_test_job(
				"tap-already-done",
				"developer:general",
				TapJobStatus::Done,
				SystemTime::UNIX_EPOCH,
			);
			execute_tap_command(
				&tap_call(json!({"action": "stop", "session": "tap-already-done"})),
				&config,
			)
			.await
			.expect("dispatch")
		})
		.await;
		tap_runs::clear_for_session(&sid);

		assert!(!out.is_error());
		let payload: serde_json::Value =
			serde_json::from_str(&out.extract_content()).expect("stop payload is JSON");
		assert_eq!(payload["status"].as_str(), Some("done"));
	}

	#[tokio::test]
	async fn discover_requires_intent() {
		let config = test_config();
		for params in [
			json!({"action": "discover"}),
			json!({"action": "discover", "intent": "  "}),
		] {
			let result = execute_tap_command(&tap_call(params), &config)
				.await
				.expect("dispatch");
			assert!(result.is_error());
			assert!(result
				.extract_content()
				.contains("Missing required parameter 'intent'"));
		}
	}

	#[tokio::test]
	#[serial]
	async fn discover_with_no_taps_installed_succeeds() {
		let _guard = DataDirGuard::new();
		let config = test_config();
		let result = execute_tap_command(
			&tap_call(json!({"action": "discover", "intent": "review a contract"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(!result.is_error());
		assert_eq!(result.extract_content(), "No tap agents installed.");
	}

	#[tokio::test]
	async fn capability_requires_prompt() {
		let config = test_config();
		for params in [
			json!({"action": "capability"}),
			json!({"action": "capability", "prompt": "   "}),
		] {
			let result = execute_tap_command(&tap_call(params), &config)
				.await
				.expect("dispatch");
			assert!(result.is_error());
			assert!(result
				.extract_content()
				.contains("Missing required parameter 'prompt'"));
		}
	}

	#[tokio::test]
	async fn run_requires_prompt() {
		let config = test_config();
		let result = execute_tap_command(
			&tap_call(json!({"action": "run", "role": "developer:general"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing required parameter 'prompt'"));
	}

	#[tokio::test]
	async fn run_requires_role_or_session() {
		let config = test_config();
		let result = execute_tap_command(
			&tap_call(json!({"action": "run", "prompt": "do work"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("Missing 'role' for new run"));
	}

	#[tokio::test]
	async fn run_with_unknown_session_is_error() {
		let config = test_config();
		let result = execute_tap_command(
			&tap_call(json!({"action": "run", "prompt": "hi", "session": "tap-ghost-000000"})),
			&config,
		)
		.await
		.expect("dispatch");
		assert!(result.is_error());
		assert!(result
			.extract_content()
			.contains("No tap-run with id 'tap-ghost-000000'"));
	}

	#[test]
	fn format_job_info_contains_all_fields() {
		let info = TapJobInfo {
			id: "tap-x-000001".to_string(),
			role: "developer:general".to_string(),
			workdir: "/tmp/proj".to_string(),
			started_at: SystemTime::UNIX_EPOCH,
			status: TapJobStatus::Done,
			live: TapLiveState::default(),
		};
		let v = format_job_info(&info);
		assert_eq!(v["id"].as_str(), Some("tap-x-000001"));
		assert_eq!(v["role"].as_str(), Some("developer:general"));
		assert_eq!(v["workdir"].as_str(), Some("/tmp/proj"));
		assert_eq!(v["status"].as_str(), Some("done"));
		assert_eq!(v["started_at"].as_u64(), Some(0));
	}
}
