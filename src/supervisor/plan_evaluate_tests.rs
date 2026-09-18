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

//! The plan seam: a `request` or `phase_complete` signal pre-screened by the
//! evaluation model after every deterministic skip, with the chat planner as
//! the fallback. Planner calls hit the scripted chat stub; a test that expects
//! none points the stub URL at a closed port, so a call would trip the
//! per-turn failure latch the assertions read. `install_fake_evaluation`
//! holds `ENV_LOCK`, so these tests never take it themselves.

use super::plan::{reconcile_after_actions, PlanSignal, PlanTaskDirective};
use crate::session::chat::session::ChatSession;
use crate::session::chat::test_support::{
	evaluate_counter, fake_provider_config, final_response, install_fake_evaluation, nouls,
	spawn_stub, FakeEvaluationStep,
};
use crate::supervisor::evaluate::{Seam, PLAN_QUESTION_ID};
use octolib::evaluation::Question;

const CREATE: &str = "{\"decision\":\"create\",\"title\":\"Widget\",\"tasks\":[{\"title\":\"Scaffold it\",\"done_when\":\"the crate compiles\"},{\"title\":\"Wire it\",\"done_when\":\"the widget renders\"}]}";
const NO_PLAN: &str = "{\"decision\":\"no_plan\",\"reason\":\"one focused change\"}";
const ADVANCE: &str = "{\"decision\":\"advance\",\"summary\":\"the suite passed\"}";
const HOLD: &str = "{\"decision\":\"hold\",\"reason\":\"waiting on evidence\"}";
/// Nothing listens here: a planner call fails its transport and sets the latch.
const NO_PLANNER: &str = "http://127.0.0.1:1/nothing";
const REQUEST: &str = "rename the CLI flag --json to --format";
const DONE_WHEN: &str = "cargo test exits 0";

fn config(plan: bool) -> crate::config::Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.plan.enabled = true;
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config.supervisor.evaluate.plan = plan;
	config
}

fn msg(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		timestamp: crate::utils::time::now_secs(),
		..Default::default()
	}
}

fn session(signal: PlanSignal) -> ChatSession {
	let mut session = ChatSession::for_tests(vec![
		msg("user", REQUEST),
		msg("assistant", "editing the flag definition"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.pending_plan_signal = Some(signal);
	session.completion_gate_eligible = true;
	session.plan_evaluated = false;
	session.planner_failed = false;
	session
}

fn answer(probability: f64) -> FakeEvaluationStep {
	FakeEvaluationStep::Answers(nouls(&[(PLAN_QUESTION_ID, probability)]))
}

fn planner_calls() -> u64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get("plan_calls")?.as_u64())
		.unwrap_or(0)
}

fn start_plan() {
	crate::mcp::core::plan::sidecar_start(
		"Ship --format",
		&[
			PlanTaskDirective {
				title: "Tests green".to_string(),
				done_when: DONE_WHEN.to_string(),
			},
			PlanTaskDirective {
				title: "Docs updated".to_string(),
				done_when: "README documents --format".to_string(),
			},
		],
	)
	.expect("plan starts");
}

async fn reconcile(session: &mut ChatSession, config: &crate::config::Config, url: &str) {
	let (_tx, rx) = tokio::sync::watch::channel(false);
	std::env::set_var("OLLAMA_API_URL", url);
	let result = reconcile_after_actions(session, config, rx).await;
	std::env::remove_var("OLLAMA_API_URL");
	result.expect("reconciliation succeeds");
}

async fn in_plan_session<Fut>(name: &str, f: Fut)
where
	Fut: std::future::Future<Output = ()>,
{
	let sid = name.to_string();
	crate::session::context::with_session_id(sid.clone(), f).await;
	crate::session::context::cleanup_session(&sid);
}

fn instructions(question: &Question) -> String {
	match question {
		Question::Noul { instructions, .. } => instructions.as_str().unwrap_or("").to_string(),
		_ => panic!("plan questions are Nouls"),
	}
}

#[tokio::test]
async fn a_request_the_evaluation_declines_makes_no_planner_call() {
	let fake = install_fake_evaluation(vec![answer(0.06)]).await;
	in_plan_session("__plan_eval_declined", async {
		let calls = planner_calls();
		let applied = evaluate_counter(Seam::Plan, "applied");
		let replaced = evaluate_counter(Seam::Plan, "avoided");
		let mut session = session(PlanSignal::Request);
		reconcile(&mut session, &config(true), NO_PLANNER).await;
		assert!(session.pending_plan_signal.is_none(), "signal consumed");
		assert!(session.plan_evaluated);
		assert!(!session.planner_failed, "declining is not failing");
		assert!(!crate::mcp::core::plan::has_active_plan());
		assert_eq!(planner_calls() - calls, 0);
		assert_eq!(evaluate_counter(Seam::Plan, "applied") - applied, 1);
		assert_eq!(evaluate_counter(Seam::Plan, "avoided") - replaced, 1);

		let requests = fake.requests();
		assert_eq!(requests.len(), 1);
		let request = &requests[0];
		assert_eq!(request.max_retries, 0);
		assert_eq!(request.timeout, std::time::Duration::from_secs(5));
		assert_eq!(request.state["current_request"], REQUEST);
		for key in [
			"working_request",
			"outcome_conditions",
			"runtime_evidence",
			"phase_trajectory",
		] {
			assert!(request.state.get(key).is_some(), "state carries {key}");
		}
		assert!(instructions(&request.questions[PLAN_QUESTION_ID]).contains("external plan"));
	})
	.await;
}

#[tokio::test]
async fn an_answer_only_turn_is_declined_before_any_evaluation() {
	let fake = install_fake_evaluation(vec![answer(0.06)]).await;
	in_plan_session("__plan_eval_answer_only", async {
		let mut session = session(PlanSignal::Request);
		let mut task = crate::supervisor::resolve::ResolvedTask::self_contained(REQUEST);
		task.answer_only = true;
		session.gate_task = Some(task);
		reconcile(&mut session, &config(true), NO_PLANNER).await;
		assert!(fake.requests().is_empty());
		assert!(session.pending_plan_signal.is_none());
		assert!(!session.planner_failed);
	})
	.await;
}

#[tokio::test]
async fn a_request_at_or_above_the_threshold_runs_the_planner_unchanged() {
	let _fake = install_fake_evaluation(vec![answer(0.71)]).await;
	in_plan_session("__plan_eval_request_passes", async {
		let calls = planner_calls();
		let applied = evaluate_counter(Seam::Plan, "applied");
		let url = spawn_stub(vec![final_response(CREATE)]).await;
		let mut session = session(PlanSignal::Request);
		reconcile(&mut session, &config(true), &url).await;
		assert!(crate::mcp::core::plan::has_active_plan());
		assert!(!session.planner_failed);
		assert_eq!(planner_calls() - calls, 1);
		assert_eq!(evaluate_counter(Seam::Plan, "applied") - applied, 0);
	})
	.await;
}

#[tokio::test]
async fn a_phase_complete_below_the_threshold_is_held_with_the_documented_reason() {
	let fake = install_fake_evaluation(vec![answer(0.08)]).await;
	in_plan_session("__plan_eval_held", async {
		start_plan();
		let calls = planner_calls();
		let applied = evaluate_counter(Seam::Plan, "applied");
		let replaced = evaluate_counter(Seam::Plan, "avoided");
		let mut session = session(PlanSignal::PhaseComplete);
		session.session.messages.push(msg("assistant", "edited src/cli.rs"));
		reconcile(&mut session, &config(true), NO_PLANNER).await;
		assert_eq!(crate::mcp::core::plan::active_step_index(), Some(0));
		assert!(!session.planner_failed);
		assert!(session.pending_plan_signal.is_none());
		assert_eq!(planner_calls() - calls, 0);
		assert_eq!(evaluate_counter(Seam::Plan, "applied") - applied, 1);
		assert_eq!(evaluate_counter(Seam::Plan, "avoided") - replaced, 1);
		let expected = format!(
			"<runtime-plan-feedback>Current phase remains open: runtime evidence does not yet show: {DONE_WHEN}</runtime-plan-feedback>"
		);
		assert!(
			session
				.session
				.messages
				.iter()
				.any(|m| m.content.contains(&expected)),
			"the hold feedback is injected verbatim inside the system-managed message"
		);

		let requests = fake.requests();
		assert_eq!(requests.len(), 1);
		let request = &requests[0];
		assert_eq!(request.state["phase_title"], "Tests green");
		assert_eq!(request.state["done_when"], DONE_WHEN);
		assert!(request.state.get("runtime_evidence").is_some());
		assert!(request.state.get("phase_trajectory").is_some());
		assert!(instructions(&request.questions[PLAN_QUESTION_ID]).ends_with(DONE_WHEN));
	})
	.await;
}

#[tokio::test]
async fn a_phase_complete_at_the_threshold_runs_the_planner_unchanged() {
	let _fake = install_fake_evaluation(vec![answer(0.5)]).await;
	in_plan_session("__plan_eval_phase_passes", async {
		start_plan();
		let applied = evaluate_counter(Seam::Plan, "applied");
		let url = spawn_stub(vec![final_response(ADVANCE)]).await;
		let mut session = session(PlanSignal::PhaseComplete);
		reconcile(&mut session, &config(true), &url).await;
		assert_eq!(crate::mcp::core::plan::active_step_index(), Some(1));
		assert!(!session.planner_failed);
		assert_eq!(evaluate_counter(Seam::Plan, "applied") - applied, 0);
	})
	.await;
}

#[tokio::test]
async fn reassess_is_never_scored() {
	let fake = install_fake_evaluation(vec![answer(0.01)]).await;
	in_plan_session("__plan_eval_reassess", async {
		start_plan();
		let url = spawn_stub(vec![final_response(HOLD)]).await;
		let mut session = session(PlanSignal::Reassess);
		reconcile(&mut session, &config(true), &url).await;
		assert!(fake.requests().is_empty());
		assert!(!session.planner_failed);
		assert_eq!(crate::mcp::core::plan::active_step_index(), Some(0));
	})
	.await;
}

#[tokio::test]
async fn an_unavailable_evaluation_runs_the_planner_and_leaves_the_latch_alone() {
	let fake =
		install_fake_evaluation(vec![FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY")]).await;
	in_plan_session("__plan_eval_unavailable", async {
		let calls = planner_calls();
		let unavailable = evaluate_counter(Seam::Plan, "unavailable");
		let url = spawn_stub(vec![final_response(NO_PLAN)]).await;
		let mut session = session(PlanSignal::Request);
		reconcile(&mut session, &config(true), &url).await;
		assert!(
			!session.planner_failed,
			"an evaluation failure never sets the latch"
		);
		assert!(!crate::mcp::core::plan::has_active_plan());
		assert_eq!(planner_calls() - calls, 1);
		assert_eq!(evaluate_counter(Seam::Plan, "unavailable") - unavailable, 1);
		assert_eq!(fake.requests().len(), 1);
	})
	.await;
}

#[tokio::test]
async fn the_seam_off_makes_no_evaluation_request_for_either_signal() {
	let fake = install_fake_evaluation(vec![answer(0.01), answer(0.01)]).await;
	in_plan_session("__plan_eval_off", async {
		let calls = planner_calls();
		let url = spawn_stub(vec![final_response(NO_PLAN), final_response(HOLD)]).await;
		let mut session = session(PlanSignal::Request);
		reconcile(&mut session, &config(false), &url).await;
		assert!(!crate::mcp::core::plan::has_active_plan());
		start_plan();
		session.pending_plan_signal = Some(PlanSignal::PhaseComplete);
		reconcile(&mut session, &config(false), &url).await;
		assert_eq!(crate::mcp::core::plan::active_step_index(), Some(0));
		assert!(fake.requests().is_empty());
		assert!(!session.planner_failed);
		assert_eq!(planner_calls() - calls, 2);
	})
	.await;
}
