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
