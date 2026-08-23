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

//! Sparse pre-mutation readiness check.
//!
//! The admission resolver may identify a task-anchoring observation whose
//! value can change which mutation is correct. Before the first direct
//! state-changing call, one cheap verifier checks whether the trajectory has
//! established that observation. It judges evidence state, never a preferred
//! route. Reads are never blocked, the check is one-shot, and every failure of
//! the supervisor itself passes through to preserve agent capability.

use crate::config::Config;
use crate::mcp::McpToolCall;
use crate::supervisor::learning::extract::{SupervisorPrompt, SupervisorSampling};
use serde::Deserialize;

const MAX_GROUNDS: usize = 4;
const MAX_GROUND_CHARS: usize = 2_000;
const MAX_GROUNDS_TOTAL_CHARS: usize = 7_000;

const SYSTEM_PROMPT: &str = r#"You are a sparse mutation-readiness verifier. An autonomous
specialist is about to change external state. Decide only whether a genuinely load-bearing
observation identified at task admission is still missing.

The specialist may use ANY valid approach. Never require a particular tool, command, sequence,
implementation, or style. Different routes reaching the same valid outcome are equivalent.

<input_format>
- <current_request authority="true"> is the user's request and the only source of scope.
- <state_dependencies> contains sequencing safeguards inferred from that request. They are not
  extra requirements and may be mistaken.
- <recorded_actions> and <recent_observations> are runtime-recorded evidence from completed calls.
- <proposed_mutations> are calls that have NOT executed yet.
</input_format>

Return ready=true when every applicable dependency is plausibly established by the recorded
evidence, when authoritative unavailability was established and the proposed action preserves
that limit, or when a dependency is irrelevant, contradicted by the request, or would require a
forbidden action. Ordinary local exploration, implementation details, and post-change validation
are never readiness dependencies.

Return ready=false only with HIGH confidence that an unresolved observation can materially change
which proposed mutation is correct. Name the missing observation, not a method for obtaining it.
If evidence is ambiguous or the specialist may have established the state through another valid
route, return ready=true. This is one sparse sequencing check, not process micromanagement.

Output exactly one JSON object and no prose:
{"ready":true,"gaps":[]}
or
{"ready":false,"gaps":["missing observation and why it matters before mutation"]}"#;

#[derive(Debug, Deserialize)]
struct ReadinessResponse {
	ready: bool,
	#[serde(default)]
	gaps: Vec<String>,
}

/// Whether this round contains a high-confidence direct state-changing call.
/// Concrete intent avoids treating every call through a write-capable generic
/// tool as a mutation.
pub fn has_mutations(calls: &[McpToolCall]) -> bool {
	calls.iter().any(|call| direct_mutation(call))
}

/// Conservative boundary signal for this early gate. Tool names and explicit
/// action/operation selectors are strong intent; a one-word command covers
/// editor-style multiplexed tools. Free-form command strings are deliberately
/// excluded because a write-capable shell often runs read-only research (and
/// tokens such as `set -e` are not state changes in the task environment).
fn direct_mutation(call: &McpToolCall) -> bool {
	let mut selectors = serde_json::Map::new();
	for key in ["action", "operation"] {
		if let Some(value) = call.parameters.get(key) {
			selectors.insert(key.to_string(), value.clone());
		}
	}
	if let Some(command) = call
		.parameters
		.get("command")
		.and_then(|value| value.as_str())
	{
		if !command.trim().is_empty() && !command.chars().any(char::is_whitespace) {
			selectors.insert("command".to_string(), serde_json::json!(command));
		}
	}
	crate::supervisor::detect::has_explicit_mutation_intent(
		&call.tool_name,
		&serde_json::Value::Object(selectors),
	)
}

/// Judge the proposed mutations against admission-time state dependencies.
/// Returns `(tool_id, error)` for each mutation to pause. Empty means pass.
pub async fn gate_round(
	calls: &[McpToolCall],
	config: &Config,
	task: &crate::supervisor::resolve::ResolvedTask,
	actions: &str,
	grounds: &[(u64, String)],
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Vec<(String, String)> {
	let mutations: Vec<&McpToolCall> = calls.iter().filter(|call| direct_mutation(call)).collect();
	if mutations.is_empty() || task.state_dependencies.is_empty() {
		return Vec::new();
	}

	let user = build_prompt(task, actions, grounds, &mutations);
	let schema = serde_json::json!({
		"type": "object",
		"additionalProperties": false,
		"properties": {
			"ready": { "type": "boolean" },
			"gaps": {
				"type": "array",
				"maxItems": 3,
				"items": { "type": "string" }
			}
		},
		"required": ["ready", "gaps"]
	});
	let response = crate::supervisor::learning::extract::call_supervisor_json(
		config,
		&config.supervisor.gate.verifier_model,
		SupervisorPrompt::new(SYSTEM_PROMPT.to_string(), user),
		crate::supervisor::stats::CallKind::Readiness,
		SupervisorSampling {
			temperature: 0.0,
			max_tokens: 512,
		},
		schema,
		operation_rx,
	)
	.await;
	let value = match response {
		Ok(value) => value,
		Err(error) => {
			crate::log_debug!(
				"Readiness check unavailable; mutations pass through: {}",
				error
			);
			return Vec::new();
		}
	};
	let parsed = match serde_json::from_value::<ReadinessResponse>(value) {
		Ok(parsed) => parsed,
		Err(error) => {
			crate::log_debug!(
				"Readiness response unusable; mutations pass through: {}",
				error
			);
			return Vec::new();
		}
	};
	decide(&mutations, parsed)
}

fn decide(mutations: &[&McpToolCall], response: ReadinessResponse) -> Vec<(String, String)> {
	if response.ready {
		return Vec::new();
	}
	let gaps: Vec<String> = response
		.gaps
		.into_iter()
		.map(|gap| gap.split_whitespace().collect::<Vec<_>>().join(" "))
		.filter(|gap| !gap.is_empty())
		.take(3)
		.collect();
	if gaps.is_empty() {
		// An unexplained block cannot guide a recovery, so it is not allowed to
		// constrain the specialist.
		return Vec::new();
	}

	let mut message = String::from(
		"[supervisor/readiness] State-changing action paused: a load-bearing observation is unresolved. Any authoritative approach is acceptable; no specific tool or route is required.",
	);
	for gap in &gaps {
		message.push_str("\n- ");
		message.push_str(gap);
	}
	message.push_str(
		"\nEstablish the observation or its authoritative unavailability, preserve any resulting limitation, then choose the appropriate action.",
	);
	crate::supervisor::stats::readiness_block(mutations.len() as u64);
	crate::supervisor::notify(&format!(
		"mutation readiness found {} unresolved observation(s) — state change paused",
		gaps.len()
	));
	mutations
		.iter()
		.map(|call| (call.tool_id.clone(), message.clone()))
		.collect()
}

fn build_prompt(
	task: &crate::supervisor::resolve::ResolvedTask,
	actions: &str,
	grounds: &[(u64, String)],
	mutations: &[&McpToolCall],
) -> String {
	let dependencies = task
		.state_dependencies
		.iter()
		.map(|dependency| {
			format!(
				"- observation: {}\n  grounded_in_user_excerpt: {}",
				crate::supervisor::escape_xml_text(&dependency.observation),
				crate::supervisor::escape_xml_text(&dependency.evidence),
			)
		})
		.collect::<Vec<_>>()
		.join("\n");
	let mutations = mutations
		.iter()
		.map(|call| {
			serde_json::json!({
				"tool": call.tool_name.as_str(),
				"parameters": &call.parameters,
			})
			.to_string()
		})
		.collect::<Vec<_>>()
		.join("\n");
	format!(
		"<current_request authority=\"true\">\n{}\n</current_request>\n\n<state_dependencies>\n{}\n</state_dependencies>\n\n<recorded_actions>\n{}\n</recorded_actions>\n\n<recent_observations>\n{}\n</recent_observations>\n\n<proposed_mutations>\n{}\n</proposed_mutations>",
		crate::supervisor::escape_xml_text(&task.resolved_request),
		dependencies,
		crate::supervisor::escape_xml_text(if actions.trim().is_empty() {
			"(none)"
		} else {
			actions
		}),
		render_recent_grounds(grounds),
		crate::supervisor::escape_xml_text(&mutations),
	)
}

fn render_recent_grounds(grounds: &[(u64, String)]) -> String {
	if grounds.is_empty() {
		return "(none)".to_string();
	}
	let mut selected = Vec::new();
	let mut total = 0usize;
	for (sequence, content) in grounds.iter().rev().take(MAX_GROUNDS) {
		let bounded: String = content.chars().take(MAX_GROUND_CHARS).collect();
		let remaining = MAX_GROUNDS_TOTAL_CHARS.saturating_sub(total);
		if remaining == 0 {
			break;
		}
		let bounded: String = bounded.chars().take(remaining).collect();
		total = total.saturating_add(bounded.chars().count());
		selected.push(format!(
			"#{}\n{}",
			sequence,
			crate::supervisor::escape_xml_text(&bounded)
		));
	}
	selected.reverse();
	selected.join("\n\n")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn call(id: &str, name: &str, parameters: serde_json::Value) -> McpToolCall {
		McpToolCall {
			tool_id: id.to_string(),
			tool_name: name.to_string(),
			parameters,
		}
	}

	#[test]
	fn only_state_changing_calls_are_candidates() {
		let calls = vec![
			call("read", "view", serde_json::json!({"path": "record"})),
			call(
				"write",
				"text_editor",
				serde_json::json!({"command": "replace", "path": "record"}),
			),
		];
		assert!(has_mutations(&calls));
		let mutations: Vec<&McpToolCall> =
			calls.iter().filter(|call| direct_mutation(call)).collect();
		let blocked = decide(
			&mutations,
			ReadinessResponse {
				ready: false,
				gaps: vec!["current external state is unknown".to_string()],
			},
		);
		assert_eq!(blocked.len(), 1);
		assert_eq!(blocked[0].0, "write");
		assert!(blocked[0].1.contains("Any authoritative approach"));

		let evidence_fetch = call(
			"fetch",
			"shell",
			serde_json::json!({"command": "set -e; gh run view 123 --log-failed"}),
		);
		assert!(!has_mutations(&[evidence_fetch]));
	}

	#[test]
	fn uncertainty_and_unactionable_rejections_pass_through() {
		let mutation = call(
			"write",
			"update_record",
			serde_json::json!({"value": "new"}),
		);
		assert!(decide(
			&[&mutation],
			ReadinessResponse {
				ready: true,
				gaps: vec!["ignored".to_string()],
			}
		)
		.is_empty());
		assert!(decide(
			&[&mutation],
			ReadinessResponse {
				ready: false,
				gaps: Vec::new(),
			}
		)
		.is_empty());
	}

	#[test]
	fn recent_observations_are_bounded_and_keep_latest() {
		let grounds = (0..10)
			.map(|index| (index, format!("observation-{index} {}", "x".repeat(5_000))))
			.collect::<Vec<_>>();
		let rendered = render_recent_grounds(&grounds);
		assert!(!rendered.contains("observation-0"));
		assert!(rendered.contains("observation-9"));
		assert!(rendered.chars().count() <= MAX_GROUNDS_TOTAL_CHARS + 100);
	}
}
