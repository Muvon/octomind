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

//! Verify-gate — when the agent self-reports `done`, an independent pass checks
//! the result against the request before completion is accepted. On gaps the
//! caller injects an advisory and re-runs the turn (bounded). A PASS labels the
//! trajectory so only verified work is learned.

use crate::config::Config;
use tokio::sync::watch;

const GATE_PROMPT: &str = r#"You are a strict completion verifier. The agent claims a task is COMPLETE.
Check the agent's final result against the user's request. Identify concrete, checkable gaps
that mean the task is NOT actually done: missing steps, unmet requirements, or claims stated
without evidence.

If the task is genuinely complete, output exactly:
<verdict>PASS</verdict>

Otherwise output one line per gap (and nothing else):
<gap>specific missing or unverified item</gap>

Be conservative — only flag real, actionable gaps. If unsure, PASS."#;

/// Outcome of a verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
	Pass,
	Gaps(Vec<String>),
}

/// Verify a self-reported completion. `task` is the user's request, `result` is
/// the agent's final answer. Fails open (PASS) on empty input or LLM error — a
/// verifier outage must never block the agent.
pub async fn verify(
	config: &Config,
	task: &str,
	result: &str,
	operation_rx: watch::Receiver<bool>,
) -> GateVerdict {
	if task.trim().is_empty() || result.trim().is_empty() {
		return GateVerdict::Pass;
	}
	let user = format!("USER REQUEST:\n{task}\n\nAGENT FINAL RESULT:\n{result}");
	let model = config.supervisor.model.clone();
	match crate::supervisor::learning::extract::call_learning_llm(
		config,
		&model,
		GATE_PROMPT.to_string(),
		user,
		operation_rx,
	)
	.await
	{
		Ok(resp) => parse_verdict(&resp),
		Err(e) => {
			crate::log_debug!("Verify-gate call failed, accepting: {}", e);
			GateVerdict::Pass
		}
	}
}

fn parse_verdict(resp: &str) -> GateVerdict {
	if resp.contains("<verdict>PASS</verdict>") {
		return GateVerdict::Pass;
	}
	let mut gaps = Vec::new();
	let mut rest = resp;
	while let Some(s) = rest.find("<gap>") {
		let after = &rest[s + 5..];
		let Some(e) = after.find("</gap>") else {
			break;
		};
		let g = after[..e].trim();
		if !g.is_empty() {
			gaps.push(g.to_string());
		}
		rest = &after[e + 6..];
	}
	if gaps.is_empty() {
		GateVerdict::Pass
	} else {
		GateVerdict::Gaps(gaps)
	}
}

/// Build the out-of-band advisory injected back into the loop on gaps.
pub fn format_advisory(gaps: &[String]) -> String {
	let mut s = String::from(
		"<supervisor>\nBefore accepting completion, a verification pass found gaps:\n",
	);
	for g in gaps {
		s.push_str("- ");
		s.push_str(g);
		s.push('\n');
	}
	s.push_str(
		"Address them, then re-report your status. If they are already handled, explain why.\n</supervisor>",
	);
	s
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pass_parsed() {
		assert_eq!(parse_verdict("<verdict>PASS</verdict>"), GateVerdict::Pass);
	}

	#[test]
	fn gaps_parsed() {
		let v = parse_verdict("<gap>no tests</gap>\n<gap>missing docs</gap>");
		assert_eq!(
			v,
			GateVerdict::Gaps(vec!["no tests".into(), "missing docs".into()])
		);
	}

	#[test]
	fn no_markers_is_pass() {
		assert_eq!(parse_verdict("looks good to me"), GateVerdict::Pass);
	}
}
