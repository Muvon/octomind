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

//! External plan manager for specialist sessions.
//!
//! The specialist sees plan state but has no plan mutation tool. It emits only
//! a compact execution signal in its hidden status report. On those sparse
//! signals this module gives a separate planner the specialist's exact standing
//! instructions, available capabilities, current request, runtime evidence, and
//! active plan. The runtime applies the planner's structured decision.

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

const PLANNER_PROMPT: &str = r#"You are the external plan manager for a specialist agent.

You own high-level execution state. The specialist can execute domain actions and see the plan, but cannot create, advance, or revise it. Decide only from the supplied specialist instructions, capabilities, current request, runtime-recorded actions, and active plan.

Planning is exceptional, not ceremonial. Create a plan only when the remaining work has at least three meaningful dependent phases, material context-loss risk, or a real branch that must be tracked. Do not create a plan for an answer, review with one deliverable, focused fix, or a routine read/change/check sequence that the specialist can hold locally.

A plan contains 2-6 outcome-oriented phases. Each `done_when` is an observable state or delivered artifact, not a list of tool calls and not implementation narration. Preserve user prohibitions. Do not specialize the framework to software development.

For signal `request`, return either:
{"decision":"create","title":"short goal","tasks":[{"title":"phase","done_when":"observable condition"}]}
{"decision":"no_plan","reason":"why external tracking is unnecessary"}

For signal `phase_complete`, compare the current phase's `done_when` with runtime evidence. Return one of:
{"decision":"advance","summary":"specific observed outcome"}
{"decision":"hold","reason":"specific missing evidence"}
{"decision":"revise","reason":"what changed","tasks":[{"title":"remaining phase","done_when":"observable condition"}]}

For signal `reassess`, a runtime-checked plan assumption has broken. Return `revise` with a valid remaining route, or `hold` when no safe route is evidenced.

Revision replaces only the unfinished tail; completed history is preserved. Never advance merely because the specialist says it is complete. Output exactly one compact JSON object and nothing else."#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanSignal {
	Request,
	PhaseComplete,
	Reassess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTaskDirective {
	pub title: String,
	pub done_when: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
enum PlanDecision {
	Create {
		title: String,
		tasks: Vec<PlanTaskDirective>,
	},
	NoPlan {
		reason: String,
	},
	Advance {
		summary: String,
	},
	Hold {
		reason: String,
	},
	Revise {
		reason: String,
		tasks: Vec<PlanTaskDirective>,
	},
}

fn render_specialist_context(chat_session: &ChatSession) -> String {
	let instructions = chat_session
		.session
		.messages
		.iter()
		.find(|message| message.role == "system")
		.map(|message| message.content.as_str())
		.unwrap_or_default();
	let request = crate::session::latest_real_user_task_content(&chat_session.session.messages)
		.unwrap_or_default();
	let capabilities = chat_session
		.cached_tools
		.as_deref()
		.unwrap_or_default()
		.iter()
		.map(|tool| format!("- {}: {}", tool.name, tool.description))
		.collect::<Vec<_>>()
		.join("\n");
	let plan = crate::mcp::core::plan::render_plan_details().unwrap_or_default();
	let evidence = chat_session.evidence.render();
	format!(
		"<specialist_instructions authority=\"standing\">\n{instructions}\n</specialist_instructions>\n\n<specialist_capabilities>\n{capabilities}\n</specialist_capabilities>\n\n<current_request authority=\"user\">\n{request}\n</current_request>\n\n<active_plan>\n{plan}\n</active_plan>\n\n<runtime_evidence trust=\"recorded\">\n{evidence}\n</runtime_evidence>"
	)
}

fn plan_state_note() -> Option<String> {
	let plan = crate::mcp::core::plan::render_plan_checklist()?;
	Some(format!(
		"<runtime-plan authority=\"execution-state\">\n{plan}Complete the current phase against its stated outcome. Report `plan=\"phase_complete\"` only when that outcome is evidenced; the external manager owns all transitions.\n</runtime-plan>"
	))
}

/// Reconcile one sparse specialist signal after its action batch has produced
/// runtime evidence. Returns without a model call when there is no applicable
/// signal or the requested plan state already exists.
pub async fn reconcile_after_actions(
	chat_session: &mut ChatSession,
	config: &Config,
	operation_rx: watch::Receiver<bool>,
) -> Result<()> {
	let Some(signal) = chat_session.pending_plan_signal.take() else {
		return Ok(());
	};
	if !config.supervisor.enabled || !config.supervisor.plan.enabled {
		return Ok(());
	}
	let active = crate::mcp::core::plan::has_active_plan();
	if matches!(signal, PlanSignal::Request) && active {
		return Ok(());
	}
	if matches!(signal, PlanSignal::Request) {
		if chat_session.plan_evaluated {
			return Ok(());
		}
		chat_session.plan_evaluated = true;
	}
	if matches!(signal, PlanSignal::PhaseComplete | PlanSignal::Reassess) && !active {
		return Ok(());
	}

	if chat_session.cached_tools.is_none() {
		chat_session.cached_tools = Some(crate::mcp::get_available_functions(config).await);
	}
	let payload = format!(
		"<signal>{}</signal>\n\n{}",
		match signal {
			PlanSignal::Request => "request",
			PlanSignal::PhaseComplete => "phase_complete",
			PlanSignal::Reassess => "reassess",
		},
		render_specialist_context(chat_session)
	);
	let response = crate::supervisor::learning::extract::call_supervisor_llm(
		config,
		&config.supervisor.plan.model,
		PLANNER_PROMPT.to_string(),
		payload,
		crate::supervisor::stats::CallKind::Plan,
		crate::supervisor::learning::extract::SupervisorSampling {
			temperature: 0.0,
			max_tokens: config.supervisor.plan.max_tokens,
		},
		operation_rx,
	)
	.await;
	let decision = match response {
		Ok(response) => match serde_json::from_str::<PlanDecision>(response.trim()) {
			Ok(decision) => decision,
			Err(error) => {
				crate::log_debug!("External planner returned invalid JSON: {}", error);
				return Ok(());
			}
		},
		Err(error) => {
			crate::log_debug!("External planner unavailable: {}", error);
			return Ok(());
		}
	};

	let changed = match (signal, decision) {
		(PlanSignal::Request, PlanDecision::Create { title, tasks }) => {
			crate::mcp::core::plan::sidecar_start(&title, &tasks)?;
			crate::supervisor::notify(&format!(
				"external plan created with {} phase(s)",
				tasks.len()
			));
			true
		}
		(PlanSignal::Request, PlanDecision::NoPlan { reason }) => {
			crate::log_debug!("External planner declined plan: {}", reason);
			false
		}
		(PlanSignal::PhaseComplete, PlanDecision::Advance { summary }) => {
			crate::mcp::core::plan::sidecar_advance(&summary)?;
			crate::supervisor::notify("external plan advanced");
			true
		}
		(PlanSignal::PhaseComplete, PlanDecision::Revise { reason, tasks }) => {
			crate::mcp::core::plan::sidecar_revise(&reason, &tasks)?;
			crate::supervisor::notify(&format!("external plan revised: {}", reason.trim()));
			true
		}
		(PlanSignal::PhaseComplete, PlanDecision::Hold { reason }) => {
			chat_session.add_system_managed_user_message(&format!(
				"<runtime-plan-feedback>Current phase remains open: {}</runtime-plan-feedback>",
				reason.trim()
			))?;
			false
		}
		(PlanSignal::Reassess, PlanDecision::Revise { reason, tasks }) => {
			crate::mcp::core::plan::sidecar_revise(&reason, &tasks)?;
			crate::supervisor::notify(&format!("external plan revised: {}", reason.trim()));
			true
		}
		(PlanSignal::Reassess, PlanDecision::Hold { reason }) => {
			chat_session.add_system_managed_user_message(&format!(
				"<runtime-plan-feedback>Plan assumption failed and no safe revision was established: {}</runtime-plan-feedback>",
				reason.trim()
			))?;
			false
		}
		(_, decision) => {
			crate::log_debug!(
				"External planner returned decision incompatible with signal: {:?}",
				decision
			);
			false
		}
	};
	if changed {
		if let Some(note) = plan_state_note() {
			chat_session.add_system_managed_user_message(&note)?;
		}
	}
	Ok(())
}

/// Final completion owns the last transition. The completion gate has already
/// judged the full request, plan, actions, and result before this is called.
pub fn finalize_after_verification(summary: &str) -> Result<()> {
	crate::mcp::core::plan::sidecar_finish(summary)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_domain_neutral_plan() {
		let decision: PlanDecision = serde_json::from_str(
			r#"{"decision":"create","title":"Publish report","tasks":[{"title":"Gather sources","done_when":"source set is recorded"},{"title":"Synthesize","done_when":"claims map to sources"},{"title":"Deliver","done_when":"report is returned"}]}"#,
		)
		.unwrap();
		assert!(matches!(decision, PlanDecision::Create { tasks, .. } if tasks.len() == 3));
	}
}
