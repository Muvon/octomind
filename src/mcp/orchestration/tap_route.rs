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

//! Picks who a task handed to `tap(action="run")` goes to.
//!
//! The calling agent sends only the task. The candidates are the agents it may
//! reach (`resolver::reachable_agents`); cosine similarity between the task
//! and each candidate's title and description ranks them. The best match gets
//! the task when it is clearly ahead. No match is
//! an error that names the closest agents; the caller may send the task
//! again with one of them as `role`, which starts it without matching.
//!
//! A task may also go to a copy of the caller (see [`reach`]): a `role` that
//! names the caller, or no `role` from a tap agent that reaches no other agent.

use crate::agent::taps::TapAgentHeader;
use crate::plugins::judgement;

/// Lowest cosine between the task and an agent's title and description
/// for the cosine stage alone to hand the task over. Calibrated 2026-11 with
/// the margin below on 2,232 task briefs written for the 186 agents of the
/// default tap (`octomind-tap/model/scripts/calibrate_cpus.py`): the pair
/// that hands over the most briefs while a wrong agent gets at most 2%
/// of them and a brief from another field fires on at most 3%. At 0.66 with
/// margin 0.05 a brief ranked against its own domain reaches its agent
/// 44% of the time, a wrong one 1.0%, and a foreign brief fires 2.5%. The
/// embedding graph scores the same on ARM and x86, so the pair holds on every
/// machine.
const ROUTE_THRESHOLD: f32 = 0.66;

/// Required gap between the best and second-best agent. Two near-tied
/// agents abstain: a wrong handoff is silent, a refusal costs one turn.
/// At 0.04 a wrong agent among all agents gets 3.0% of the briefs and a
/// foreign brief fires 3.4%.
const ROUTE_MARGIN: f32 = 0.05;

/// How many ranked candidates the debug log and a no-match error show.
const SHOWN_CANDIDATES: usize = 5;

/// Tool error when the caller reaches no agent at all.
pub const NO_AGENT: &str =
	"No agent matches this task. Do it with the tools you have, or report that it is outside what you can do.";

/// Last line of a no-match error: how the caller recovers from it.
const NO_MATCH_RECOVERY: &str =
	"If one of them covers the task, call `run` again with `role` set to its name. If none does, do the task with the tools you have, or report that it is outside what you can do.";

/// Tool error when a copy of an agent asks for another copy of itself.
pub const COPY_OF_COPY: &str =
	"You are a copy of yourself that was started for one part of the work: do this part yourself, a copy cannot start another copy.";

/// Who a `run` starts.
pub struct Routed {
	pub manifest: std::sync::Arc<crate::agent::manifest::Manifest>,
	pub title: Option<String>,
	/// The run is a copy of the caller.
	pub copy: bool,
}

#[derive(Debug)]
enum Reach<'a> {
	/// A new session of the caller itself.
	Copy,
	/// The agent `role` names, started without matching.
	Named(&'a TapAgentHeader),
	/// The task is matched among these agents.
	Match(Vec<&'a TapAgentHeader>),
}

/// Decide who a `run` reaches. `in_copy` is whether the caller is itself a
/// copy: it may not start another one.
///
/// - `role` equal to the caller's tag starts a copy of the caller only when it is itself a tap agent.
/// - Without `role`, a tap agent that reaches no other agent (no `# Uses:`
///   header) starts a copy of itself, without matching.
/// - Otherwise the task goes to the agents the caller reaches; the caller is
///   never among them.
fn reach<'a>(
	caller: &str,
	caller_is_tap_agent: bool,
	agents: &'a [TapAgentHeader],
	role: Option<&str>,
	in_copy: bool,
) -> Result<Reach<'a>, String> {
	let reachable = crate::agent::resolver::reachable_agents(caller, caller_is_tap_agent, agents)
		.map_err(|error| format!("{error:#}"))?;
	let names_itself = caller_is_tap_agent && role == Some(caller);
	let reaches_nobody_else = role.is_none() && reachable.is_empty() && caller_is_tap_agent;
	if names_itself || reaches_nobody_else {
		return if in_copy {
			Err(COPY_OF_COPY.to_string())
		} else {
			Ok(Reach::Copy)
		};
	}
	if reachable.is_empty() {
		return Err(NO_AGENT.to_string());
	}
	if let Some(role) = role {
		// A name outside the caller's reach is refused, not rerouted: the caller
		// asked for one agent and would not notice another taking the task.
		return reachable
			.iter()
			.copied()
			.find(|agent| agent.tag == role)
			.map(Reach::Named)
			.ok_or_else(|| unreachable_role(role, &reachable));
	}
	Ok(Reach::Match(reachable))
}

/// The tool error for a `role` outside the caller's reach: the names it can pass instead.
fn unreachable_role(role: &str, reachable: &[&TapAgentHeader]) -> String {
	let names: Vec<&str> = reachable.iter().map(|agent| agent.tag.as_str()).collect();
	format!(
		"`{role}` is not an agent you can reach. You can reach: {}. Call `run` again with `role` set to one of them, or without `role` to match the task.",
		names.join(", ")
	)
}

/// Choose who gets `task`, or the tool error to return. A `role` the caller
/// may reach gets the task without matching: it names that agent.
pub async fn route_task(task: &str, role: Option<&str>, config: &crate::config::Config) -> Result<Routed, String> {
	let sid = crate::session::context::current_session_id();
	let caller = sid
		.as_ref()
		.and_then(crate::session::context::get_session_role)
		.unwrap_or_default();
	let source = config.tap_manifest.as_ref().filter(|m| m.tag == caller);
	let catalog = crate::agent::manifest::catalog(sid.as_deref(), source)
		.map_err(|e| format!("Failed to enumerate tap agents: {e:#}"))?;
	let agents: Vec<_> = catalog.values().map(|m| m.header()).collect();
	let routed = |agent: &TapAgentHeader, copy| Routed {
		manifest: catalog[&agent.tag].clone(),
		title: agent.header.title.clone(),
		copy,
	};
	let in_copy = crate::session::context::current_session_mode().copy;
	// `<ns>:default` names the base agent `<ns>`.
	let role = role.map(crate::agent::registry::canonical_tag);
	let caller_is_tap_agent = source.is_some();
	let reachable = match reach(&caller, caller_is_tap_agent, &agents, role, in_copy)? {
		Reach::Copy => {
			crate::log_debug!("tap route: caller={:?} starts a copy of itself", caller);
			let header = agents
				.iter()
				.find(|a| a.tag == caller)
				.expect("tap caller is in its catalog");
			return Ok(routed(header, true));
		}
		Reach::Named(agent) => {
			crate::log_debug!("tap route: caller={:?} named '{}'", caller, agent.tag);
			return Ok(routed(agent, false));
		}
		Reach::Match(reachable) => reachable,
	};

	let ranked = rank_agents(task, &reachable)
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

	let cosine = crate::mcp::runtime::capability::select_with_margin(ranked.clone(), ROUTE_THRESHOLD, ROUTE_MARGIN);
	match evaluate_pick(task, cosine, &ranked).await {
		Some((score, agent)) => {
			crate::log_debug!("tap route: picked '{}' (score {:.2})", agent.tag, score);
			Ok(routed(agent, false))
		}
		None => Err(closest_agents("No agent was matched to this task.", &ranked)),
	}
}

/// The tool error for a task that starts nothing: `reason`, the best-ranked
/// agents by name, and how the caller recovers. This list is the only
/// place the caller learns a name, so it never has to guess one.
fn closest_agents(reason: &str, ranked: &[(f32, &TapAgentHeader)]) -> String {
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
async fn rank_agents<'a>(
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

/// The text an agent is matched on and a plugin's judgement reads.
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

/// A plugin picks among the best cosine matches: cosine only shortlists, the
/// choice decides. The agent it names gets the task, whatever cosine
/// ranked first; `none` starts nothing. With no answer, the cosine decision
/// stands.
async fn evaluate_pick<'a>(
	task: &str,
	cosine: Option<(f32, &'a TapAgentHeader)>,
	ranked: &[(f32, &'a TapAgentHeader)],
) -> Option<(f32, &'a TapAgentHeader)> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return cosine;
	};
	let candidates = &ranked[..ranked.len().min(judgement::TAP_TOP_K)];
	let descriptions: Vec<String> = candidates.iter().map(|(_, agent)| describe(agent)).collect();
	let Some(pick) = judgement::TAPS
		.pick(
			&session_id,
			task,
			candidates
				.iter()
				.zip(descriptions.iter())
				.map(|((_, agent), description)| (agent.tag.as_str(), description.as_str())),
		)
		.await
	else {
		return cosine;
	};
	let cosine_tag = cosine.map(|(_, agent)| agent.tag.as_str());
	let tags: Vec<&str> = candidates.iter().map(|(_, agent)| agent.tag.as_str()).collect();
	let picked = match apply_tap_answer(&tags, &pick.choice) {
		Some(tag) if cosine_tag == Some(tag) => cosine,
		Some(tag) => candidates
			.iter()
			.find(|(_, agent)| agent.tag == tag)
			.map(|(_, agent)| (pick.probability(tag) as f32, *agent)),
		None => None,
	};
	if picked.map(|(_, agent)| agent.tag.as_str()) != cosine_tag {
		crate::log_debug!(
			"tap judgement: {} → {} (choice '{}' p={:.2})",
			cosine_tag.unwrap_or("none"),
			picked.map(|(_, agent)| agent.tag.as_str()).unwrap_or("none"),
			pick.choice,
			pick.probability(&pick.choice)
		);
		pick.applied(&session_id);
	}
	picked
}

/// The agent the choice names, among the tags it listed: `none` names
/// no agent. No probability bar: `choice` is already the likeliest option
/// of the list, `none` included, and that list spreads the mass over up to
/// `TAP_TOP_K` agents. Measured 2026-11 with live Jev over the 14
/// developer agents: three of four 2 KB briefs from a real lead got their
/// top option at 0.30 to 0.51, so a 0.6 bar refused them; on 168 written briefs
/// the bar kept the same 158 right picks and only turned 2 of 8 wrong picks
/// into refusals.
fn apply_tap_answer<'a>(candidates: &[&'a str], choice: &str) -> Option<&'a str> {
	candidates.iter().copied().find(|tag| *tag == choice)
}

#[cfg(test)]
#[path = "tap_route_tests.rs"]
mod tests;
