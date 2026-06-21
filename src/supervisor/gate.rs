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

const GATE_PROMPT: &str = r#"You are a strict completion verifier. A different agent claims its task is COMPLETE.
Judge the END STATE, not the agent's story: ignore its self-report and stated claim, and
check only what the AGENT FINAL RESULT actually evidences against the USER REQUEST.

Work through every part of the request, one at a time. For each, find the concrete proof it
was done — a file path, line or code excerpt, command output, or named test in the result. A
part counts as done only if such evidence is present; a confident or well-formatted assertion
with no locatable artifact does NOT count. Reason first, then decide. Do not reward length,
formatting, or tone — only verifiable substance.

Flag a gap only when a requested part is provably missing, a stated requirement is unmet, or a
claim has no supporting evidence. Each gap must name the specific unmet item.

If every part is evidenced — or you cannot point to a concrete unmet item — output exactly:
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

/// True when a message is a supervisor-injected note (a `<pay-attention>` advisory
/// or a `<recall>` block), not a genuine user turn. Lets the gate find the real
/// task instead of verifying against its own prior advisory.
pub fn is_supervisor_injection(content: &str) -> bool {
	let t = content.trim_start();
	t.starts_with("<pay-attention>") || t.starts_with("<recall>")
}

/// Verify a self-reported completion. `task` is the user's request, `result` is
/// the agent's final answer, `claim` is the agent's own stated reason from its
/// `done` self-report (checked against the result). Fails open (PASS) on empty
/// input or LLM error — a verifier outage must never block the agent.
pub async fn verify(
	config: &Config,
	task: &str,
	result: &str,
	claim: Option<&str>,
	operation_rx: watch::Receiver<bool>,
) -> GateVerdict {
	if task.trim().is_empty() || result.trim().is_empty() {
		return GateVerdict::Pass;
	}
	let claim_line = match claim {
		Some(c) if !c.trim().is_empty() => format!("\n\nAGENT'S STATED CLAIM: {c}"),
		_ => String::new(),
	};
	let user = format!("USER REQUEST:\n{task}\n\nAGENT FINAL RESULT:\n{result}{claim_line}");
	// Verify with a deliberately separate (ideally different-family) model — a
	// same-family verifier shares the generator's blind spots and rubber-stamps
	// them. Strict config guarantees this is set; no fallback to the generator.
	let model = config.supervisor.gate.verifier_model.clone();
	match crate::supervisor::learning::extract::call_learning_llm(
		config,
		&model,
		GATE_PROMPT.to_string(),
		user,
		crate::supervisor::stats::CallKind::Gate,
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
		"<pay-attention>\nYou reported this task complete, but a verification pass found gaps before it can be accepted as done:\n",
	);
	for g in gaps {
		s.push_str("- ");
		s.push_str(g);
		s.push('\n');
	}
	s.push_str(
		"The task is not done until each gap is closed. For each, do the work, then cite the concrete evidence that closes it — the file and line, the passing test, or the command output. If a gap is already satisfied, point to that exact evidence rather than describing it. If a gap is wrong or out of scope, say so and why. Then re-report your status.\n</pay-attention>",
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
