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

//! Plan-reconcile e2e against the scripted fake provider: a pending plan
//! signal drives the real specialist-context rendering and planner call.
//! A valid `create` decision must produce an active plan; a garbage
//! response must trip the per-turn failure latch and leave no plan behind.

use super::plan::{reconcile_after_actions, PlanSignal};
use crate::session::chat::session::ChatSession;
use crate::session::chat::test_support::{
	fake_provider_config, final_response, spawn_stub, ENV_LOCK,
};

fn plan_config() -> crate::config::Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.plan.enabled = true;
	config.supervisor.plan.model = "ollama:fake-model".to_string();
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

fn plan_session() -> ChatSession {
	let mut session = ChatSession::for_tests(vec![
		msg("user", "build the widget end to end"),
		msg("assistant", "starting with the scaffolding"),
	]);
	session.model = "ollama:fake-model".to_string();
	session.pending_plan_signal = Some(PlanSignal::Request);
	session.completion_gate_eligible = true;
	session.plan_evaluated = false;
	session.planner_failed = false;
	session
}

/// Keep the sender alive for the duration of the call: dropping it makes
/// the cancellation wrapper read the operation as cancelled.
fn cancel_pair() -> (
	tokio::sync::watch::Sender<bool>,
	tokio::sync::watch::Receiver<bool>,
) {
	tokio::sync::watch::channel(false)
}

#[tokio::test]
async fn test_plan_request_creates_active_plan() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "__plan_e2e_create".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let url = spawn_stub(vec![final_response(
			"{\"decision\":\"create\",\"title\":\"Ship the widget\",\"tasks\":[{\"title\":\"build it\",\"done_when\":\"it compiles\"},{\"title\":\"test it\",\"done_when\":\"tests pass\"}]}",
		)])
		.await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = plan_config();
		let mut session = plan_session();
		let (_tx, rx) = cancel_pair();
		reconcile_after_actions(&mut session, &config, rx)
			.await
			.expect("reconcile");

		let msgs: Vec<&str> = session
			.session
			.messages
			.iter()
			.map(|m| m.content.as_str())
			.collect();
		assert!(
			crate::mcp::core::plan::has_active_plan(),
			"create decision must produce an active plan; planner_failed={}, evaluated={}, msgs={msgs:?}",
			session.planner_failed,
			session.plan_evaluated
		);
		assert!(!session.planner_failed);
		assert!(session.pending_plan_signal.is_none(), "signal consumed");

		std::env::remove_var("OLLAMA_API_URL");
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn test_plan_request_garbage_response_trips_failure_latch() {
	let _guard = ENV_LOCK.lock().await;
	let sid = "__plan_e2e_garbage".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		let url = spawn_stub(vec![final_response("certainly! here is no json at all")]).await;
		std::env::set_var("OLLAMA_API_URL", &url);

		let config = plan_config();
		let mut session = plan_session();
		let (_tx, rx) = cancel_pair();
		reconcile_after_actions(&mut session, &config, rx)
			.await
			.expect("reconcile survives garbage");

		assert!(
			session.planner_failed,
			"unusable planner output must trip the per-turn latch"
		);
		assert!(
			!crate::mcp::core::plan::has_active_plan(),
			"no plan may be created from garbage"
		);

		std::env::remove_var("OLLAMA_API_URL");
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn test_plan_signal_noop_without_supervisor() {
	let mut config = plan_config();
	config.supervisor.enabled = false;
	let mut session = plan_session();
	let (_tx, rx) = cancel_pair();
	reconcile_after_actions(&mut session, &config, rx)
		.await
		.expect("reconcile");
	assert!(session.pending_plan_signal.is_none(), "signal consumed");
	assert!(!session.planner_failed);
}

// ---------------------------------------------------------------------------
// Signal/decision matrix: every arm of the reconcile state machine against
// the scripted planner, each in its own session so plan sidecars stay isolated.
// ---------------------------------------------------------------------------

const CREATE: &str = "{\"decision\":\"create\",\"title\":\"Ship the widget\",\"tasks\":[{\"title\":\"build it\",\"done_when\":\"it compiles\"},{\"title\":\"test it\",\"done_when\":\"tests pass\"}]}";
const NO_PLAN: &str =
	"{\"decision\":\"no_plan\",\"reason\":\"single deliverable, no phases needed\"}";
const ADVANCE: &str = "{\"decision\":\"advance\",\"summary\":\"scaffolding compiles\"}";
const REVISE: &str = "{\"decision\":\"revise\",\"reason\":\"assumptions changed\",\"tasks\":[{\"title\":\"rebuild it\",\"done_when\":\"it links\"}]}";
const HOLD: &str = "{\"decision\":\"hold\",\"reason\":\"waiting on evidence\"}";

async fn reconcile_with(session: &mut ChatSession, url: &str) -> anyhow::Result<()> {
	let config = plan_config();
	let (_tx, rx) = cancel_pair();
	std::env::set_var("OLLAMA_API_URL", url);
	let result = reconcile_after_actions(session, &config, rx).await;
	std::env::remove_var("OLLAMA_API_URL");
	result
}

async fn in_plan_session<Fut>(name: &str, f: Fut)
where
	Fut: std::future::Future<Output = ()>,
{
	let sid = name.to_string();
	crate::session::context::with_session_id(sid.clone(), f).await;
	crate::session::context::cleanup_session(&sid);
}

#[tokio::test]
async fn a_no_plan_decision_leaves_no_plan_and_no_failure() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_noplan", async {
		let url = spawn_stub(vec![final_response(NO_PLAN)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url)
			.await
			.expect("no_plan reconciles");
		assert!(!crate::mcp::core::plan::has_active_plan());
		assert!(!session.planner_failed, "declining is not failing");
		assert!(session.pending_plan_signal.is_none());
	})
	.await;
}

#[tokio::test]
async fn the_failure_latch_consumes_a_second_signal_without_a_call() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_latch", async {
		let mut session = plan_session();
		session.planner_failed = true;
		let before = session.session.messages.len();
		// No stub: any planner call would fail the transport and be visible.
		reconcile_with(&mut session, "http://127.0.0.1:1/nothing")
			.await
			.expect("latched signal is consumed silently");
		assert!(session.planner_failed, "the latch stays set for the turn");
		assert!(session.pending_plan_signal.is_none());
		assert_eq!(
			session.session.messages.len(),
			before,
			"no feedback message is injected for a latched signal"
		);
	})
	.await;
}

#[tokio::test]
async fn a_request_signal_with_an_active_plan_is_consumed() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_active", async {
		let url = spawn_stub(vec![final_response(CREATE)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url).await.expect("create");
		assert!(crate::mcp::core::plan::has_active_plan());

		session.pending_plan_signal = Some(PlanSignal::Request);
		session.plan_evaluated = false;
		let before = session.session.messages.len();
		reconcile_with(&mut session, "http://127.0.0.1:1/nothing")
			.await
			.expect("request with active plan is consumed");
		assert_eq!(
			session.session.messages.len(),
			before,
			"no second planner round runs for an already-active plan"
		);
	})
	.await;
}

#[tokio::test]
async fn an_already_evaluated_turn_does_not_replan() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_evaluated", async {
		let mut session = plan_session();
		session.plan_evaluated = true;
		reconcile_with(&mut session, "http://127.0.0.1:1/nothing")
			.await
			.expect("evaluated turn is consumed");
		assert!(!crate::mcp::core::plan::has_active_plan());
		assert!(session.pending_plan_signal.is_none());
	})
	.await;
}

#[tokio::test]
async fn a_system_managed_turn_cannot_create_a_plan() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_system", async {
		let mut session = plan_session();
		session.completion_gate_eligible = false;
		reconcile_with(&mut session, "http://127.0.0.1:1/nothing")
			.await
			.expect("system-managed turn is consumed");
		assert!(!crate::mcp::core::plan::has_active_plan());
		assert!(session.pending_plan_signal.is_none());
	})
	.await;
}

#[tokio::test]
async fn an_answer_only_turn_declines_planning() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_answeronly", async {
		let mut session = plan_session();
		let mut task =
			crate::supervisor::resolve::ResolvedTask::self_contained("explain the design");
		task.answer_only = true;
		session.gate_task = Some(task);
		reconcile_with(&mut session, "http://127.0.0.1:1/nothing")
			.await
			.expect("answer-only turn is consumed");
		assert!(!crate::mcp::core::plan::has_active_plan());
		assert!(session.pending_plan_signal.is_none());
		assert!(!session.planner_failed, "declining is not failing");
	})
	.await;
}

#[tokio::test]
async fn a_phase_signal_without_an_active_plan_is_consumed() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_orphan_phase", async {
		let mut session = plan_session();
		session.pending_plan_signal = Some(PlanSignal::PhaseComplete);
		reconcile_with(&mut session, "http://127.0.0.1:1/nothing")
			.await
			.expect("orphan phase signal is consumed");
		assert!(session.pending_plan_signal.is_none());
		assert!(!session.planner_failed);
	})
	.await;
}

#[tokio::test]
async fn a_phase_complete_advance_moves_the_plan_and_recharges_the_gate() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_advance", async {
		let url = spawn_stub(vec![final_response(CREATE), final_response(ADVANCE)]).await;
		let mut session = plan_session();
		session.nudge_iterations = 5;
		reconcile_with(&mut session, &url).await.expect("create");
		session.pending_plan_signal = Some(PlanSignal::PhaseComplete);
		reconcile_with(&mut session, &url).await.expect("advance");
		assert!(crate::mcp::core::plan::has_active_plan());
		assert!(!session.planner_failed);
		assert_eq!(
			session.nudge_iterations, 0,
			"a planner-verified advance recharges the pre-gate budget"
		);
	})
	.await;
}

#[tokio::test]
async fn a_phase_complete_hold_keeps_the_phase_open_with_feedback() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_hold", async {
		let url = spawn_stub(vec![final_response(CREATE), final_response(HOLD)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url).await.expect("create");
		session.pending_plan_signal = Some(PlanSignal::PhaseComplete);
		reconcile_with(&mut session, &url).await.expect("hold");
		assert!(crate::mcp::core::plan::has_active_plan());
		let msgs: Vec<&str> = session
			.session
			.messages
			.iter()
			.map(|m| m.content.as_str())
			.collect();
		assert!(
			msgs.iter()
				.any(|m| m.contains("runtime-plan-feedback") && m.contains("remains open")),
			"a hold explains itself to the agent: {msgs:?}"
		);
	})
	.await;
}

#[tokio::test]
async fn a_phase_complete_revise_rewrites_the_plan_and_resets_evidence() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_revise", async {
		let url = spawn_stub(vec![final_response(CREATE), final_response(REVISE)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url).await.expect("create");
		session.pending_plan_signal = Some(PlanSignal::PhaseComplete);
		reconcile_with(&mut session, &url).await.expect("revise");
		assert!(crate::mcp::core::plan::has_active_plan());
		assert!(!session.planner_failed);
	})
	.await;
}

#[tokio::test]
async fn a_reassess_revise_rewrites_the_plan() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_reassess_revise", async {
		let url = spawn_stub(vec![final_response(CREATE), final_response(REVISE)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url).await.expect("create");
		session.pending_plan_signal = Some(PlanSignal::Reassess);
		reconcile_with(&mut session, &url)
			.await
			.expect("reassess revise");
		assert!(crate::mcp::core::plan::has_active_plan());
		assert!(!session.planner_failed);
	})
	.await;
}

#[tokio::test]
async fn a_reassess_hold_reports_the_failed_assumption() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_reassess_hold", async {
		let url = spawn_stub(vec![final_response(CREATE), final_response(HOLD)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url).await.expect("create");
		session.pending_plan_signal = Some(PlanSignal::Reassess);
		reconcile_with(&mut session, &url)
			.await
			.expect("reassess hold");
		let msgs: Vec<&str> = session
			.session
			.messages
			.iter()
			.map(|m| m.content.as_str())
			.collect();
		assert!(
			msgs.iter().any(|m| m.contains("Plan assumption failed")),
			"the reassess hold names the failed assumption: {msgs:?}"
		);
	})
	.await;
}

#[tokio::test]
async fn a_decision_incompatible_with_the_signal_is_a_planner_failure() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_incompatible", async {
		let url = spawn_stub(vec![final_response(ADVANCE)]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url)
			.await
			.expect("incompatible decision is contained");
		assert!(
			session.planner_failed,
			"an advance answer to a request signal trips the latch"
		);
		assert!(!crate::mcp::core::plan::has_active_plan());
	})
	.await;
}

#[tokio::test]
async fn a_transport_failure_trips_the_latch() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_transport", async {
		let url = crate::session::chat::test_support::spawn_stub_with_status(vec![(
			500,
			serde_json::json!({"error": "planner down"}),
		)])
		.await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url)
			.await
			.expect("transport failure is contained");
		assert!(session.planner_failed);
		assert!(!crate::mcp::core::plan::has_active_plan());
	})
	.await;
}

#[tokio::test]
async fn a_decision_missing_its_fields_is_unusable() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_unusable", async {
		let url = spawn_stub(vec![final_response("{\"decision\":\"create\"}")]).await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url)
			.await
			.expect("unusable decision is contained");
		assert!(session.planner_failed);
		assert!(!crate::mcp::core::plan::has_active_plan());
	})
	.await;
}

#[tokio::test]
async fn a_planner_failure_with_an_active_plan_leaves_the_phase_open() {
	let _guard = ENV_LOCK.lock().await;
	in_plan_session("__plan_e2e_fail_active", async {
		let url = spawn_stub(vec![
			final_response(CREATE),
			final_response("definitely not json"),
		])
		.await;
		let mut session = plan_session();
		reconcile_with(&mut session, &url).await.expect("create");
		session.pending_plan_signal = Some(PlanSignal::PhaseComplete);
		reconcile_with(&mut session, &url)
			.await
			.expect("failure with active plan is contained");
		assert!(session.planner_failed);
		assert!(
			crate::mcp::core::plan::has_active_plan(),
			"a failed decision must not destroy plan state"
		);
		let msgs: Vec<&str> = session
			.session
			.messages
			.iter()
			.map(|m| m.content.as_str())
			.collect();
		assert!(
			msgs.iter().any(|m| m.contains("could not decide")),
			"the agent is told the plan manager did not decide: {msgs:?}"
		);
	})
	.await;
}
