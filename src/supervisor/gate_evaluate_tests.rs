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

//! The gate seam: charged findings refuted by the evaluation model in place of
//! the second-verifier chat pass, with that pass as the fallback. The verifier
//! itself always hits the scripted chat stub, so a refutation call the seam
//! should have replaced reads "SCRIPT EXHAUSTED" and refutes nothing.
//! `install_fake_evaluation` holds `ENV_LOCK`, so these tests never take it
//! themselves.

use super::*;
use crate::session::chat::test_support::{
	evaluate_counter, fake_provider_config, final_response, install_fake_evaluation, nouls,
	spawn_stub, FakeEvaluationStep,
};
use crate::supervisor::evaluate::{gate_question_id, Seam};
use octolib::evaluation::Question;

const CLEAN_SHAPES: &str = r#"<shape name="circular" found="no">independent expectation</shape>
<shape name="context-stripped" found="no">representative context</shape>
<shape name="acceptance-only" found="no">not applicable</shape>
<shape name="unenumerated-category" found="no">bounded scope</shape>"#;
const TEST_GAP: &str = r#"<gap settles="run the suite">no test was run after the change</gap>"#;
const README_GAP: &str = r#"<gap settles="update the README">README still documents --json</gap>"#;
const TEST_FINDING: &str = "no test was run after the change — clear it by: run the suite";
const README_FINDING: &str = "README still documents --json — clear it by: update the README";
const ACTIONS: &str = "#1 [read] shell cargo test → ok";

fn config(gate: bool) -> Config {
	let mut config = fake_provider_config();
	config.supervisor.enabled = true;
	config.supervisor.model.model = Some("ollama:fake-model".to_string());
	config.supervisor.evaluate.gate = gate;
	config
}

fn gaps(gaps: &[&str]) -> String {
	format!("{CLEAN_SHAPES}{}", gaps.concat())
}

fn input<'a>(actions: &'a str, grounds: &'a [(u64, String)]) -> GateInput<'a> {
	GateInput {
		original_task: "rename the CLI flag --json to --format",
		task: "rename the CLI flag --json to --format",
		task_scope: crate::supervisor::resolve::ResolutionScope::SelfContained,
		context_sources: &[],
		resolution_evidence: &[],
		result: "the flag is renamed and the suite passes",
		claim: Some("done: tests pass"),
		actions,
		grounds,
		plan: "",
		ground_truth: "",
		prior_gaps: &[],
		role_context: "",
		evidence_conditions: &[],
	}
}

/// Noul answers `f0..fn` with the given probabilities.
fn answers(probabilities: &[f64]) -> FakeEvaluationStep {
	let pairs: Vec<(String, f64)> = probabilities
		.iter()
		.enumerate()
		.map(|(slot, p)| (gate_question_id(slot), *p))
		.collect();
	let borrowed: Vec<(&str, f64)> = pairs.iter().map(|(id, p)| (id.as_str(), *p)).collect();
	FakeEvaluationStep::Answers(nouls(&borrowed))
}

/// Chat calls under `CallKind::Gate`: every verifier round plus, when the
/// seam did not answer, one refutation call.
fn gate_chat_calls() -> u64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get("gate_calls")?.as_u64())
		.unwrap_or(0)
}

async fn run(
	config: &Config,
	actions: &str,
	grounds: &[(u64, String)],
	stub: Vec<serde_json::Value>,
) -> GateVerdict {
	let url = spawn_stub(stub).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let verdict = verify(config, input(actions, grounds), rx).await;
	std::env::remove_var("OLLAMA_API_URL");
	verdict
}

#[tokio::test]
async fn findings_at_the_threshold_are_refuted_and_the_rest_stand() {
	let fake = install_fake_evaluation(vec![answers(&[0.91, 0.22])]).await;
	let calls = gate_chat_calls();
	let applied = evaluate_counter(Seam::Gate, "applied");
	let replaced = evaluate_counter(Seam::Gate, "avoided");
	let verdict = run(
		&config(true),
		ACTIONS,
		&[],
		vec![final_response(&gaps(&[TEST_GAP, README_GAP]))],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![README_FINDING.to_string()]));
	assert_eq!(gate_chat_calls() - calls, 1, "the verifier call only");
	assert_eq!(evaluate_counter(Seam::Gate, "applied") - applied, 1);
	assert_eq!(evaluate_counter(Seam::Gate, "avoided") - replaced, 1);

	let requests = fake.requests();
	assert_eq!(requests.len(), 1);
	let request = &requests[0];
	assert_eq!(request.max_retries, 0);
	assert_eq!(request.timeout, std::time::Duration::from_secs(5));
	let evidence = request.state["evidence"].as_str().expect("evidence");
	assert!(evidence.contains("<recorded_actions>"));
	assert!(evidence.contains("cargo test"));
	let findings = request.state["findings"].as_array().expect("findings");
	assert_eq!(findings.len(), 2);
	assert_eq!(findings[0]["number"], 1);
	assert_eq!(findings[0]["text"], TEST_FINDING);
	assert_eq!(findings[1]["text"], README_FINDING);
	for (slot, prefix) in [(0, "Finding 1: "), (1, "Finding 2: ")] {
		match &request.questions[&gate_question_id(slot)] {
			Question::Noul { instructions, .. } => {
				assert!(instructions.as_str().unwrap().starts_with(prefix));
			}
			_ => panic!("gate questions are Nouls"),
		}
	}
}

#[tokio::test]
async fn every_finding_refuted_is_a_pass() {
	let _fake = install_fake_evaluation(vec![answers(&[0.88])]).await;
	let calls = gate_chat_calls();
	let verdict = run(
		&config(true),
		ACTIONS,
		&[],
		vec![final_response(&gaps(&[TEST_GAP]))],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Pass);
	assert_eq!(gate_chat_calls() - calls, 1);
}

#[tokio::test]
async fn a_finding_below_the_threshold_stands() {
	let _fake = install_fake_evaluation(vec![answers(&[0.79])]).await;
	let calls = gate_chat_calls();
	let verdict = run(
		&config(true),
		ACTIONS,
		&[],
		vec![final_response(&gaps(&[TEST_GAP]))],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![TEST_FINDING.to_string()]));
	assert_eq!(gate_chat_calls() - calls, 1);
}

#[tokio::test]
async fn an_unavailable_evaluation_runs_the_chat_refutation_once() {
	let fake =
		install_fake_evaluation(vec![FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY")]).await;
	let calls = gate_chat_calls();
	let unavailable = evaluate_counter(Seam::Gate, "unavailable");
	let verdict = run(
		&config(true),
		ACTIONS,
		&[],
		vec![
			final_response(&gaps(&[TEST_GAP])),
			final_response(r#"<finding n="1" verdict="refuted">#1 ran the suite</finding>"#),
		],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Pass);
	assert_eq!(
		gate_chat_calls() - calls,
		2,
		"verifier plus chat refutation"
	);
	assert_eq!(evaluate_counter(Seam::Gate, "unavailable") - unavailable, 1);
	assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn evidence_over_the_state_cap_runs_the_chat_refutation() {
	let fake = install_fake_evaluation(vec![answers(&[0.99])]).await;
	let calls = gate_chat_calls();
	let unavailable = evaluate_counter(Seam::Gate, "unavailable");
	let huge = "#1 [read] shell cargo test → ok, every crate compiled and linked\n".repeat(3_000);
	assert!(crate::session::estimate_tokens(&huge) > crate::supervisor::evaluate::MAX_STATE_TOKENS);
	let verdict = run(
		&config(true),
		&huge,
		&[],
		vec![
			final_response(&gaps(&[TEST_GAP])),
			final_response(r#"<finding n="1" verdict="stands">nothing shows a run</finding>"#),
		],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![TEST_FINDING.to_string()]));
	assert_eq!(gate_chat_calls() - calls, 2);
	assert_eq!(evaluate_counter(Seam::Gate, "unavailable") - unavailable, 1);
	assert!(
		fake.requests().is_empty(),
		"an oversized state is never sent"
	);
}

#[tokio::test]
async fn the_seam_off_makes_no_evaluation_request() {
	let fake = install_fake_evaluation(vec![answers(&[0.99])]).await;
	let calls = gate_chat_calls();
	let verdict = run(
		&config(false),
		ACTIONS,
		&[],
		vec![
			final_response(&gaps(&[TEST_GAP])),
			final_response(r#"<finding n="1" verdict="stands">nothing shows a run</finding>"#),
		],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![TEST_FINDING.to_string()]));
	assert!(fake.requests().is_empty());
	assert_eq!(gate_chat_calls() - calls, 2);
}

#[tokio::test]
async fn the_readback_round_runs_unchanged_and_the_seam_sees_its_evidence() {
	let fake = install_fake_evaluation(vec![answers(&[0.95])]).await;
	let calls = gate_chat_calls();
	let grounds = vec![(1, "test result: ok. 42 passed; 0 failed".to_string())];
	let verdict = run(
		&config(true),
		ACTIONS,
		&grounds,
		vec![
			final_response(r#"<readback seq="1">what the suite printed</readback>"#),
			final_response(&gaps(&[TEST_GAP])),
		],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Pass);
	assert_eq!(gate_chat_calls() - calls, 2, "verifier and readback rounds");
	let requests = fake.requests();
	assert_eq!(requests.len(), 1);
	let evidence = requests[0].state["evidence"].as_str().expect("evidence");
	assert!(evidence.contains("<readback_evidence>"));
	assert!(evidence.contains("42 passed"));
}

#[tokio::test]
async fn the_format_retry_runs_unchanged_before_the_seam() {
	let fake = install_fake_evaluation(vec![answers(&[0.95])]).await;
	let calls = gate_chat_calls();
	let verdict = run(
		&config(true),
		ACTIONS,
		&[],
		vec![
			final_response("certainly, here is my verdict in prose"),
			final_response(&gaps(&[TEST_GAP])),
		],
	)
	.await;
	assert_eq!(verdict, GateVerdict::Pass);
	assert_eq!(gate_chat_calls() - calls, 2, "verifier and format retry");
	assert_eq!(fake.requests().len(), 1);
}
