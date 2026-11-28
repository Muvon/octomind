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
//! against a roster of agents.

use super::*;
use crate::agent::registry::AgentHeader;
use crate::plugins::judgement::TAP_TOP_K;
use crate::plugins::test_support::{connect_stub, StubPlugin};
use crate::session::context::with_session_id;
use serial_test::serial;

fn agent(tag: &str) -> TapAgentHeader {
	TapAgentHeader {
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
fn a_no_match_error_names_the_best_ranked_agents_and_the_recovery_step() {
	let mut agents: Vec<TapAgentHeader> = (0..SHOWN_CANDIDATES + 2).map(|i| agent(&format!("d{i}:x"))).collect();
	agents[1].header.title = None;
	agents[1].header.description = None;
	let ranked: Vec<(f32, &TapAgentHeader)> = agents
		.iter()
		.enumerate()
		.map(|(i, agent)| (0.5 - i as f32 * 0.01, agent))
		.collect();

	let error = closest_agents("No agent was matched to this task.", &ranked);

	let lines: Vec<&str> = error.lines().collect();
	assert_eq!(
		lines[0],
		"No agent was matched to this task. The closest ones to this task:"
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
fn the_named_agent_gets_the_task_and_none_starts_nothing() {
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
	// The task is the whole subject: the plugin reads what the agent would.
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

	// Cosine abstained: the choice's agent gets the task.
	let picked = pick_in(sid, "do the b thing", None, &ranked).await;
	assert_eq!(
		picked.map(|(score, agent)| (score, agent.tag.as_str())),
		Some((0.3, "b:x"))
	);

	// Cosine picked another one: the choice's agent replaces it.
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

// --- who a run reaches -------------------------------------------------------

fn agent_using(tag: &str, uses: &[&str]) -> TapAgentHeader {
	let mut agent = agent(tag);
	agent.header.uses = uses.iter().map(|entry| entry.to_string()).collect();
	agent
}

fn match_tags<'a>(reach: Result<Reach<'a>, String>) -> Vec<&'a str> {
	match reach {
		Ok(Reach::Match(candidates)) => candidates.iter().map(|agent| agent.tag.as_str()).collect(),
		_ => panic!("expected the task to be matched among candidates"),
	}
}

#[test]
fn a_role_naming_the_caller_starts_a_copy_of_the_caller() {
	let agents = [agent_using("alpha", &["beta"]), agent("beta")];
	assert!(matches!(
		reach("alpha", true, &agents, Some("alpha"), false),
		Ok(Reach::Copy)
	));
}

#[test]
fn a_tap_agent_without_uses_starts_a_copy_without_matching() {
	let agents = [agent("alpha"), agent("beta")];
	assert!(matches!(reach("alpha", true, &agents, None, false), Ok(Reach::Copy)));
	// A role other than its own is not reachable for an agent that reaches nobody.
	assert_eq!(
		reach("alpha", true, &agents, Some("beta"), false).err().as_deref(),
		Some(NO_AGENT)
	);
}

#[test]
fn a_caller_that_reaches_agents_matches_among_them_and_never_itself() {
	let agents = [agent_using("alpha", &["beta", "gamma"]), agent("beta"), agent("gamma")];
	assert_eq!(
		match_tags(reach("alpha", true, &agents, None, false)),
		["beta", "gamma"]
	);

	let everyone = [agent_using("alpha", &["*"]), agent("beta"), agent("gamma")];
	assert_eq!(
		match_tags(reach("alpha", true, &everyone, None, false)),
		["beta", "gamma"]
	);
}

#[test]
fn a_role_the_caller_reaches_is_named_and_one_it_does_not_reach_is_refused_with_the_names() {
	let agents = [agent_using("alpha", &["beta"]), agent("beta"), agent("gamma")];
	assert!(
		matches!(reach("alpha", true, &agents, Some("beta"), false), Ok(Reach::Named(named)) if named.tag == "beta")
	);
	let refusal = reach("alpha", true, &agents, Some("gamma"), false).expect_err("gamma is outside the reach");
	assert!(
		refusal.starts_with("`gamma` is not an agent you can reach. You can reach: beta."),
		"{refusal}"
	);
}

#[test]
fn a_base_agent_matches_among_its_variants_and_a_variant_starts_a_copy_of_itself() {
	let agents = [agent("dev"), agent("dev:rust"), agent("dev:go"), agent("other")];
	assert_eq!(
		match_tags(reach("dev", true, &agents, None, false)),
		["dev:go", "dev:rust"]
	);
	assert!(
		matches!(reach("dev", true, &agents, Some("dev:rust"), false), Ok(Reach::Named(named)) if named.tag == "dev:rust")
	);

	// A variant reaches nobody, not even its siblings, so it can only start a copy of itself.
	assert!(matches!(reach("dev:rust", true, &agents, None, false), Ok(Reach::Copy)));
	assert!(matches!(
		reach("dev:rust", true, &agents, Some("dev:rust"), false),
		Ok(Reach::Copy)
	));
	assert_eq!(
		reach("dev:rust", true, &agents, Some("dev:go"), false).err().as_deref(),
		Some(NO_AGENT)
	);
	assert_eq!(
		reach("dev:rust", true, &agents, None, true).err().as_deref(),
		Some(COPY_OF_COPY)
	);
}

#[test]
fn a_caller_that_is_not_a_tap_agent_matches_among_all_agents_but_cannot_copy_itself() {
	let agents = [agent("beta"), agent("gamma")];
	assert_eq!(
		match_tags(reach("local-role", false, &agents, None, false)),
		["beta", "gamma"]
	);
	assert!(reach("local-role", false, &agents, Some("local-role"), false).is_err());
}

#[test]
fn a_local_role_named_like_a_tap_agent_is_not_treated_as_that_agent() {
	// `alpha` the tap agent reaches nobody and would copy itself; the local role `alpha` reaches the rest.
	let agents = [agent("alpha"), agent("beta")];
	assert!(matches!(reach("alpha", true, &agents, None, false), Ok(Reach::Copy)));
	assert_eq!(match_tags(reach("alpha", false, &agents, None, false)), ["beta"]);
}

#[test]
fn a_copy_cannot_start_a_copy_but_keeps_its_other_reach() {
	let agents = [agent_using("alpha", &["beta"]), agent("beta")];
	assert_eq!(
		reach("alpha", true, &agents, Some("alpha"), true).err().as_deref(),
		Some(COPY_OF_COPY)
	);
	assert_eq!(match_tags(reach("alpha", true, &agents, None, true)), ["beta"]);
	assert!(matches!(
		reach("alpha", true, &agents, Some("beta"), true),
		Ok(Reach::Named(_))
	));

	let alone = [agent("alpha"), agent("beta")];
	assert_eq!(
		reach("alpha", true, &alone, None, true).err().as_deref(),
		Some(COPY_OF_COPY)
	);
}

// --- matching against a real roster ------------------------------------------

/// The roster of the default tap as the `assistant` agent reaches it: every
/// base agent except `assistant` itself, as tag (the agent's directory name),
/// title, description, copied from the manifests' headers.
const AGENT_ROSTER: [(&str, &str, &str); 33] = [
	("academic", "AI Research Assistant – Literature Reviews & Papers", "An AI research assistant for literature reviews, research questions, thesis statements, paper drafts, abstracts, reviewer replies and citation checks."),
	("ai-engineer", "AI Engineer – LLM Apps, Evals, Red Teaming", "Builds LLM and RAG apps, designs evals, red-teams prompts and agents, and classifies AI systems under the EU AI Act. Delivers specs, reports and memos."),
	("analyst", "AI Data Analyst – SQL, Pipelines & Spreadsheets", "Answers data questions end to end: data analysis and statistics, SQL queries, warehouse pipelines, forecasting models, and Excel or Google Sheets formulas."),
	("chef", "AI Recipe Generator – Meal Plans & Grocery Lists", "Suggests recipes from your ingredients, plans weekly meals with budget-capped priced grocery lists, scales recipes, and coaches technique with food safety."),
	("coach", "AI Coach – Goals, Career, Habits & Accountability", "An AI life coach for goals and accountability, career and job search, resumes, interviews, negotiation, speaking, habits, fitness and wellbeing."),
	("community-manager", "Discord Server Setup & Moderation – Community Manager", "Plans and runs online communities: Discord server setup blueprints, moderation rules and AutoMod packs, onboarding, engagement rituals and health audits."),
	("designer", "AI Design Assistant – UX Audits, Accessibility, Logos", "An AI design assistant for UX audits and microcopy, WCAG and EAA accessibility audits, user research synthesis, usability test plans, logos and brand kits."),
	("developer", "AI Coding Assistant – Fix Bugs & Build Features", "An AI coding assistant that fixes bugs, builds features and refactors, writes tests, specs, docs and changelogs, and reviews diffs in plain language."),
	("devops", "AI DevOps Engineer – Servers, Kubernetes, CI/CD", "Fixes servers and infrastructure from logs and metrics: Linux, Docker, Kubernetes, Terraform, cloud IAM and cost, CI/CD, monitoring and incident response."),
	("doctor", "AI Symptom Checker – Health Info, Labs, Urgency", "Explains symptoms by urgency, lab results, medications, nutrition, sleep and medical bills with cited evidence. Information, not diagnosis or advice."),
	("family-advisor", "AI Parenting Guide – Eldercare, Benefits, Bereavement", "Family help: parenting from newborn to teen, aging-parent care, benefits paperwork and the weeks after a death. Checklists and next steps, not advice."),
	("financial-planner", "AI Bookkeeping & Budget Assistant – Debt, Tax, Retirement", "Handles budgets and debt payoff, bookkeeping, student loans, personal tax, retirement, insurance claims, statement analysis. Education, not advice."),
	("founder", "AI Pitch Deck Generator – Idea to First Customers", "Takes a product from idea to first customers: idea validation, go-to-market plan, brand, pitch deck, pricing, ads and investor outreach drafts."),
	("handyman", "AI Handyman – Home Repair & Maintenance Guide", "Diagnoses home repairs with DIY-or-pro safety gates, walks through fixes, plans maintenance schedules and helps you hire and brief a contractor."),
	("hr", "AI HR Assistant – Hiring & People Operations", "Runs hiring from scorecard to offer, plus onboarding, reviews, feedback scripts, performance plans and policies, with employment-law flags for counsel."),
	("lawyer", "AI Legal Assistant – Legal Research & Rights Info", "Researches laws and statutes by country, explains rights, reviews contracts, and drafts demand letters for consumer and visa matters. Not legal advice."),
	("marketer", "AI Marketing Agent – Email, Growth, PR, Social", "An AI marketing agent for founders and small teams: lifecycle emails and newsletters, growth plans, paid media setup, partner programs, PR and social."),
	("mechanic", "AI Mechanic – Car Warning Lights, Quotes, Used Cars", "Triages car symptoms and warning lights, calls DIY versus mechanic, checks repair quotes, plans maintenance and builds used-car checklists. Safety first."),
	("merchant", "AI Ecommerce Assistant – Listings, Pricing, Orders", "Handles seller-side ecommerce: product listings, pricing and margins, orders, refunds, marketplace accounts and store policies. Asks before any change."),
	("octomind", "Octomind Assistant – Config, Taps & Workflows", "Answers Octomind CLI, config, runtime and Besydr questions, edits config.toml, builds tap agents, skills, capabilities, deps and workflow pipelines."),
	("operator", "AI Web Scraper & UI Automation – Browser, Desktop, Mobile", "Drives web pages, desktop and mobile apps for browser automation: scrapes data, fills forms, runs flows, posts approved copy, returns files and step logs."),
	("plain", "Plain Assistant – System Prompt Baseline", "Simple AI assistant with a one-line system prompt and no persona. Use it to chat with AI, run prompt testing, and compare models against a baseline."),
	("pm", "AI Product Manager – PRDs, Roadmaps, Status Reports", "Turns ideas into PRDs, stories, RICE roadmaps and OKRs, and runs delivery as an AI project manager: milestone plans, RAID logs and RAG status reports."),
	("producer", "AI UGC Video Generator – Scripts, Ads, Edits, Uploads", "AI video producer: scripts, storyboards, UGC and short-form ads, explainers, footage edits, podcast clips, thumbnails, uploads to TikTok, Reels, YouTube."),
	("sales-rep", "AI Sales Assistant – B2B Outreach, Proposals & CRM", "B2B sales work: cold outreach copy, lead enrichment and scoring, discovery and qualification, closing and negotiation, RFP proposals, CRM and pipeline."),
	("security-analyst", "AI Cybersecurity Analyst – AppSec, SOC, Compliance", "Defensive cybersecurity: secure code review, alert and phishing triage, incident response, GDPR and SOC 2 readiness, scam and identity-theft recovery."),
	("seo", "AI SEO Tool – Site Audits, Keywords, AI Visibility", "Audits sites for technical, on-page and link problems, plans keyword research, content clusters and link building, and diagnoses visibility in AI answers."),
	("shopper", "AI Shopping Assistant – Product Research & Price Comparison", "Compares products and prices across retailers, analyzes reviews, and suggests gifts for a person you describe. Honest trade-offs; no affiliate bias."),
	("support-agent", "AI Customer Service Agent – Ticket Triage & Success", "Triages support queues, drafts replies for approval, files tickets, writes help articles and macros, scores account health, runs churn saves and renewals."),
	("travel-agent", "AI Travel Planner – Day-by-Day Itineraries & Road Trips", "Builds day-by-day itineraries, road trips, budgets, transport logistics and packing lists, verifying prices and hours online. You book; it plans."),
	("tutor", "AI Tutor – Any Subject, Coding, Math, Languages", "Tutors any subject, coding, math, languages and test prep with retrieval practice and Socratic hints, and drafts teacher lesson plans. Never ghostwrites."),
	("vet", "Dog Symptom Checker – AI Pet Health & Training Guide", "Triages dog and cat symptoms with urgency tiers, toxin checks and home care, and coaches force-free training. Information, not a veterinary diagnosis."),
	("writer", "AI Writing Tool – Articles, Blogs, Social Posts, Slides", "Writes and edits articles, blog posts, social posts, replies, fiction notes, translations and slide decks; audits drafts and publishes approved content."),
];

/// Briefs as the lead agent is told to write them (first line names the kind
/// of work and the deliverable), with the agent of that field that should take
/// each; `None` is a task no agent in the roster covers.
const AGENT_TASKS: [(&str, Option<&str>); 19] = [
	("Fix a crash: the CSV importer in src/import.rs panics on a file with a blank header row. Find the root cause, patch it, and run the test suite.", Some("developer")),
	("Review the attached lease for tenant-unfriendly clauses under German law and draft a letter disputing the deposit deduction.", Some("lawyer")),
	("Explain what these lab results mean: ferritin 8 ng/mL, TSH 5.2 mIU/L, and whether the symptoms I listed need a doctor this week.", Some("doctor")),
	("Build a weekly meal plan for two people on a 90 euro budget using what is in my fridge: lentils, spinach, eggs, rice. Include a priced grocery list.", Some("chef")),
	("Write a 900-word blog post for our fintech blog on why invoice financing is rising among freelancers, with a headline and two pull quotes.", Some("writer")),
	("Audit the site shop.example.com for crawl errors and duplicate titles, then give me a keyword plan for the winter boots category.", Some("seo")),
	("Review our signup flow for usability and WCAG contrast problems, and list fixes ranked by how many users they affect.", Some("designer")),
	("Our pods in the production cluster keep restarting with OOMKilled. Read the logs and metrics, find the cause, and propose the Kubernetes memory limits.", Some("devops")),
	("Draft the hiring scorecard and a structured interview plan for a senior accountant, then write the offer letter template.", Some("hr")),
	("Plan a seven-day trip to Portugal in May for two adults: day-by-day itinerary, train and car logistics, a budget, and a packing list.", Some("travel-agent")),
	("Teach my 15-year-old how to solve quadratic equations: give hints instead of answers and finish with five practice problems.", Some("tutor")),
	("My 6-year-old beagle ate a handful of raisins an hour ago and is now vomiting. Tell me how urgent this is and what to do before the clinic opens.", Some("vet")),
	("The check engine light came on and the car shakes at idle. Say whether I can drive it, what to check first, and whether a 640 dollar repair quote is fair.", Some("mechanic")),
	("Build a plan to pay off 14,000 in credit card debt and a 9,000 student loan on a 3,800 monthly income, and show how it changes my retirement savings.", Some("financial-planner")),
	("Review this Express login handler for injection and session flaws, and tell me which findings must be fixed before the SOC 2 audit.", Some("security-analyst")),
	("Write a three-email cold sequence to heads of logistics at mid-size manufacturers about our freight tracking software, and score the lead list I attached.", Some("sales-rep")),
	("Hey, how are you doing today?", None),
	("Thanks, that is all for now.", None),
	("Tell me a joke about penguins.", None),
];

#[tokio::test]
#[serial]
async fn tasks_reach_the_agent_of_their_field_and_uncovered_tasks_reach_none() {
	let roster: Vec<TapAgentHeader> = AGENT_ROSTER
		.iter()
		.map(|(tag, title, description)| TapAgentHeader {
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
	for (task, expected) in AGENT_TASKS {
		let ranked = rank_agents(task, &candidates).await.expect("rank");
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
	assert!(wrong.is_empty(), "handed to the wrong agent: {wrong:#?}");
	assert!(
		missed.len() <= MAX_MISSED_AGENT_TASKS,
		"{} agent tasks found no agent: {missed:#?}",
		missed.len()
	);
}

/// Briefs the cosine stage may refuse before the fixture fails: 4 are refused
/// on the current graph, plus 1 because the int8 graph scores a few hundredths
/// differently per CPU kind.
const MAX_MISSED_AGENT_TASKS: usize = 5;
