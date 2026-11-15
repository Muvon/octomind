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

//! Picks the specialist for a task handed to `tap(action="run")`.
//!
//! The calling agent sends only the task. The candidates are the agents it may
//! reach (`resolver::reachable_specialists`); cosine similarity between the task
//! and each candidate's title and description ranks them. With the `taps`
//! decision seam on, that ranking is a shortlist and the seam picks from it;
//! with it off, the best match gets the task when it is clearly ahead. No match is
//! an error that names the closest specialists; the caller may send the task
//! again with one of them as `role`, which starts it without matching.

use crate::agent::taps::{TapAgentHeader, ORCHESTRATOR_SPEC};
use crate::config::Config;

/// Lowest cosine between the task and a specialist's title and description
/// for the cosine stage alone to hand the task over. Calibrated 2026-11 with
/// the margin below on 2,232 task briefs written for the 186 agents of the
/// default tap (`octomind-tap/model/scripts/calibrate_cpus.py`): the pair
/// that hands over the most briefs while a wrong specialist gets at most 2%
/// of them and a brief from another field fires on at most 3%. At 0.65 with
/// margin 0.05 a brief ranked against its own domain reaches its specialist
/// 46% of the time, a wrong one 1.1%, and a foreign brief fires 2.7%. The
/// embedding graph scores the same on ARM and x86, so the pair holds on every
/// machine.
const ROUTE_THRESHOLD: f32 = 0.65;

/// Required gap between the best and second-best specialist. Two near-tied
/// specialists abstain: a wrong handoff is silent, a refusal costs one turn.
/// At 0.04 a wrong specialist among all agents gets 3.0% of the briefs and a
/// foreign brief fires 3.4%.
const ROUTE_MARGIN: f32 = 0.05;

/// How many ranked candidates the debug log and a no-match error show.
const SHOWN_CANDIDATES: usize = 5;

/// Tool error when the caller reaches no specialist at all.
pub const NO_SPECIALIST: &str =
	"No specialist matches this task. Do it with the tools you have, or report that it is outside what you can do.";

/// Last line of a no-match error: how the caller recovers from it.
const NO_MATCH_RECOVERY: &str =
	"If one of them covers the task, call `run` again with `role` set to its name. If none does, do the task with the tools you have, or report that it is outside what you can do.";

/// Choose the specialist for `task`, or the tool error to return. A `role` the
/// caller may reach gets the task without matching: a full tag names that
/// agent, a bare domain its `chief`.
pub async fn route_task(task: &str, role: Option<&str>, config: &Config) -> Result<TapAgentHeader, String> {
	let agents = crate::agent::taps::list_agent_headers()
		.map_err(|error| format!("Failed to enumerate tap agents: {error:#}"))?;
	let caller = crate::session::context::current_session_id()
		.and_then(|session_id| crate::session::context::get_session_role(&session_id))
		.unwrap_or_default();
	let reachable =
		crate::agent::resolver::reachable_specialists(&caller, &agents).map_err(|error| format!("{error:#}"))?;
	if reachable.is_empty() {
		return Err(NO_SPECIALIST.to_string());
	}

	if let Some(role) = role {
		let chief = format!("{role}:{ORCHESTRATOR_SPEC}");
		if let Some(agent) = reachable.iter().find(|agent| agent.tag == role || agent.tag == chief) {
			crate::log_debug!("tap route: caller={:?} named '{}'", caller, agent.tag);
			return Ok((*agent).clone());
		}
	}

	let ranked = rank_specialists(task, &reachable)
		.await
		.map_err(|error| format!("tap run embedding failed: {error:#}"))?;
	let preview: Vec<String> = ranked
		.iter()
		.take(SHOWN_CANDIDATES)
		.map(|(score, agent)| format!("{}={score:.3}", agent.tag))
		.collect();
	crate::log_debug!(
		"tap route: caller={:?} candidates={} threshold={} margin={} top=[{}]",
		caller,
		ranked.len(),
		ROUTE_THRESHOLD,
		ROUTE_MARGIN,
		preview.join(", ")
	);

	// A name outside the caller's reach is refused, not rerouted: the caller
	// asked for one specialist and would not notice another taking the task.
	if let Some(role) = role {
		return Err(closest_specialists(
			&format!("`{role}` is not a specialist you can reach."),
			&ranked,
		));
	}

	let cosine = crate::mcp::runtime::capability::select_with_margin(ranked.clone(), ROUTE_THRESHOLD, ROUTE_MARGIN);
	match evaluate_pick(config, task, cosine, &ranked).await {
		Some((score, agent)) => {
			crate::log_debug!("tap route: picked '{}' (score {:.2})", agent.tag, score);
			Ok(agent.clone())
		}
		None => Err(closest_specialists("No specialist was matched to this task.", &ranked)),
	}
}

/// The tool error for a task that starts nothing: `reason`, the best-ranked
/// specialists by name, and how the caller recovers. This list is the only
/// place the caller learns a name, so it never has to guess one.
fn closest_specialists(reason: &str, ranked: &[(f32, &TapAgentHeader)]) -> String {
	let lines: Vec<String> = ranked
		.iter()
		.take(SHOWN_CANDIDATES)
		.map(|(_, agent)| {
			let about = describe(agent);
			if about == agent.tag {
				format!("- {}", agent.tag)
			} else {
				format!("- {} — {about}", agent.tag)
			}
		})
		.collect();
	format!(
		"{reason} The closest ones to this task:\n{}\n{NO_MATCH_RECOVERY}",
		lines.join("\n")
	)
}

/// `candidates` with their cosine to `task`, best first.
async fn rank_specialists<'a>(
	task: &str,
	candidates: &[&'a TapAgentHeader],
) -> anyhow::Result<Vec<(f32, &'a TapAgentHeader)>> {
	// `embed` loads the model on first use: a delegation waits for it once
	// instead of failing while it warms up.
	let task_vec = crate::embeddings::embed(task).await?;
	let corpus: Vec<String> = candidates.iter().map(|agent| describe(agent)).collect();
	let corpus_vecs = crate::embeddings::embed_many(&corpus).await?;
	let mut ranked: Vec<(f32, &TapAgentHeader)> = candidates
		.iter()
		.zip(corpus_vecs.iter())
		.map(|(agent, vec)| (crate::embeddings::cosine(&task_vec, vec), *agent))
		.collect();
	ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	Ok(ranked)
}

/// The text a specialist is matched on and the decision seam reads.
fn describe(agent: &TapAgentHeader) -> String {
	let parts: Vec<&str> = [agent.header.title.as_deref(), agent.header.description.as_deref()]
		.into_iter()
		.flatten()
		.collect();
	if parts.is_empty() {
		agent.tag.clone()
	} else {
		parts.join(". ")
	}
}

/// The decision seam picks among the best cosine matches: cosine only
/// shortlists, the Choice decides. The specialist it names gets the task,
/// whatever cosine ranked first; `none` starts nothing. Off or unavailable,
/// the cosine decision stands.
async fn evaluate_pick<'a>(
	config: &Config,
	task: &str,
	cosine: Option<(f32, &'a TapAgentHeader)>,
	ranked: &[(f32, &'a TapAgentHeader)],
) -> Option<(f32, &'a TapAgentHeader)> {
	use crate::supervisor::evaluate::{self, Seam};

	if !evaluate::enabled(config, Seam::Taps) {
		return cosine;
	}
	let candidates = &ranked[..ranked.len().min(evaluate::TAP_TOP_K)];
	let descriptions: Vec<String> = candidates.iter().map(|(_, agent)| describe(agent)).collect();
	let question = evaluate::tap_question(
		candidates
			.iter()
			.zip(descriptions.iter())
			.map(|((_, agent), description)| (agent.tag.as_str(), description.as_str())),
	);
	let questions = std::collections::BTreeMap::from([(evaluate::TAP_QUESTION_ID.to_string(), question)]);
	let Some(answers) = evaluate::run(
		config,
		Seam::Taps,
		serde_json::Value::String(task.to_string()),
		questions,
	)
	.await
	else {
		return cosine;
	};
	// The runner answers every asked id with its own type or a refusal.
	let Some(octolib::evaluation::Answer::Choice {
		choice, probabilities, ..
	}) = answers.get(evaluate::TAP_QUESTION_ID)
	else {
		evaluate::unavailable(Seam::Taps, "refused");
		return cosine;
	};
	let cosine_tag = cosine.map(|(_, agent)| agent.tag.as_str());
	let tags: Vec<&str> = candidates.iter().map(|(_, agent)| agent.tag.as_str()).collect();
	let verdict = match apply_tap_answer(&tags, choice) {
		Ok(verdict) => verdict,
		Err(reason) => {
			evaluate::unavailable(Seam::Taps, reason);
			return cosine;
		}
	};
	let probability = |tag: &str| probabilities.get(tag).copied().unwrap_or(0.0);
	let picked = match verdict {
		Some(tag) if cosine_tag == Some(tag) => cosine,
		Some(tag) => candidates
			.iter()
			.find(|(_, agent)| agent.tag == tag)
			.map(|(_, agent)| (probability(tag) as f32, *agent)),
		None => None,
	};
	if picked.map(|(_, agent)| agent.tag.as_str()) != cosine_tag {
		crate::log_debug!(
			"tap evaluate: {} → {} (choice '{}' p={:.2})",
			cosine_tag.unwrap_or("none"),
			picked.map(|(_, agent)| agent.tag.as_str()).unwrap_or("none"),
			choice,
			probability(choice)
		);
		crate::supervisor::stats::evaluate_applied(Seam::Taps, 1);
	}
	picked
}

/// Apply the seam's rule to one Choice answer: `candidates` are the tags the
/// Choice listed. `Ok(Some)` is the specialist that gets the task, `Ok(None)`
/// an abstain; `Err` names a choice that was never offered. No probability
/// bar: `choice` is already the likeliest option of the list, `none` included,
/// and that list spreads the mass over up to `TAP_TOP_K` specialists. Measured
/// 2026-11 with live Jev over the 14 developer specialists: three of four 2 KB
/// briefs from a real lead got their top option at 0.30 to 0.51, so a 0.6 bar
/// refused them; on 168 written briefs the bar kept the same 158 right picks
/// and only turned 2 of 8 wrong picks into refusals.
fn apply_tap_answer<'a>(candidates: &[&'a str], choice: &str) -> Result<Option<&'a str>, &'static str> {
	if choice == crate::supervisor::evaluate::TAP_NONE {
		return Ok(None);
	}
	candidates
		.iter()
		.copied()
		.find(|tag| *tag == choice)
		.map(Some)
		.ok_or("invalid response")
}

#[cfg(test)]
#[path = "tap_route_tests.rs"]
mod tests;
