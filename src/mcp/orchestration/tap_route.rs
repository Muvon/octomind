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
//! an error the caller reads in the same turn, so it never lists agents first.

use crate::agent::taps::TapAgentHeader;
use crate::config::Config;

/// Lowest cosine between the task and a specialist's title and description
/// for the cosine stage alone to hand the task over. On the developer fixture
/// in `tap_route_tests.rs` a task from another field scores up to 0.58 against
/// its nearest developer specialist and a covered task scores 0.65 or more.
const ROUTE_THRESHOLD: f32 = 0.62;

/// Required gap between the best and second-best specialist. Two near-tied
/// specialists abstain: a wrong handoff is silent, a refusal costs one turn.
const ROUTE_MARGIN: f32 = 0.04;

/// How many ranked candidates the debug log shows.
const LOGGED_CANDIDATES: usize = 5;

/// Tool error when no reachable specialist fits the task.
pub const NO_SPECIALIST: &str =
	"No specialist matches this task. Do it with the tools you have, or report that it is outside what you can do.";

/// Choose the specialist for `task`, or the tool error to return.
pub async fn route_task(task: &str, config: &Config) -> Result<TapAgentHeader, String> {
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

	let ranked = rank_specialists(task, &reachable)
		.await
		.map_err(|error| format!("tap run embedding failed: {error:#}"))?;
	let preview: Vec<String> = ranked
		.iter()
		.take(LOGGED_CANDIDATES)
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

	let cosine = crate::mcp::runtime::capability::select_with_margin(ranked.clone(), ROUTE_THRESHOLD, ROUTE_MARGIN);
	match evaluate_pick(config, task, cosine, &ranked).await {
		Some((score, agent)) => {
			crate::log_debug!("tap route: picked '{}' (score {:.2})", agent.tag, score);
			Ok(agent.clone())
		}
		None => Err(NO_SPECIALIST.to_string()),
	}
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
/// shortlists, the Choice decides. Its pick gets the task at `TAP_ACTIVATE_AT`
/// or above, or at any probability when cosine picked the same one; `none` and
/// an unsure pick start nothing. Off or unavailable, the cosine decision stands.
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
	let verdict = match apply_tap_answer(cosine_tag, &tags, choice, probabilities) {
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

/// Apply the seam's rule to one Choice answer: `cosine` is the cosine winner's
/// tag, `candidates` the tags the Choice listed. `Ok(Some)` is the specialist
/// that gets the task, `Ok(None)` an abstain; `Err` names a choice that was
/// never offered. Pure, so the rule is unit-testable.
fn apply_tap_answer<'a>(
	cosine: Option<&'a str>,
	candidates: &[&'a str],
	choice: &str,
	probabilities: &std::collections::BTreeMap<String, f64>,
) -> Result<Option<&'a str>, &'static str> {
	use crate::supervisor::evaluate::{TAP_ACTIVATE_AT, TAP_NONE};

	if choice == TAP_NONE {
		return Ok(None);
	}
	let Some(chosen) = candidates.iter().copied().find(|tag| *tag == choice) else {
		return Err("invalid response");
	};
	let probability = probabilities.get(chosen).copied().unwrap_or(0.0);
	Ok((cosine == Some(chosen) || probability >= TAP_ACTIVATE_AT).then_some(chosen))
}

#[cfg(test)]
#[path = "tap_route_tests.rs"]
mod tests;
