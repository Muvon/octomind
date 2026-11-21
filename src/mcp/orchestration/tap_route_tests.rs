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

//! The route's error text, the judgement over the cosine pick (the ranked
//! candidates are the input, a stub plugin answers) and the ranking of a task
//! against a roster of specialists.

use super::*;
use crate::agent::registry::AgentHeader;
use crate::plugins::judgement::TAP_TOP_K;
use crate::plugins::test_support::{connect_stub, StubPlugin};
use crate::session::context::with_session_id;
use serial_test::serial;

fn agent(tag: &str) -> TapAgentHeader {
	TapAgentHeader {
		category: tag.split(':').next().expect("domain").to_string(),
		tag: tag.to_string(),
		header: AgentHeader {
			title: Some(format!("{tag} title")),
			description: Some(format!("{tag} description")),
			uses: Vec::new(),
		},
	}
}

#[test]
fn describe_joins_title_and_description_and_falls_back_to_the_tag() {
	assert_eq!(describe(&agent("law:us")), "law:us title. law:us description");

	let mut untitled = agent("law:us");
	untitled.header.title = None;
	assert_eq!(describe(&untitled), "law:us description");

	untitled.header.description = None;
	assert_eq!(describe(&untitled), "law:us");
}

#[test]
fn a_no_match_error_names_the_best_ranked_specialists_and_the_recovery_step() {
	let mut agents: Vec<TapAgentHeader> = (0..SHOWN_CANDIDATES + 2).map(|i| agent(&format!("d{i}:x"))).collect();
	agents[1].header.title = None;
	agents[1].header.description = None;
	let ranked: Vec<(f32, &TapAgentHeader)> = agents
		.iter()
		.enumerate()
		.map(|(i, agent)| (0.5 - i as f32 * 0.01, agent))
		.collect();

	let error = closest_specialists("No specialist was matched to this task.", &ranked);

	let lines: Vec<&str> = error.lines().collect();
	assert_eq!(
		lines[0],
		"No specialist was matched to this task. The closest ones to this task:"
	);
	assert_eq!(lines[1], "- d0:x — d0:x title. d0:x description");
	// An agent with no title and no description is listed by name alone.
	assert_eq!(lines[2], "- d1:x");
	// The best `SHOWN_CANDIDATES` by score, then the recovery step; the two worst are left out.
	assert_eq!(lines.len(), SHOWN_CANDIDATES + 2);
	assert_eq!(lines[SHOWN_CANDIDATES + 1], NO_MATCH_RECOVERY);
	assert!(!error.contains(&format!("d{SHOWN_CANDIDATES}:x")));
}

fn tap_choice(choice: &str, probabilities: &[(&str, f64)]) -> serde_json::Value {
	let probabilities: std::collections::BTreeMap<&str, f64> = probabilities.iter().copied().collect();
	serde_json::json!({"judgements": {"specialist": {
		"type": "choice",
		"choice": choice,
		"probabilities": probabilities,
	}}})
}

async fn judging_plugin(sid: &str, answers: Vec<serde_json::Value>) -> StubPlugin {
	let stub = StubPlugin::new(&["judge"]).queue(answers);
	connect_stub(sid, "developer", std::path::Path::new("/tmp"), &stub).await;
	stub
}

async fn pick_in<'a>(
	sid: &str,
	task: &str,
	cosine: Option<(f32, &'a TapAgentHeader)>,
	ranked: &[(f32, &'a TapAgentHeader)],
) -> Option<(f32, &'a TapAgentHeader)> {
	with_session_id(sid.to_string(), evaluate_pick(task, cosine, ranked)).await
}

#[test]
fn the_named_specialist_gets_the_task_and_none_starts_nothing() {
	let candidates = ["a:x", "b:x"];
	assert_eq!(apply_tap_answer(&candidates, "b:x"), Some("b:x"));
	assert_eq!(apply_tap_answer(&candidates, judgement::NONE), None);
}

#[tokio::test]
async fn evaluate_pick_keeps_the_cosine_decision_without_a_plugin_that_judges() {
	let agents = [agent("a:x"), agent("b:x")];
	let ranked: Vec<(f32, &TapAgentHeader)> = vec![(0.9, &agents[0]), (0.5, &agents[1])];

	let picked = pick_in("tap-judge-absent", "do the a thing", Some(ranked[0]), &ranked).await;

	assert_eq!(picked.map(|(_, agent)| agent.tag.as_str()), Some("a:x"));
}

#[tokio::test]
#[serial]
async fn evaluate_pick_asks_over_the_top_candidates_and_can_veto_the_cosine_winner() {
	let sid = "tap-judge-veto";
	let stub = judging_plugin(sid, vec![tap_choice("none", &[("none", 0.99)])]).await;
	let tags: Vec<String> = (0..TAP_TOP_K + 2).map(|i| format!("d{i:02}:x")).collect();
	let agents: Vec<TapAgentHeader> = tags.iter().map(|tag| agent(tag)).collect();
	let ranked: Vec<(f32, &TapAgentHeader)> = agents
		.iter()
		.enumerate()
		.map(|(i, agent)| (0.9 - i as f32 * 0.01, agent))
		.collect();

	let picked = pick_in(sid, "unrelated task", Some(ranked[0]), &ranked).await;

	assert!(picked.is_none());
	let asked = stub.sent("judge");
	assert_eq!(asked.len(), 1);
	assert_eq!(asked[0]["purpose"], "taps");
	// The task is the whole subject: the plugin reads what the specialist would.
	assert_eq!(asked[0]["subject"], "unrelated task");
	let options = asked[0]["questions"]["specialist"]["options"]
		.as_object()
		.expect("options");
	// The best `TAP_TOP_K` by score plus `none`; the two worst are left out.
	let mut listed: Vec<String> = tags[..TAP_TOP_K].to_vec();
	listed.push("none".to_string());
	assert_eq!(options.keys().cloned().collect::<Vec<_>>(), listed);
	assert_eq!(options["d00:x"], "d00:x title. d00:x description");
	crate::plugins::end_session(sid);
}

#[tokio::test]
#[serial]
async fn evaluate_pick_follows_the_choice_at_any_probability() {
	let sid = "tap-judge-follow";
	// 0.3 is a top option of a wide list: the mass is spread over the other candidates.
	let answer = tap_choice("b:x", &[("b:x", 0.3)]);
	let stub = judging_plugin(sid, vec![answer.clone(), answer]).await;
	let agents = [agent("a:x"), agent("b:x")];
	let ranked: Vec<(f32, &TapAgentHeader)> = vec![(0.5, &agents[0]), (0.49, &agents[1])];

	// Cosine abstained: the choice's specialist gets the task.
	let picked = pick_in(sid, "do the b thing", None, &ranked).await;
	assert_eq!(
		picked.map(|(score, agent)| (score, agent.tag.as_str())),
		Some((0.3, "b:x"))
	);

	// Cosine picked another one: the choice's specialist replaces it.
	let picked = pick_in(sid, "do the b thing", Some(ranked[0]), &ranked).await;
	assert_eq!(
		picked.map(|(score, agent)| (score, agent.tag.as_str())),
		Some((0.3, "b:x"))
	);

	// Both changed the outcome; the plugin hears of one of them with its next event.
	assert_eq!(stub.sent("judge")[1]["judged"]["taps"], 1);
	crate::plugins::end_session(sid);
}

#[tokio::test]
#[serial]
async fn evaluate_pick_keeps_cosine_when_the_plugin_gives_no_usable_answer() {
	let sid = "tap-judge-unusable";
	// An empty answer, then a choice that was never offered.
	let stub = judging_plugin(sid, vec![serde_json::json!({}), tap_choice("z:x", &[("z:x", 0.9)])]).await;
	let agents = [agent("a:x"), agent("b:x")];
	let ranked: Vec<(f32, &TapAgentHeader)> = vec![(0.9, &agents[0]), (0.5, &agents[1])];

	for _ in 0..2 {
		let picked = pick_in(sid, "do the a thing", Some(ranked[0]), &ranked).await;
		assert_eq!(picked.map(|(_, agent)| agent.tag.as_str()), Some("a:x"));
	}
	assert_eq!(stub.sent("judge").len(), 2);
	crate::plugins::end_session(sid);
}

// --- matching against a real roster ------------------------------------------

/// The `developer` domain of the default tap as its lead reaches it: tag,
/// title, description, copied from the manifests' headers.
const DEVELOPER_ROSTER: [(&str, &str, &str); 14] = [
	("developer:brief", "Code Change Briefer – Git Diff Summary", "Turns a diff, commit, or range into plain-language review cards: behavioral impact, traced risk, and source links behind every finding."),
	("developer:changelog", "Changelog Maintainer – CHANGELOG.md Generator", "Generates a Keep a Changelog 1.1.0 CHANGELOG.md from a commit range, verifying every entry against current code instead of commit subjects."),
	("developer:context", "Context Curator", "Explores codebase, recalls memories, and narrows entry points. Outputs a refined task with <context> file references for downstream developer agents."),
	("developer:doc", "Documentation Custodian", "Audits docs for staleness against code and git history, syncs drifted claims with minimum-diff edits, and writes new docs only when something is missing."),
	("developer:general", "General Developer", "Implements bug fixes, features, and refactors: researches upstream fixes, surveys every affected surface, fixes root causes, and verifies with tests."),
	("developer:learn", "Project Memory Curator", "Builds the project's long-term memory by anchoring every entry to verified code references, then refines it with user corrections."),
	("developer:plan", "Implementation Plan Architect", "Turns a full spec or a rough ask into a phased, parallelizable implementation plan with disjoint task ownership, merge gates, and spec coverage."),
	("developer:qa", "QA & Test Engineer – Test Plans & Bug Reports", "Designs risk-based test plans with boundary, equivalence, and state-transition techniques, runs what's runnable, writes bug reports precise enough to fix."),
	("developer:readme", "README Architect – README Generator", "Writes the project README after exploring the codebase: detects project type and audience, verifies every command by running it, and keeps it scannable."),
	("developer:release-notes", "Release Notes Writer", "Generates detailed, user-facing release notes from a commit/version range by reading code, not commit messages; prepends the section to a target file."),
	("developer:reverse-spec", "Reverse Specification Writer", "Reconstructs the task behind a commit, branch, or diff: the prompt as it likely arrived, plus a clarified spec with every hunk bucketed by intent."),
	("developer:social-preview", "GitHub Social Preview Designer – Open Graph Image", "GitHub social preview designer: builds a 1280x640 Open Graph image from the repo's own brand, checks it at feed size, and verifies the upload."),
	("developer:spec", "Behavioral Specification Writer", "Turns a task into a traceable EARS spec: numbered requirements, explicit constraints, scenarios, and a verification command, with no unstated assumptions."),
	("developer:tester", "Test Implementation & Coverage Engineer", "Writes behavior-proving tests in any language, closes risk-ranked coverage gaps, sets up mutation tooling, and proves every assertion fails first."),
];

/// Briefs as the developer lead is told to write them (first line names the
/// kind of work and the deliverable), with the specialist that should take
/// each; `None` is a task no developer specialist covers.
const DEVELOPER_TASKS: [(&str, Option<&str>); 19] = [
	("Implement a bug fix: the parser in src/parser.rs panics when the input is empty. Find the root cause, patch it, and run the test suite.", Some("developer:general")),
	("Write unit tests for the retry module in src/net/retry.rs covering the backoff edge cases, and show each test fails without the fix.", Some("developer:tester")),
	("Design a risk-based test plan for the checkout flow, run what can be run, and write bug reports for what fails.", Some("developer:qa")),
	("Summarize the diff between v1.2.0 and HEAD as review cards with the behavioral impact and the risks of each change.", Some("developer:brief")),
	("Generate the CHANGELOG.md entries for the commits since the v0.70.0 tag in Keep a Changelog format.", Some("developer:changelog")),
	("Write user-facing release notes for version 0.71.0 from the commit range v0.70.0..HEAD.", Some("developer:release-notes")),
	("Explore the codebase to find where session compression is decided and return the task with file references for the developer who will change it.", Some("developer:context")),
	("Break this feature spec into a phased implementation plan with parallel tasks, clear ownership, and merge gates.", Some("developer:plan")),
	("Write a behavioral specification with numbered EARS requirements, constraints, and scenarios for the new rate limiter.", Some("developer:spec")),
	("Audit the docs folder for claims that no longer match the code and fix the stale ones with minimal edits.", Some("developer:doc")),
	("Write the project README: what the project is, how to install it, and usage commands you have verified by running them.", Some("developer:readme")),
	("Reconstruct what task commit 4f2a9c1 was solving and write the clarified spec it implemented.", Some("developer:reverse-spec")),
	("Create the GitHub social preview image for this repository from its brand colors and logo.", Some("developer:social-preview")),
	("Record what we learned about the build system in the project's long-term memory, each entry with its code reference.", Some("developer:learn")),
	("Implement a feature: add a --json flag to the export command, update its help text, and cover it with a test.", Some("developer:general")),
	("Draft a cold outreach email to enterprise prospects about our pricing change.", None),
	("Review this employment contract for non-compete clauses under California law.", None),
	("Plan a three-day vegetarian menu for a family of four.", None),
	("Interpret these blood test results and say which values are out of range.", None),
];

#[tokio::test]
#[serial]
async fn developer_tasks_reach_their_specialist_and_foreign_tasks_reach_none() {
	let roster: Vec<TapAgentHeader> = DEVELOPER_ROSTER
		.iter()
		.map(|(tag, title, description)| TapAgentHeader {
			category: "developer".to_string(),
			tag: tag.to_string(),
			header: AgentHeader {
				title: Some(title.to_string()),
				description: Some(description.to_string()),
				uses: Vec::new(),
			},
		})
		.collect();
	let candidates: Vec<&TapAgentHeader> = roster.iter().collect();

	let mut wrong = Vec::new();
	let mut missed = Vec::new();
	for (task, expected) in DEVELOPER_TASKS {
		let ranked = rank_specialists(task, &candidates).await.expect("rank");
		let picked = crate::mcp::runtime::capability::select_with_margin(ranked.clone(), ROUTE_THRESHOLD, ROUTE_MARGIN)
			.map(|(_, agent)| agent.tag.as_str());
		eprintln!(
			"route fixture: expected={:?} picked={:?} top1={}={:.3} top2={}={:.3} | {task}",
			expected, picked, ranked[0].1.tag, ranked[0].0, ranked[1].1.tag, ranked[1].0
		);
		match (expected, picked) {
			(expected, picked) if expected == picked => {}
			// A refusal costs the caller one turn; it then does the task itself.
			(Some(_), None) => missed.push(task),
			// A wrong handoff is silent, so none is tolerated.
			_ => wrong.push(task),
		}
	}
	assert!(wrong.is_empty(), "handed to the wrong specialist: {wrong:#?}");
	assert!(
		missed.len() <= MAX_MISSED_DEVELOPER_TASKS,
		"{} developer tasks found no specialist: {missed:#?}",
		missed.len()
	);
}

/// Developer briefs the cosine stage may refuse before the fixture fails.
const MAX_MISSED_DEVELOPER_TASKS: usize = 3;
