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

// Tool execution module - handles ordered tool execution, display, and error handling
// Unified interface for both main sessions and layers

use crate::config::Config;
use crate::log_info;
use crate::mcp::{McpToolCall, McpToolResult};
use crate::plugins::payload::{self, ContextParts};
use crate::plugins::protocol::{
	event, BackgroundOutput, Returned, SubagentRuns, ToolAfter, ToolBefore, ToolOutcome, TreeChange,
};
use crate::session::chat::session::ChatSession;
use crate::session::chat::ToolProcessor;
use crate::session::output::OutputMode;
use anyhow::Result;
use colored::Colorize;
use std::collections::HashMap;

/// The `stopping` value of a `tool.after` event whose round reaches the hard stop.
const STOPPING_LOOP: &str = "loop";

/// What a plugin writes where the path of the saved original belongs.
const ORIGINAL_PATH_PLACEHOLDER: &str = "{original_path}";

/// What a tool itself returned for a call, kept until the round's results are
/// finished: the repeat check and the size cap change what the agent is shown.
struct OwnAnswer {
	error: bool,
	/// Fingerprint of the whole text, for the repeat stop.
	digest: String,
	/// Its error flag and first characters, kept only when a plugin asked.
	returned: Option<Returned>,
}

impl OwnAnswer {
	fn of(result: &McpToolResult, returned_chars: Option<usize>) -> Self {
		Self {
			error: result.is_error(),
			digest: payload::result_hash(&result.extract_content()),
			returned: returned_chars.map(|limit| payload::returned_by_tool(result, limit)),
		}
	}
}

/// The calls of a finished round with what is known about each besides its result.
struct RoundCalls<'a> {
	calls: &'a [McpToolCall],
	/// Per call: a rule denied it, so its result is the reason.
	denied: &'a [bool],
	own_answers: &'a HashMap<String, OwnAnswer>,
}

/// Why a tool round ends the turn instead of asking the model again.
pub enum RoundStop {
	/// The hard stop: this many tool rounds in a row repeated an earlier one.
	RepeatedRounds(usize),
	/// A plugin ended the turn with this notice.
	Plugin(String),
}

/// Context for tool execution - can be either main session or layer context
pub enum ToolExecutionContext<'a> {
	/// Main session context with full session access
	MainSession {
		chat_session: &'a mut ChatSession,
		tool_processor: &'a mut ToolProcessor,
		/// Assistant text accompanying this batch, or its thinking when it wrote
		/// none: the nearest available explanation of why these calls were
		/// issued now.
		tool_round_intent: &'a str,
		/// Set by the round when the plugin path ends the turn after it.
		round_stop: Option<RoundStop>,
	},
	/// Layer/agent context — tool access controlled by the ACP session's role config
	Layer { session_name: String, layer_name: String },
	/// Deferred tool execution retains its launched tap identity before output is capped.
	Scheduled {
		session_name: String,
		tap_run: Option<String>,
	},
}

impl ToolExecutionContext<'_> {
	/// Get session name for logging
	pub fn session_name(&self) -> &str {
		match self {
			ToolExecutionContext::MainSession { chat_session, .. } => &chat_session.session.info.name,
			ToolExecutionContext::Layer { session_name, .. } | ToolExecutionContext::Scheduled { session_name, .. } => {
				session_name
			}
		}
	}

	/// Get execution context for display (None for main session, Some(context) for layers/agents)
	pub fn execution_context(&self) -> Option<String> {
		match self {
			ToolExecutionContext::MainSession { .. } => None, // No suffix for main session
			ToolExecutionContext::Layer { layer_name, .. } => Some(layer_name.clone()),
			ToolExecutionContext::Scheduled { .. } => Some("schedule".to_string()),
		}
	}

	/// Check if tool is allowed in this context.
	/// Tool access for layers/agents is controlled by the ACP session's role config.
	pub fn is_tool_allowed(&self, _tool_name: &str) -> bool {
		true
	}

	/// Get error tracker (if available)
	pub fn error_tracker(&mut self) -> Option<&mut crate::session::chat::tool_error_tracker::ToolErrorTracker> {
		match self {
			ToolExecutionContext::MainSession { tool_processor, .. } => Some(&mut tool_processor.error_tracker),
			ToolExecutionContext::Layer { .. } | ToolExecutionContext::Scheduled { .. } => None,
		}
	}

	/// Increment tool call counter. Also the single funnel every tool execution
	/// passes through, so it is where per-tool telemetry is counted.
	pub fn increment_tool_calls(&mut self, tool_name: &str) {
		crate::telemetry::record_tool(tool_name);
		if let ToolExecutionContext::MainSession { chat_session, .. } = self {
			chat_session.session.info.tool_calls += 1;
		}
	}

	fn capture_tap_run(&mut self, call: &crate::mcp::McpToolCall, result: &crate::mcp::McpToolResult) {
		let Self::Scheduled { tap_run, .. } = self else {
			return;
		};
		if call.tool_name == "tap"
			&& !result.is_error()
			&& matches!(
				call.parameters.get("action").and_then(|value| value.as_str()),
				Some("run" | "workflow")
			) {
			*tap_run = serde_json::from_str::<serde_json::Value>(&result.extract_content())
				.ok()
				.and_then(|value| value.get("id").and_then(|id| id.as_str()).map(str::to_owned));
		}
	}
}

/// Execute tools in emitted order, allowing consecutive read-only calls to overlap.
pub async fn execute_tools_in_context(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	context: &mut ToolExecutionContext<'_>,
	config: &Config,
	operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	mode: OutputMode,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	// Filter tools based on context permissions
	let allowed_tool_calls: Vec<_> = current_tool_calls
		.into_iter()
		.filter(|tool_call| {
			if context.is_tool_allowed(&tool_call.tool_name) {
				true
			} else {
				if !mode.should_suppress_cli_output() {
					println!(
						"{} {} {}",
						"Tool".red(),
						tool_call.tool_name,
						"not allowed in this context".red()
					);
				}
				false
			}
		})
		.collect();

	if allowed_tool_calls.is_empty() {
		return Ok((Vec::new(), 0));
	}

	execute_tools_with_context(allowed_tool_calls, context, config, operation_cancelled, mode).await
}

// Legacy main-session interface; execution order follows the same barriers as layers.
pub async fn execute_tools_parallel(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	tool_round_intent: &str,
	chat_session: &mut ChatSession,
	config: &Config,
	tool_processor: &mut ToolProcessor,
	operation_cancelled: tokio::sync::watch::Receiver<bool>,
	mode: OutputMode,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64, Option<RoundStop>)> {
	if current_tool_calls.len() == 1 && is_tap_capability_call(&current_tool_calls[0]) {
		if *operation_cancelled.borrow() {
			return Ok((Vec::new(), 0, None));
		}
		let tree_before = plugin_tree_fingerprint(config);
		let session_name = chat_session.session.info.name.clone();
		let blocks = admit_tool_batch(
			&session_name,
			config,
			&current_tool_calls,
			CallOrigin::Model(Some(&mut *chat_session)),
		)
		.await;
		if *operation_cancelled.borrow() {
			return Ok((Vec::new(), 0, None));
		}
		chat_session.session.info.tool_calls += 1;
		crate::telemetry::record_tool(&current_tool_calls[0].tool_name);
		if let Some(message) = blocks.into_iter().next().flatten() {
			let call = &current_tool_calls[0];
			return Ok((
				vec![crate::mcp::McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					message,
				)],
				0,
				None,
			));
		}
		let (result, elapsed_ms) = execute_tap_capability_inline(&current_tool_calls[0], chat_session, config).await;
		// Hard cap first: plugins see (and may replace) what the agent would
		// actually receive.
		let mut results = handle_large_tool_results(vec![result], config, mode).await?;
		let round_stop = plugin_round(
			chat_session,
			config,
			&RoundCalls {
				calls: &current_tool_calls,
				denied: &[false],
				own_answers: &HashMap::new(),
			},
			&mut results,
			tool_round_intent,
			tree_before,
		)
		.await;
		return Ok((results, elapsed_ms, round_stop));
	}

	let mut context = ToolExecutionContext::MainSession {
		chat_session,
		tool_processor,
		tool_round_intent,
		round_stop: None,
	};

	let result = execute_tools_in_context(
		current_tool_calls.clone(),
		&mut context,
		config,
		Some(operation_cancelled),
		mode,
	)
	.await;

	let ToolExecutionContext::MainSession { round_stop, .. } = context else {
		unreachable!("the context was built as a main session above");
	};
	result.map(|(results, elapsed_ms)| (results, elapsed_ms, round_stop))
}

fn is_tap_capability_call(call: &crate::mcp::McpToolCall) -> bool {
	call.tool_name == "tap"
		&& call
			.parameters
			.get("action")
			.and_then(|v| v.as_str())
			.is_some_and(|action| action == "capability")
}

async fn execute_tap_capability_inline(
	call: &crate::mcp::McpToolCall,
	chat_session: &mut ChatSession,
	config: &Config,
) -> (crate::mcp::McpToolResult, u64) {
	let started = std::time::Instant::now();
	let prompt = match call.parameters.get("prompt").and_then(|v| v.as_str()) {
		Some(p) if !p.trim().is_empty() => p.trim().to_string(),
		_ => {
			return (
				crate::mcp::McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Missing required parameter 'prompt'.".to_string(),
				),
				started.elapsed().as_millis() as u64,
			);
		}
	};

	let session_id = crate::session::context::current_session_id();
	let skills_before = session_id
		.as_ref()
		.map(crate::session::context::get_active_skills)
		.unwrap_or_default();
	let capabilities_before = crate::mcp::runtime::capability::list_active_names();

	let workdir = crate::mcp::workdir::get_thread_working_directory();
	crate::mcp::runtime::skill_auto::run_activation(&prompt, &workdir, chat_session).await;
	let mut activated_capabilities =
		crate::mcp::runtime::capability::auto_activate_capabilities_for_intent(&prompt, config).await;

	let activated_skills = session_id
		.as_ref()
		.map(crate::session::context::get_active_skills)
		.unwrap_or_default()
		.into_iter()
		.filter(|name| !skills_before.contains(name))
		.collect::<Vec<_>>();

	for name in crate::mcp::runtime::capability::list_active_names() {
		if !capabilities_before.contains(&name) && !activated_capabilities.contains(&name) {
			activated_capabilities.push(name);
		}
	}

	let message = if activated_skills.is_empty() && activated_capabilities.is_empty() {
		"No skill or capability matched the prompt."
	} else {
		"Skill/capability auto-activation completed."
	};

	let result = crate::mcp::McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		serde_json::json!({
			"activated_skills": activated_skills,
			"activated_capabilities": activated_capabilities,
			"message": message,
		})
		.to_string(),
	);

	(result, started.elapsed().as_millis() as u64)
}

// Implementation that works with execution context
/// Shared admission, including inline capability calls. Native prechecks are
/// previewed before the model, then committed against the final allowed batch.
pub(crate) async fn admit_tool_batch(
	session_id: &str,
	config: &Config,
	calls: &[crate::mcp::McpToolCall],
	origin: CallOrigin<'_>,
) -> Vec<Option<String>> {
	let session_id = session_id.to_string();
	let plugin_judges = crate::plugins::subscribed(&session_id, event::TOOL_BEFORE);
	let mut plugin_denials = HashMap::new();
	let messages = if plugin_judges {
		let preview = crate::session::guardrails::preview_batch(&session_id, config, calls);
		let hard = preview.blocked;
		let generated = preview.generated;
		let plugin_verdict = plugin_tool_verdict(&session_id, config, calls, &hard, &generated, origin).await;
		let mut admissions = hard
			.into_iter()
			.map(|message| crate::session::guardrails::Admission {
				message,
				..Default::default()
			})
			.collect::<Vec<_>>();
		for (index, reason) in plugin_verdict.denials {
			if admissions[index].message.is_none() {
				admissions[index].message = Some(reason.clone());
				plugin_denials.insert(index, reason);
			}
		}
		for (index, rules) in plugin_verdict.lifts {
			admissions[index].overridden_guards.extend(rules);
		}
		crate::session::guardrails::check_batch_admitted(&session_id, config, calls, &admissions)
	} else {
		crate::session::guardrails::check_batch(&session_id, config, calls)
	};
	messages
		.into_iter()
		.enumerate()
		.map(|(index, message)| {
			message.map(|message| {
				// A plugin's text is used as it is: the plugin owns its prefix.
				if message.starts_with("[guardrail]") || plugin_denials.get(&index) == Some(&message) {
					message
				} else {
					format!("[guardrail] {message}")
				}
			})
		})
		.collect()
}

/// What plugins decided about the calls of one batch, by index into the batch.
#[derive(Default)]
struct PluginToolVerdict {
	denials: Vec<(usize, String)>,
	lifts: Vec<(usize, Vec<String>)>,
}

/// Who made the calls a batch admits.
pub(crate) enum CallOrigin<'a> {
	/// The model, in a round that has the chat session or (an in-process
	/// subagent, a layer) none.
	Model(Option<&'a mut ChatSession>),
	/// The script of the `code` call `code_call_id`. Its round holds the chat
	/// session; `snapshot` is the session as the round found it.
	Script {
		code_call_id: &'a str,
		snapshot: Option<std::sync::Arc<payload::RoundSnapshot>>,
	},
}

/// `tool.before`: ask the plugins about a batch the built-in guards have
/// already looked at. `blocked` holds each call's built-in denial and
/// `generated` the generated rules that matched it.
async fn plugin_tool_verdict(
	session_id: &String,
	config: &Config,
	calls: &[crate::mcp::McpToolCall],
	blocked: &[Option<String>],
	generated: &[Vec<(String, String)>],
	origin: CallOrigin<'_>,
) -> PluginToolVerdict {
	let script_call_id = match &origin {
		CallOrigin::Script { code_call_id, .. } => Some(code_call_id.to_string()),
		CallOrigin::Model(_) => None,
	};
	let tool_calls = calls
		.iter()
		.enumerate()
		.map(|(index, call)| {
			let server = crate::mcp::tool_map::get_server_for_tool(&call.tool_name).map(|s| s.name().to_string());
			let capability =
				crate::session::guardrails::resolve_capability(session_id, config, server.as_deref(), &call.tool_name);
			let rules = generated[index]
				.iter()
				.filter(|(id, _)| crate::session::guardrails::plugin_of_rule(session_id, id).is_some())
				.map(|(id, _)| id.clone())
				.collect();
			payload::tool_call(call, capability, blocked[index].clone(), rules, script_call_id.clone())
		})
		.collect();

	let no_messages = Vec::new();
	let outcome = match &origin {
		CallOrigin::Model(Some(chat_session)) => {
			let parts = ContextParts::of(chat_session);
			let context = parts.context(&chat_session.session.messages, None);
			crate::plugins::tool_before(&context, ToolBefore { calls: tool_calls }).await
		}
		CallOrigin::Script {
			snapshot: Some(snapshot),
			..
		} => {
			let context = snapshot.parts.context(&snapshot.messages, None);
			crate::plugins::tool_before(&context, ToolBefore { calls: tool_calls }).await
		}
		_ => {
			let parts = detached_context_parts(session_id);
			let context = parts.context(&no_messages, None);
			crate::plugins::tool_before(&context, ToolBefore { calls: tool_calls }).await
		}
	};

	match origin {
		CallOrigin::Model(Some(chat_session)) => {
			if let Err(error) = crate::plugins::apply::apply(chat_session, outcome.effects).await {
				crate::log_debug!("Applying the effects of tool.before failed: {:#}", error);
			}
		}
		// The round that holds the chat session applies them when it ends.
		CallOrigin::Script { snapshot: Some(_), .. } => {
			crate::plugins::defer_effects(session_id, outcome.effects);
			crate::plugins::apply::settle(session_id, None);
		}
		CallOrigin::Model(None) | CallOrigin::Script { snapshot: None, .. } => {
			crate::plugins::apply::settle(session_id, None)
		}
	}

	let position_of = |call_id: &str| calls.iter().position(|call| call.tool_id == call_id);
	PluginToolVerdict {
		denials: outcome
			.denies
			.into_iter()
			.filter_map(|deny| Some((position_of(&deny.call_id)?, deny.reason)))
			.collect(),
		lifts: outcome
			.lifts
			.into_iter()
			.filter_map(|lift| Some((position_of(&lift.call_id)?, lift.rules)))
			.collect(),
	}
}

/// The common fields of an event for a session that has no chat session at
/// hand: an in-process subagent, known only by its synthetic id.
fn detached_context_parts(session_id: &String) -> ContextParts {
	let workdir = crate::mcp::get_thread_working_directory();
	ContextParts {
		session_id: session_id.clone(),
		parent_session_id: None,
		role: crate::session::context::get_session_role(session_id).unwrap_or_default(),
		// The model of a session without a chat session is not readable here.
		model: None,
		project: crate::plugins::project::project_name(&workdir),
		workdir,
		status: None,
		plan: None,
		active_skills: crate::session::context::get_active_skills(session_id),
	}
}

async fn execute_tools_with_context(
	current_tool_calls: Vec<crate::mcp::McpToolCall>,
	context: &mut ToolExecutionContext<'_>,
	config: &Config,
	operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	mode: OutputMode,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	let mut tool_tasks = Vec::new();
	// A tool the `code` tool lists may be called as a tool of its own; such a
	// call becomes the `code` script it means before anything else looks at it.
	// Layers and scheduled runs reach this without the main loop's rewrite.
	let current_tool_calls = crate::mcp::code::script_direct_calls(config, current_tool_calls).await;

	// Guardrails run sequentially over the ordered batch before any tool is
	// spawned. Tools arrive in the order the model emitted them, so `+/-`
	// history conditions evaluate against the intent log — earlier allowed
	// calls in the batch are visible to later ones. Blocked tools are NEVER
	// spawned; they return an immediate error result, saving the execution
	// roundtrip entirely.
	let session_id_for_guardrails = context.session_name().to_string();
	// Messages are stored already prefixed with their source, so the spawn loop
	// below emits them verbatim.
	let tree_before = match context {
		ToolExecutionContext::MainSession { .. } => plugin_tree_fingerprint(config),
		_ => None,
	};
	let block_messages = admit_tool_batch(
		&session_id_for_guardrails,
		config,
		&current_tool_calls,
		CallOrigin::Model(match context {
			ToolExecutionContext::MainSession { chat_session, .. } => Some(&mut **chat_session),
			_ => None,
		}),
	)
	.await;
	if operation_cancelled.as_ref().is_some_and(|rx| *rx.borrow()) {
		return Ok((Vec::new(), 0));
	}
	// Plugins judge each call a script makes while this round holds the chat
	// session, so they are shown the session as the round found it.
	let round_snapshot = match &*context {
		ToolExecutionContext::MainSession { chat_session, .. }
			if crate::plugins::subscribed(&session_id_for_guardrails, event::TOOL_BEFORE)
				&& current_tool_calls
					.iter()
					.any(|call| call.tool_name == crate::mcp::code::TOOL_NAME) =>
		{
			Some(std::sync::Arc::new(payload::RoundSnapshot {
				parts: ContextParts::of(chat_session),
				messages: chat_session.session.messages.clone(),
			}))
		}
		_ => None,
	};

	// A write is a barrier on both sides: reads must not observe an unfinished
	// earlier write, and a later write must not overtake earlier reads. Closing
	// a watch channel releases every dependent, including when a task panics.
	let mut previous_write: Option<tokio::sync::watch::Receiver<()>> = None;
	let mut pending_reads = Vec::new();

	for (index, tool_call) in current_tool_calls.clone().iter().enumerate() {
		// Increment tool call counter
		context.increment_tool_calls(&tool_call.tool_name);

		// CRITICAL FIX: Use the EXACT tool_id from the original API response
		// Don't generate a new UUID - use the one from the original tool_calls
		let original_tool_id = tool_call.tool_id.clone();

		// Clone tool_name separately for tool task tracking
		let tool_name = tool_call.tool_name.clone();
		let tool_index = index + 1; // 1-based index for display

		// Execute in a tokio task
		let config_clone = config.clone();

		let tool_id_for_task = original_tool_id.clone();
		let tool_call_clone = tool_call.clone(); // Clone for async move
		let cancel_token_for_task = operation_cancelled.clone(); // Pass cancellation token

		// Get session ID for task-local propagation
		let session_id = context.session_name().to_string();

		// Guardrail decided this tool is denied — return the error result
		// directly without spawning the underlying executor.
		let block_msg = block_messages.get(index).cloned().unwrap_or(None);
		let read_only = crate::mcp::tool_shape::tool_read_only_hint(&tool_name) == Some(true);
		let mut predecessors = if read_only {
			Vec::new()
		} else {
			std::mem::take(&mut pending_reads)
		};
		if let Some(previous) = &previous_write {
			predecessors.push(previous.clone());
		}
		let (release, finished) = tokio::sync::watch::channel(());
		if read_only {
			pending_reads.push(finished);
		} else {
			previous_write = Some(finished);
		}
		let layer = !matches!(context, ToolExecutionContext::MainSession { .. });
		let workdir_override = crate::mcp::workdir::working_directory_override();
		let snapshot = round_snapshot.clone();
		let task = tokio::spawn(crate::session::accounting::inherit(async move {
			let _release = release;
			for mut predecessor in predecessors {
				let _ = predecessor.changed().await;
			}
			if let Some(msg) = block_msg {
				return Ok((
					crate::mcp::McpToolResult::error(tool_call_clone.tool_name, tool_id_for_task, msg),
					0u64,
				));
			}
			// Queueing precedes the executor's timer and cancellation check.
			let execution = crate::session::context::with_session_id(session_id, async move {
				let mut call_with_id = tool_call_clone;
				call_with_id.tool_id = tool_id_for_task;
				if layer {
					crate::mcp::execute_layer_tool_call(&call_with_id, &config_clone, cancel_token_for_task).await
				} else {
					let execution = crate::mcp::execute_tool_call(&call_with_id, &config_clone, cancel_token_for_task);
					payload::with_round_snapshot(snapshot, execution).await
				}
			});
			if let Some(workdir) = workdir_override {
				crate::mcp::workdir::with_working_directory(workdir, execution).await
			} else {
				execution.await
			}
		}));

		tool_tasks.push((tool_name, task, original_tool_id, tool_index));
	}

	// FIXED: Proper parallel awaiting with immediate cancellation support
	let mut tool_results = Vec::new();
	// The longest start of a tool's own text any plugin asked to see.
	let returned_chars = crate::plugins::options(&session_id_for_guardrails)
		.into_iter()
		.filter_map(|(_, options)| options.returned_chars)
		.max();
	let mut own_answers: HashMap<String, OwnAnswer> = HashMap::new();
	let mut _has_error = false;
	let mut total_tool_time_ms = 0; // Track cumulative tool execution time

	// Extract task info for later use
	let task_info: Vec<(String, String, usize)> = tool_tasks
		.iter()
		.map(|(tool_name, _, tool_id, tool_index)| (tool_name.clone(), tool_id.clone(), *tool_index))
		.collect();

	// Extract just the tasks for parallel execution
	let tasks: Vec<_> = tool_tasks.into_iter().map(|(_, task, _, _)| task).collect();

	// Animation lifecycle during tool execution:
	// - response.rs starts animation AFTER tool header is printed (so header is visible)
	// - This function stops animation BEFORE displaying results (no ghost spinners)
	// Do NOT start animation here — it's already running from response.rs.

	// Use tokio::select! to run tasks with cancellation support.
	let all_tasks = futures::future::join_all(tasks);
	tokio::pin!(all_tasks);
	let task_results: Vec<_> = tokio::select! {
		// Cancelled executors can finish in the same poll as the cancellation signal.
		biased;
		_ = async {
			if let Some(ref cancel_rx) = operation_cancelled {
				let mut cancel_rx_clone = cancel_rx.clone();
				while !*cancel_rx_clone.borrow() {
					if cancel_rx_clone.changed().await.is_err() {
						break;
					}
				}
			} else {
				std::future::pending::<()>().await;
			}
		} => {
			// Animation is stopped by main_loop.rs when Ctrl+C fires - no action needed here.

			// Cancellation occurred - provide immediate feedback
			use colored::*;
			if !mode.should_suppress_cli_output() {
				println!(
					"{}",
					"🛑 All tool execution cancelled - returning to input".bright_yellow()
				);
			}

			// Show cancellation message for each tool
			if !mode.should_suppress_cli_output() {
				for (tool_name, _, _) in task_info {
					println!(
						"{}",
						format!("🛑 Tool '{}' cancelled - server preserved", tool_name).bright_yellow()
					);
				}
			}

			// Tools that finished before the cancel are discarded undelivered.
			crate::session::context::bump_context_epoch();
			return Ok((Vec::new(), total_tool_time_ms));
		}
		results = &mut all_tasks => results,
	};

	// Note: No explicit stop needed before printing tool results.
	// The spinner-aware print macros use pb.suspend() to safely interleave
	// output with the live spinner (see src/lib.rs + animation_manager).

	// All tasks completed before cancellation
	for ((tool_name, tool_id, tool_index), task_result) in task_info.into_iter().zip(task_results) {
		// Store tool call info for consolidated display after execution
		let tool_call_info = current_tool_calls
			.iter()
			.find(|tc| tc.tool_id == tool_id)
			.or_else(|| current_tool_calls.iter().find(|tc| tc.tool_name == tool_name));

		// Store for display after execution
		let stored_tool_call = tool_call_info.cloned();

		match task_result {
			Ok(result) => {
				match result {
					Ok((res, tool_time_ms)) => {
						// Lifecycle identity belongs to the raw result, before deduplication or output caps.
						if let Some(call) = stored_tool_call.as_ref() {
							context.capture_tap_run(call, &res);
						}
						if matches!(context, ToolExecutionContext::MainSession { .. }) {
							own_answers.insert(tool_id.clone(), OwnAnswer::of(&res, returned_chars));
						}
						// Deduplication runs HERE, before display — so the terminal block,
						// the sink emission (response.rs), and the AI's session message all
						// show the SAME thing. Errors are never deduped. A successful
						// duplicate becomes an ERROR result carrying the placeholder: it
						// renders as a ✗ block, emits success=false to the UI, and reaches
						// the model as an error instead of a silent "success" only the model
						// could tell was elided. It does NOT count as a tool failure (the
						// tool ran fine — it's a repeat), so error_tracker is left untouched.
						// Text-only keys cannot distinguish changed image payloads or preserve
						// their association with each new tool call; rich results are not elided.
						let dedup_placeholder = if res.is_error() || !crate::mcp::tool_shape::is_plain_text_result(&res)
						{
							None
						} else {
							let content = res.extract_content();
							// Key dedup on (tool_name, args, content): identical args with
							// CHANGED output (re-reading a file after editing it) must reach
							// the model, and different args that return the same bytes (a
							// `view` that ignores its range) must not collide.
							let args = stored_tool_call
								.as_ref()
								.map(|tc| tc.parameters.to_string())
								.unwrap_or_default();
							if crate::session::dedup::is_duplicate(&tool_name, &args, &content) {
								// `was_truncated` selects the stronger stop+narrow message.
								// Truncation runs later (handle_large_tool_results), so detect
								// it directly here on the full content.
								let was_truncated = crate::utils::truncation::truncate_mcp_response_global(
									&content,
									config.mcp_response_tokens_threshold,
									&tool_name,
								)
								.1;
								Some(crate::session::dedup::placeholder(&tool_name, &content, was_truncated))
							} else {
								crate::session::dedup::record(&tool_name, &args, &content);
								None
							}
						};

						// CRITICAL MCP PROTOCOL FIX: Check if result is actually an error
						if let Some(placeholder) = dedup_placeholder {
							// Duplicate: shown + sent as an error, but not counted as a
							// tool failure — leave error_tracker untouched.
							let dup_err = anyhow::anyhow!("{}", placeholder);
							display_tool_error(
								&stored_tool_call,
								&tool_name,
								&dup_err,
								tool_index,
								config,
								mode,
								context.execution_context(),
							)
							.await;
							tool_results.push(crate::mcp::McpToolResult::error(
								tool_name.clone(),
								tool_id.clone(),
								placeholder,
							));
							total_tool_time_ms += tool_time_ms;
						} else if res.is_error() {
							// This is an MCP error result (isError: true) - treat as error
							_has_error = true;

							// Record error in error tracker
							if let Some(error_tracker) = context.error_tracker() {
								let has_hit_threshold = error_tracker.record_error(&tool_name);
								if has_hit_threshold {
									crate::log_debug!("Tool '{}' has hit error threshold", tool_name);
								}
							}

							// Extract error message from MCP result
							let error_content = res.extract_content();
							let error = anyhow::anyhow!("{}", error_content);

							// Display as error, not success
							display_tool_error(
								&stored_tool_call,
								&tool_name,
								&error,
								tool_index,
								config,
								mode,
								context.execution_context(),
							)
							.await;

							// Still push the result for conversation continuity (AI needs to see the error)
							tool_results.push(res.clone());

							// Accumulate tool execution time even for errors
							total_tool_time_ms += tool_time_ms;
						} else {
							// This is a genuine success (isError: false or missing)
							// Tool succeeded, reset the error counter (if available)
							if let Some(error_tracker) = context.error_tracker() {
								error_tracker.record_success(&tool_name);
							}

							// Display the complete tool execution with consolidated info
							let display_params = ToolDisplayParams {
								stored_tool_call: &stored_tool_call,
								tool_name: &tool_name,
								tool_index,
							};
							display_tool_success(
								display_params,
								&res,
								tool_time_ms,
								config,
								mode,
								context.execution_context(),
							)
							.await;

							tool_results.push(res.clone());

							// Accumulate tool execution time
							total_tool_time_ms += tool_time_ms;
						}
					}
					Err(e) => {
						_has_error = true;

						// Display error in consolidated format for other errors
						display_tool_error(
							&stored_tool_call,
							&tool_name,
							&e,
							tool_index,
							config,
							mode,
							context.execution_context(),
						)
						.await;

						// Track errors for this tool (if error tracking is available)
						let loop_detected = if let Some(error_tracker) = context.error_tracker() {
							error_tracker.record_error(&tool_name)
						} else {
							false
						};

						if loop_detected {
							// Always show loop detection warning since it's critical
							if let Some(error_tracker) = context.error_tracker() {
								if !mode.should_suppress_cli_output() {
									println!("{}", format!("⚠ Warning: {} failed {} times in a row - AI should try a different approach",
								tool_name, error_tracker.max_consecutive_errors()).bright_yellow());
								}

								// Add a detailed error result for loop detection
								let loop_error_result = crate::mcp::McpToolResult::error(
									tool_name.clone(),
									tool_id.clone(),
									loop_error_message(
										&tool_name,
										error_tracker.max_consecutive_errors(),
										&e.to_string(),
									),
								);
								tool_results.push(loop_error_result);
							}
						} else {
							// Regular error - add normal error result
							let error_result = if let Some(error_tracker) = context.error_tracker() {
								crate::mcp::McpToolResult::error(
									tool_name.clone(),
									tool_id.clone(),
									attempt_error_message(
										error_tracker.get_error_count(&tool_name),
										error_tracker.max_consecutive_errors(),
										&e.to_string(),
									),
								)
							} else {
								crate::mcp::McpToolResult::error(
									tool_name.clone(),
									tool_id.clone(),
									format!("Tool execution failed: {}", e),
								)
							};
							tool_results.push(error_result);

							if let Some(error_tracker) = context.error_tracker() {
								log_info!(
									"Tool '{}' failed {} of {} times. Adding error to context.",
									tool_name,
									error_tracker.get_error_count(&tool_name),
									error_tracker.max_consecutive_errors()
								);
							}
						}
					}
				}
			}
			Err(e) => {
				_has_error = true;

				// Display task error in consolidated format for other errors —
				// `display_tool_error` already prints a framed `╭ … ╰ ✗` block
				// with the error summary on the close line, so no extra raw
				// `println!` is needed below (that would just spill outside
				// the visual block as an unframed line).
				display_tool_error(
					&stored_tool_call,
					&tool_name,
					&anyhow::anyhow!("{}", e),
					tool_index,
					config,
					mode,
					context.execution_context(),
				)
				.await;

				// ALWAYS add error result for task failures too (unless it was a user decline)
				let error_result = crate::mcp::McpToolResult::error(
					tool_name.clone(),
					tool_id.clone(),
					format!("Internal task error: {}", e),
				);
				tool_results.push(error_result);
			}
		}
	}

	// Post-result hooks: run AFTER all tool results are collected but BEFORE
	// truncation, so hook scripts see full untruncated output. Hooks for
	// guardrail-blocked tools are skipped (synthetic results aren't real).
	let blocked_flags: Vec<bool> = block_messages.iter().map(|m| m.is_some()).collect();
	crate::session::hooks::run_hooks(
		&session_id_for_guardrails,
		config,
		&current_tool_calls,
		&tool_results,
		&blocked_flags,
	)
	.await;

	// Hard per-tool cap first: whatever the agent would actually receive is what
	// the condenser sees and selects line ranges over. Condensing the untruncated
	// body would let it keep lines the truncated result never contains.
	let mut tool_results = handle_large_tool_results(tool_results, config, mode).await?;

	// Plugins see the capped text and may replace it; this must come before
	// anything emits or stores the results.
	if let ToolExecutionContext::MainSession {
		chat_session,
		tool_round_intent,
		round_stop,
		..
	} = context
	{
		*round_stop = plugin_round(
			chat_session,
			config,
			&RoundCalls {
				calls: &current_tool_calls,
				denied: &blocked_flags,
				own_answers: &own_answers,
			},
			&mut tool_results,
			tool_round_intent,
			tree_before,
		)
		.await;
	}

	Ok((tool_results, total_tool_time_ms))
}

/// A background job's result, delivered when its MCP notification arrives, is
/// the deferred output of the tool call that launched it, so it gets what an
/// on-time result gets: the hard per-tool cap, then the plugin round.
/// Only the MCP output inside the delivery is shaped; status snapshots and
/// every other inbox message pass through untouched. Every inbox drain runs
/// this before its messages reach the transcript.
pub(crate) async fn shape_background_deliveries(
	batch: &mut [crate::session::inbox::InboxMessage],
	chat_session: &ChatSession,
	config: &Config,
) -> Result<()> {
	use crate::session::shell_jobs::{delivery_message, split_delivery, DELIVERY_ERROR_ATTR};

	let mut framings = Vec::new();
	let mut results = Vec::new();
	for (index, msg) in batch.iter().enumerate() {
		let crate::session::inbox::InboxSource::BackgroundJob { id } = &msg.source else {
			continue;
		};
		let Some((open_tag, output)) = split_delivery(&msg.content) else {
			continue;
		};
		let tool_name = msg.source.display_kind().to_string();
		results.push(if open_tag.contains(DELIVERY_ERROR_ATTR) {
			crate::mcp::McpToolResult::error(tool_name, id.clone(), output.to_string())
		} else {
			crate::mcp::McpToolResult::success(tool_name, id.clone(), output.to_string())
		});
		framings.push((index, open_tag.to_string()));
	}
	if results.is_empty() {
		return Ok(());
	}

	let mut results = handle_large_tool_results(results, config, config.output_mode()).await?;
	plugin_background_round(chat_session, &mut results).await;
	for ((index, open_tag), result) in framings.into_iter().zip(results) {
		batch[index].content = delivery_message(&open_tag, &result.extract_content());
	}
	Ok(())
}

/// The fingerprint a round's `tool.after` compares against, taken before the
/// round runs. Only measured when plugins are configured.
fn plugin_tree_fingerprint(config: &Config) -> Option<u64> {
	config
		.has_plugins()
		.then(crate::plugins::workdir::fingerprint)
		.flatten()
}

/// Whether a plugin may replace this result and keep the original readable:
/// plain text, a place to save it, and not itself the read of a saved original.
fn is_replaceable(call: &crate::mcp::McpToolCall, result: &crate::mcp::McpToolResult) -> bool {
	crate::mcp::tool_shape::is_plain_text_result(result)
		&& crate::session::context::current_session_id().is_some()
		&& !crate::utils::spill::references_spill(&call.parameters)
		&& !crate::utils::spill::spill_exists(&result.tool_name, &result.extract_content())
}

/// Put `replace.text` in the result, keeping its error flag. With
/// `keep_original` the full text is saved first and its path goes where the
/// plugin wrote `{original_path}`; when it cannot be saved the result stays.
fn apply_replace(result: &mut crate::mcp::McpToolResult, replace: crate::plugins::protocol::Replace) {
	if !crate::mcp::tool_shape::is_plain_text_result(result) {
		return;
	}
	let text = if replace.keep_original {
		let Some(path) = crate::utils::spill::write_spill(&result.tool_name, &result.extract_content()) else {
			return;
		};
		replace
			.text
			.replace(ORIGINAL_PATH_PLACEHOLDER, &path.display().to_string())
	} else {
		replace.text
	};
	let content = vec![rmcp::model::ContentBlock::text(text)];
	result.result = if result.is_error() {
		rmcp::model::CallToolResult::error(content)
	} else {
		rmcp::model::CallToolResult::success(content)
	};
}

/// The end of a finished tool round: runs after the hard size cap and before
/// the results are condensed, emitted or stored. Feeds octomind's own repeat
/// counter, which works with no plugin configured; with plugins it also
/// records what the round changed, sends `tool.after` and applies the
/// replacements. Returns why the turn ends after this round.
async fn plugin_round(
	chat_session: &mut ChatSession,
	config: &Config,
	round: &RoundCalls<'_>,
	results: &mut [McpToolResult],
	intent: &str,
	tree_before: Option<u64>,
) -> Option<RoundStop> {
	let RoundCalls {
		calls,
		denied,
		own_answers,
	} = *round;
	let session_id = chat_session.session.info.name.clone();
	let workdir = crate::mcp::get_thread_working_directory();
	for effects in crate::plugins::take_deferred_effects(&session_id) {
		if let Err(error) = crate::plugins::apply::apply(chat_session, effects).await {
			crate::log_debug!("Applying the effects of a script's tool.before failed: {:#}", error);
		}
	}

	let outcomes: Vec<ToolOutcome> = calls
		.iter()
		.enumerate()
		.filter_map(|(index, call)| {
			let result = results.iter().find(|result| result.tool_id == call.tool_id)?;
			let was_denied = denied.get(index).copied().unwrap_or(false);
			let replaceable = !was_denied && is_replaceable(call, result);
			let returned = own_answers.get(&call.tool_id).and_then(|own| own.returned.as_ref());
			Some(payload::tool_outcome(
				call,
				result,
				&workdir,
				was_denied,
				replaceable,
				returned,
			))
		})
		.collect();
	let replaceable_ids: std::collections::HashSet<String> = outcomes
		.iter()
		.filter(|outcome| outcome.replaceable)
		.map(|outcome| outcome.id.clone())
		.collect();

	// A script's own calls name the paths it changed; the `code` call names none.
	let made = outcomes
		.iter()
		.flat_map(|outcome| std::iter::once(outcome).chain(&outcome.script_calls));
	for outcome in made.filter(|o| o.mutation && !o.error && !o.denied) {
		chat_session.turn_mutated_paths.extend(outcome.paths.iter().cloned());
	}

	// Counted on the text as the cap left it, before any replacement, and on
	// what the tools themselves returned.
	let (shown_round, returned_round): (Vec<_>, Vec<_>) = calls
		.iter()
		.filter_map(|call| {
			let outcome = outcomes.iter().find(|outcome| outcome.id == call.tool_id)?;
			let shown = crate::session::repeat_guard::RoundCall {
				tool: &call.tool_name,
				parameters: &call.parameters,
				result: outcome.result.as_deref().unwrap_or_default(),
				is_error: outcome.error,
				is_mutation: outcome.mutation,
				free_form: outcome.free_form,
			};
			let returned = match own_answers.get(&call.tool_id) {
				Some(own) => crate::session::repeat_guard::RoundCall {
					result: &own.digest,
					is_error: own.error,
					..shown
				},
				None => shown,
			};
			Some((shown, returned))
		})
		.unzip();
	chat_session
		.repeat_guard
		.note_round_returned(&shown_round, &returned_round);
	let tree_after = plugin_tree_fingerprint(config);
	// A call that ran and could have moved the tree: a mutation, or a command,
	// which may write before it fails. A script could when one of its calls could.
	let write_capable = outcomes
		.iter()
		.flat_map(|outcome| std::iter::once(outcome).chain(&outcome.script_calls))
		.any(|outcome| !outcome.denied && (outcome.mutation || outcome.command.is_some()));
	if super::round_moved_tree(write_capable, tree_before, tree_after) {
		chat_session.repeat_guard.note_tree_moved();
	}
	let identical = chat_session.repeat_guard.identical_rounds();
	let max_identical = config.max_identical_rounds;
	let looping = max_identical > 0 && identical >= max_identical;

	let mut plugin_stop = None;
	if crate::plugins::subscribed(&session_id, event::TOOL_AFTER) {
		let (finished, verified) = crate::mcp::agent::handback::take_handback();
		let fingerprint_text =
			|fingerprint: Option<u64>| fingerprint.map(|value| value.to_string()).unwrap_or_default();
		let payload = ToolAfter {
			intent: (!intent.trim().is_empty()).then(|| intent.to_string()),
			calls: outcomes,
			tree: Some(TreeChange {
				before: fingerprint_text(tree_before),
				after: fingerprint_text(tree_after),
			}),
			subagents: (finished > 0).then_some(SubagentRuns {
				finished: finished as u32,
				verified: verified as u32,
			}),
			background: Vec::new(),
			stopping: looping.then(|| STOPPING_LOOP.to_string()),
			context_files: Vec::new(),
		};
		let parts = ContextParts::of(chat_session);
		let outcome = crate::plugins::tool_after(&parts.context(&chat_session.session.messages, None), payload).await;
		for replace in outcome.replaces {
			// Only a call the plugin was told it may replace: a denial reason or the
			// read of a saved original must stay as it is.
			if !replaceable_ids.contains(&replace.call_id) {
				crate::log_debug!(
					"Ignored the replacement for call '{}': it was not replaceable",
					replace.call_id
				);
				continue;
			}
			if let Some(result) = results.iter_mut().find(|result| result.tool_id == replace.call_id) {
				apply_replace(result, replace);
			}
		}
		plugin_stop = outcome.stop;
		if let Err(error) = crate::plugins::apply::apply(chat_session, outcome.effects).await {
			crate::log_debug!("Applying the effects of tool.after failed: {:#}", error);
		}
	}

	if looping {
		Some(RoundStop::RepeatedRounds(identical))
	} else {
		plugin_stop.map(RoundStop::Plugin)
	}
}

/// `tool.after` for background job output that arrived between rounds: no
/// calls, one output per delivery. Replacements go into the delivery text.
/// Only the part of the effects that needs no `&mut` session can be applied
/// here: the callers of this function hold a shared session.
async fn plugin_background_round(chat_session: &ChatSession, results: &mut [McpToolResult]) {
	let session_id = &chat_session.session.info.name;
	if !crate::plugins::subscribed(session_id, event::TOOL_AFTER) {
		return;
	}
	// No launching call is linked to a job yet, so each delivery goes under its own id.
	let background: Vec<BackgroundOutput> = results
		.iter()
		.map(|result| BackgroundOutput {
			call_id: result.tool_id.clone(),
			label: result.tool_name.clone(),
			output: result.extract_content(),
		})
		.collect();
	let payload = ToolAfter {
		intent: None,
		calls: Vec::new(),
		tree: None,
		subagents: None,
		background: background.clone(),
		stopping: None,
		context_files: Vec::new(),
	};
	let parts = ContextParts::of(chat_session);
	let outcome = crate::plugins::tool_after(&parts.context(&chat_session.session.messages, None), payload).await;
	for replace in outcome.replaces {
		if let Some(index) = background.iter().position(|output| output.call_id == replace.call_id) {
			apply_replace(&mut results[index], replace);
		}
	}
	if !outcome.effects.notes.is_empty() || outcome.effects.plan.is_some() {
		crate::log_debug!("tool.after for background output answered with notes or a plan change; not applied");
	}
	crate::plugins::apply::settle(session_id, None);
}

/// Error body for a tool that hit the consecutive-failure threshold. The
/// wording steers the model away from retry loops — keep it stable.
fn loop_error_message(tool_name: &str, max_errors: usize, last_error: &str) -> String {
	format!("LOOP DETECTED: Tool '{tool_name}' failed {max_errors} consecutive times. Last error: {last_error}. Please try a completely different approach or ask the user for guidance.")
}

/// Error body for a regular failure, carrying the attempt counter so the
/// model can see how close it is to the loop threshold.
fn attempt_error_message(attempt: usize, max_errors: usize, error: &str) -> String {
	format!("Tool execution failed (attempt {attempt}/{max_errors}): {error}")
}

// Handle large tool results: apply token-cap truncation (warnings removed).
async fn handle_large_tool_results(
	results: Vec<crate::mcp::McpToolResult>,
	config: &Config,
	_mode: OutputMode,
) -> Result<Vec<crate::mcp::McpToolResult>> {
	// Apply token truncation — hard cap on per-tool output size.
	let results: Vec<crate::mcp::McpToolResult> = results
		.into_iter()
		.map(|mut result| {
			// Flattening and rebuilding a rich MCP result would discard resource,
			// image, audio, or structured-content semantics. Until the hard cap
			// has a protocol-aware representation, correctness wins: leave these
			// payloads intact rather than silently converting them to plain text.
			if !crate::mcp::tool_shape::is_plain_text_result(&result) {
				return result;
			}
			let content_str = result.extract_content();
			let (truncated, was_truncated) = crate::utils::truncation::truncate_mcp_response_global(
				&content_str,
				config.mcp_response_tokens_threshold,
				&result.tool_name,
			);
			if was_truncated {
				// Preserve the error flag through truncation. Rebuilding every
				// result as success() flipped failing tools to is_error()==false,
				// which let truncated errors (a failing build/test easily exceeds
				// the token cap) enter the dedup cache and get elided on the next
				// identical failure — violating the "errors are never deduped"
				// invariant exactly when the model needs the error text most.
				let was_error = result.is_error();
				let content = vec![rmcp::model::ContentBlock::text(truncated)];
				result.result = if was_error {
					rmcp::model::CallToolResult::error(content)
				} else {
					rmcp::model::CallToolResult::success(content)
				};
			}
			result
		})
		.collect();

	Ok(results)
}

// Parameters for tool display functions
struct ToolDisplayParams<'a> {
	stored_tool_call: &'a Option<crate::mcp::McpToolCall>,
	tool_name: &'a str,
	tool_index: usize,
}

// Display successful tool execution. Each tool gets a self-contained block:
// `╭ tool · server` + railed params + railed output + `╰ ✓ tool Xms · N tokens`.
// The header is re-rendered here regardless of upfront preview — the upfront
// preview is only a compact `▸ tool · server` queue listing for parallel
// tools, not a substitute for the full result block.
async fn display_tool_success(
	params: ToolDisplayParams<'_>,
	res: &crate::mcp::McpToolResult,
	tool_time_ms: u64,
	config: &Config,
	mode: OutputMode,
	execution_context: Option<String>,
) {
	if !mode.should_suppress_cli_output() {
		crate::session::chat::tool_display::display_individual_tool_header_with_context(
			params.tool_name,
			params.stored_tool_call,
			config,
			params.tool_index,
			execution_context.as_deref(),
		)
		.await;
		crate::session::chat::tool_display::display_script_calls(&res.script_calls);
	}

	// Output content (info/debug levels only; respect JSONL suppression).
	// `display_tool_output_smart` adds the `│ ` rail per line.
	if !mode.should_suppress_cli_output()
		&& (config.get_log_level().is_info_enabled() || config.get_log_level().is_debug_enabled())
	{
		let content = res.extract_content();
		if !content.trim().is_empty() {
			if config.get_log_level().is_debug_enabled() {
				use colored::Colorize;
				let rail = "│".bright_black();
				for line in content.lines() {
					println!("{} {}", rail, line);
				}
			} else {
				crate::session::chat::tool_display::display_tool_output_smart(&content);
			}
		}
	}

	// Close line: `╰ ✓ tool Xms · N tokens [· truncated to MK]` — tool name
	// identifies which tool finished when results arrive out of order. When
	// the response will be truncated by `mcp_response_tokens_threshold`, the
	// indicator is appended inline (yellow) instead of dumping a separate
	// `⚠️ … response truncated …` line below the block.
	if !mode.should_suppress_cli_output() {
		let content = res.extract_content();
		let token_count = crate::session::token_counter::estimate_tokens(&content);
		let formatted_tokens = crate::session::chat::format_number(token_count as u64);

		use colored::Colorize;
		let threshold = config.mcp_response_tokens_threshold;
		let truncated_suffix = if threshold > 0 && token_count > threshold {
			let formatted_threshold = crate::session::chat::format_number(threshold as u64);
			format!(
				" {} {}",
				"·".bright_black(),
				format!("truncated to {} tokens", formatted_threshold).bright_yellow()
			)
		} else {
			String::new()
		};
		println!(
			"{} {} {} {}ms {} {} tokens{}",
			"╰".bright_cyan(),
			"✓".bright_green(),
			params.tool_name.bright_cyan(),
			tool_time_ms,
			"·".bright_black(),
			formatted_tokens,
			truncated_suffix,
		);
	}
}

// Display tool error in consolidated format. Matches the success layout:
// re-rendered header + optional body lines under a `│ ` rail + `╰ ✗ tool:
// summary` close. The body rail is essential when the error message spans
// multiple lines (e.g. a `view` on a directory returns a one-line summary
// plus the directory listing as a hint) — without it, every line after the
// first prints raw and breaks the visual block.
async fn display_tool_error(
	stored_tool_call: &Option<crate::mcp::McpToolCall>,
	tool_name: &str,
	error: &anyhow::Error,
	tool_index: usize,
	config: &Config,
	mode: OutputMode,
	execution_context: Option<String>,
) {
	if mode.should_suppress_cli_output() {
		return;
	}

	crate::session::chat::tool_display::display_individual_tool_header_with_context(
		tool_name,
		stored_tool_call,
		config,
		tool_index,
		execution_context.as_deref(),
	)
	.await;

	use colored::Colorize;
	let error_text = error.to_string();
	let mut lines = error_text.lines();
	let summary = lines.next().unwrap_or("").to_string();
	let body: String = lines.collect::<Vec<&str>>().join("\n");
	if !body.is_empty() {
		crate::session::chat::tool_display::display_tool_output_smart(&body);
	}
	println!(
		"{} {} {}: {}",
		"╰".bright_cyan(),
		"✗".bright_red(),
		tool_name.bright_red(),
		summary,
	);
}

/// Parameters for layer tool execution using the unified parallel logic.
pub struct LayerToolExecutionParams {
	pub tool_calls: Vec<crate::mcp::McpToolCall>,
	pub session_name: String,
	pub layer_name: String,
	pub operation_cancelled: Option<tokio::sync::watch::Receiver<bool>>,
	pub mode: OutputMode,
}

/// Execute tool calls for layers using the unified parallel execution logic
pub async fn execute_layer_tool_calls_parallel(
	config: &Config,
	params: LayerToolExecutionParams,
) -> Result<(Vec<crate::mcp::McpToolResult>, u64)> {
	let mut context = ToolExecutionContext::Layer {
		session_name: params.session_name,
		layer_name: params.layer_name,
	};

	execute_tools_in_context(
		params.tool_calls,
		&mut context,
		config,
		params.operation_cancelled,
		params.mode,
	)
	.await
}

#[cfg(test)]
#[path = "tool_execution_tests.rs"]
mod tests;
