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
//! the second-verifier chat pass, failing closed when unavailable. The verifier
//! itself always hits the scripted chat stub, so a refutation call the seam
//! should have replaced reads "SCRIPT EXHAUSTED" and refutes nothing.
//! `install_fake_evaluation` holds `ENV_LOCK`, so these tests never take it
//! themselves.

use super::*;
use crate::session::chat::test_support::{
	evaluate_counter, fake_provider_config, final_response, install_fake_evaluation, nouls, spawn_recording_stub,
	FakeEvaluationStep,
};
use crate::supervisor::evaluate::{gate_question_id, Seam};
use octolib::evaluation::Question;

static EMPTY_LEDGER: std::sync::LazyLock<EvidenceLedger> = std::sync::LazyLock::new(EvidenceLedger::default);
static EMPTY_EVIDENCE: std::sync::LazyLock<RenderedEvidence> = std::sync::LazyLock::new(RenderedEvidence::default);

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
	config.decision.supervisor = gate;
	config
}

fn gaps(gaps: &[&str]) -> String {
	format!("{CLEAN_SHAPES}{}", gaps.concat())
}

fn input<'a>(actions: &'a str, grounds: &'a [(u64, String)]) -> GateInput<'a> {
	GateInput {
		ledger: &EMPTY_LEDGER,
		original_task: "rename the CLI flag --json to --format",
		task: "rename the CLI flag --json to --format",
		task_scope: crate::supervisor::resolve::ResolutionScope::SelfContained,
		context_sources: &[],
		resolution_evidence: &[],
		result: "the flag is renamed and the suite passes",
		claim: Some("done: tests pass"),
		stop: Some(crate::supervisor::detect::SelfReport::Done),
		actions,
		historical_actions: "",
		grounds,
		plan: "",
		ground_truth: &EMPTY_EVIDENCE,
		prior_gaps: &[],
		role_context: "",
		evidence_conditions: &[],
		first_verification: true,
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

async fn run(config: &Config, actions: &str, grounds: &[(u64, String)], stub: Vec<serde_json::Value>) -> GateVerdict {
	let expected_calls = stub.len();
	let (url, captured) = spawn_recording_stub(stub).await;
	std::env::set_var("OLLAMA_API_URL", &url);
	let (_tx, rx) = tokio::sync::watch::channel(false);
	let verdict = verify(config, input(actions, grounds), rx).await;
	std::env::remove_var("OLLAMA_API_URL");
	assert_eq!(captured.lock().unwrap().len(), expected_calls);
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
	let verdict = run(&config(true), ACTIONS, &[], vec![final_response(&gaps(&[TEST_GAP]))]).await;
	assert_eq!(verdict, GateVerdict::Pass);
	assert_eq!(gate_chat_calls() - calls, 1);
}

#[tokio::test]
async fn a_finding_below_the_threshold_stands() {
	let _fake = install_fake_evaluation(vec![answers(&[0.79])]).await;
	let calls = gate_chat_calls();
	let verdict = run(&config(true), ACTIONS, &[], vec![final_response(&gaps(&[TEST_GAP]))]).await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![TEST_FINDING.to_string()]));
	assert_eq!(gate_chat_calls() - calls, 1);
}

#[tokio::test]
async fn an_unavailable_evaluation_leaves_all_findings_standing_without_chat_refutation() {
	let fake = install_fake_evaluation(vec![FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY")]).await;
	let calls = gate_chat_calls();
	let unavailable = evaluate_counter(Seam::Gate, "unavailable");
	let verdict = run(&config(true), ACTIONS, &[], vec![final_response(&gaps(&[TEST_GAP]))]).await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![TEST_FINDING.to_string()]));
	assert_eq!(gate_chat_calls() - calls, 1, "verifier only");
	assert_eq!(evaluate_counter(Seam::Gate, "unavailable") - unavailable, 1);
	assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn evidence_over_the_state_cap_is_read_in_slices_and_any_slice_refutes() {
	let mut steps = vec![answers(&[0.1])];
	steps.extend((0..9).map(|_| answers(&[0.95])));
	let fake = install_fake_evaluation(steps).await;
	let calls = gate_chat_calls();
	let huge = "#1 [read] shell cargo test → ok, every crate compiled and linked\n".repeat(3_000);
	assert!(crate::session::estimate_tokens(&huge) > crate::supervisor::evaluate::MAX_STATE_TOKENS);
	let verdict = run(&config(true), &huge, &[], vec![final_response(&gaps(&[TEST_GAP]))]).await;
	assert_eq!(verdict, GateVerdict::Pass, "one slice citing the refutation is enough");
	assert_eq!(gate_chat_calls() - calls, 1, "the verifier call only");

	let requests = fake.requests();
	assert!(requests.len() >= 2, "the evidence is read in slices");
	let mut evidence = String::new();
	for request in &requests {
		assert!(
			crate::session::estimate_tokens(&request.state.to_string())
				<= crate::supervisor::evaluate::MAX_STATE_TOKENS
		);
		assert_eq!(
			request.state["findings"].as_array().map(Vec::len),
			Some(1),
			"every slice carries every finding"
		);
		assert_eq!(request.questions.len(), 1);
		evidence.push_str(request.state["evidence"].as_str().expect("evidence"));
	}
	assert!(evidence.contains(&huge), "the slices cover the evidence in order");
}

#[tokio::test]
async fn a_line_over_the_state_cap_leaves_all_findings_standing() {
	let fake = install_fake_evaluation((0..4).map(|_| answers(&[0.99])).collect()).await;
	let unavailable = evaluate_counter(Seam::Gate, "unavailable");
	let huge = "#1 [read] shell cargo test → ok, every crate compiled and linked ".repeat(3_000);
	assert!(crate::session::estimate_tokens(&huge) > crate::supervisor::evaluate::MAX_STATE_TOKENS);
	let verdict = run(&config(true), &huge, &[], vec![final_response(&gaps(&[TEST_GAP]))]).await;
	assert_eq!(verdict, GateVerdict::Gaps(vec![TEST_FINDING.to_string()]));
	assert_eq!(evaluate_counter(Seam::Gate, "unavailable") - unavailable, 1);
	assert!(
		fake.requests()
			.iter()
			.all(|request| !request.state["evidence"].as_str().unwrap_or_default().contains(&huge)),
		"an oversized slice is never sent"
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

// ---------------------------------------------------------------------------
// Diff hunk selection: an oversized diff is chosen by relevance, not cut by size.
// ---------------------------------------------------------------------------

const HUNK_BODY_LINES: usize = 220;

/// A one-file diff of `hunks` hunks, each a header, a marker line and padding, so
/// the whole is far over the diff budget.
fn padded_file_diff(path: &str, hunks: usize) -> String {
	let mut diff = format!("diff --git a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n");
	for hunk in 0..hunks {
		diff.push_str(&format!(
			"@@ -{0},3 +{0},4 @@ fn region_{hunk}\n+// marker-{path}-{hunk}\n",
			hunk * 100 + 1
		));
		for line in 0..HUNK_BODY_LINES {
			diff.push_str(&format!("+let padding_{hunk}_{line} = compute({line});\n"));
		}
		diff.push_str("-removed\n");
	}
	diff
}

/// Noul answers `h0..hn` with the given probabilities.
fn hunk_answers(probabilities: &[f64]) -> FakeEvaluationStep {
	let pairs: Vec<(String, f64)> = probabilities
		.iter()
		.enumerate()
		.map(|(slot, p)| (crate::supervisor::evaluate::gate_hunk_question_id(slot), *p))
		.collect();
	let borrowed: Vec<(&str, f64)> = pairs.iter().map(|(id, p)| (id.as_str(), *p)).collect();
	FakeEvaluationStep::Answers(nouls(&borrowed))
}

const CONDITIONS: [&str; 2] = ["The flag is renamed.", "The old flag is rejected."];

#[test]
fn split_hunks_separates_the_header_from_each_hunk() {
	let diff = padded_file_diff("src/a.rs", 3);
	let (preamble, hunks) = split_hunks(&diff);
	assert!(preamble.starts_with("diff --git a/src/a.rs") && preamble.ends_with("+++ b/src/a.rs\n"));
	assert_eq!(hunks.len(), 3);
	assert!(hunks.iter().all(|hunk| hunk.starts_with("@@ -")));
	assert_eq!(
		format!("{preamble}{}", hunks.concat()),
		diff,
		"nothing is lost or duplicated"
	);
	let (header_only, none) = split_hunks("diff --git a/x b/x\nBinary files differ\n");
	assert_eq!(header_only, "diff --git a/x b/x\nBinary files differ\n");
	assert!(none.is_empty());
}

#[test]
fn an_omitted_hunk_names_its_header_and_size() {
	let stub = omitted_hunk("@@ -1,2 +1,3 @@ fn f\n context\n+one\n+two\n-three\n");
	assert_eq!(stub, "@@ -1,2 +1,3 @@ fn f\n(hunk not shown: +2 -1 lines)\n");
}

#[tokio::test]
async fn the_evaluation_keeps_the_hunks_the_conditions_depend_on_and_stubs_the_rest() {
	let fake = install_fake_evaluation(vec![hunk_answers(&[0.9, 0.1, 0.1, 0.9, 0.1])]).await;
	let path = "src/a.rs".to_string();
	let conditions: Vec<String> = CONDITIONS.iter().map(ToString::to_string).collect();

	let selected = relevant_hunks(
		&config(true),
		"rename the flag",
		&conditions,
		&[(&path, padded_file_diff(&path, 5))],
	)
	.await
	.expect("the evaluation answered");

	assert_eq!(selected.len(), 1);
	let text = &selected[0].1;
	for kept in [0, 3] {
		assert!(
			text.contains(&format!("marker-src/a.rs-{kept}")),
			"hunk {kept} is read in full"
		);
		assert!(
			text.contains(&format!("padding_{kept}_{}", HUNK_BODY_LINES - 1)),
			"all of hunk {kept}"
		);
	}
	for dropped in [1, 2, 4] {
		let start = dropped * 100 + 1;
		assert!(!text.contains(&format!("marker-src/a.rs-{dropped}")));
		assert!(text.contains(&format!(
			"@@ -{start},3 +{start},4 @@ fn region_{dropped}\n(hunk not shown:"
		)));
	}
	assert!(text.starts_with("diff --git a/src/a.rs"), "the file header stays");

	let requests = fake.requests();
	assert_eq!(requests.len(), 1);
	let state = &requests[0].state;
	assert_eq!(
		state["conditions"].as_array().map(Vec::len),
		Some(2),
		"the conditions reach the evaluation"
	);
	assert_eq!(state["task"], "rename the flag");
	assert_eq!(state["hunks"].as_array().map(Vec::len), Some(5));
	assert_eq!(requests[0].questions.len(), 5);
}

#[tokio::test]
async fn a_hunk_at_the_keep_threshold_is_read_and_one_below_it_is_stubbed() {
	use crate::supervisor::evaluate::GATE_HUNK_KEEP_AT;
	let path = "src/a.rs".to_string();
	let diffs = [(&path, padded_file_diff(&path, 3))];
	let _fake = install_fake_evaluation(vec![hunk_answers(&[GATE_HUNK_KEEP_AT, GATE_HUNK_KEEP_AT - 0.01, 0.9])]).await;
	let selected = relevant_hunks(&config(true), "rename the flag", &[], &diffs)
		.await
		.expect("two hunks are kept");
	let text = &selected[0].1;
	assert_eq!(text.matches("(hunk not shown").count(), 1);
	assert!(
		!text.contains("marker-src/a.rs-1"),
		"the hunk below the threshold is the stub"
	);

	let _fake = install_fake_evaluation(vec![hunk_answers(&[0.1, 0.1, 0.1])]).await;
	assert!(
		relevant_hunks(&config(true), "rename the flag", &[], &diffs)
			.await
			.is_none(),
		"keeping no hunk is not a selection"
	);
}

#[tokio::test]
async fn the_evaluation_reads_a_bounded_sample_of_each_hunk() {
	let fake = install_fake_evaluation(vec![hunk_answers(&[0.9])]).await;
	let path = "src/a.rs".to_string();
	let diff = padded_file_diff(&path, 1);
	relevant_hunks(&config(true), "rename the flag", &[], &[(&path, diff.clone())])
		.await
		.expect("the evaluation answered");
	let shown = fake.requests()[0].state["hunks"][0]["text"]
		.as_str()
		.unwrap()
		.to_string();
	assert!(shown.len() < diff.len(), "the oversized hunk is sampled");
	assert!(crate::session::estimate_tokens(&shown) <= crate::supervisor::evaluate::GATE_HUNK_SAMPLE_TOKENS);
}

#[tokio::test]
async fn an_unavailable_evaluation_selects_nothing() {
	let _fake = install_fake_evaluation(vec![FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY")]).await;
	let path = "src/a.rs".to_string();
	assert!(relevant_hunks(
		&config(true),
		"rename the flag",
		&[],
		&[(&path, padded_file_diff(&path, 2))]
	)
	.await
	.is_none());
}

#[tokio::test]
async fn a_diff_without_hunks_is_not_scored() {
	let fake = install_fake_evaluation(vec![hunk_answers(&[0.9])]).await;
	let path = "logo.png".to_string();
	let binary = "diff --git a/logo.png b/logo.png\nBinary files differ\n".to_string();
	assert!(
		relevant_hunks(&config(true), "rename the flag", &[], &[(&path, binary)])
			.await
			.is_none()
	);
	assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn the_seam_off_renders_the_original_block_without_a_call() {
	let fake = install_fake_evaluation(vec![hunk_answers(&[0.9, 0.9, 0.9])]).await;
	let mut ledger = EvidenceLedger::default();
	ledger.fold_job_completion(None, "cargo test", "test result: ok");
	let gate_input = GateInput {
		ledger: &ledger,
		..input(ACTIONS, &[])
	};
	let rendered = render_ground_truth_for(&config(false), &gate_input, None).await;
	assert_eq!(rendered.text, render_ground_truth(&ledger).text);
	assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn the_detector_note_follows_the_ground_truth() {
	const NOTE: &str = "Verification detector: no qualifying check was recognized.";
	let mut ledger = EvidenceLedger::default();
	ledger.fold_job_completion(None, "cargo test", "test result: ok");
	let gate_input = GateInput {
		ledger: &ledger,
		..input(ACTIONS, &[])
	};
	let rendered = render_ground_truth_for(&config(false), &gate_input, Some(NOTE)).await;
	assert_eq!(
		rendered.text,
		format!("{}\n\n{NOTE}", render_ground_truth(&ledger).text)
	);
	let nothing = render_ground_truth_for(&config(false), &input(ACTIONS, &[]), Some(NOTE)).await;
	assert_eq!(nothing.text, NOTE, "no ground truth leaves the note alone");
}

#[test]
fn the_selected_diff_budget_is_what_the_verifier_window_leaves() {
	let path = "src/a.rs".to_string();
	let selected = [(&path, padded_file_diff(&path, 2))];
	let mut config = config(true);
	config.supervisor.model.model = Some("no-such-provider:m".to_string());
	assert_eq!(
		verifier_diff_budget(&config, &input(ACTIONS, &[]), None, &selected),
		GT_DIFF_MAX,
		"an unknown window keeps the size cut's budget"
	);

	config.supervisor.model.model = Some("anthropic:claude-sonnet-4-6".to_string());
	let roomy = verifier_diff_budget(&config, &input(ACTIONS, &[]), None, &selected);
	let long_actions = "#1 [read] shell cargo test → ok, every crate compiled and linked\n".repeat(3_000);
	let crowded = verifier_diff_budget(&config, &input(&long_actions, &[]), None, &selected);
	assert!(
		roomy > crowded,
		"the rest of the input shrinks the room: {roomy} vs {crowded}"
	);
	assert!(
		crowded > GT_DIFF_MAX,
		"a large window still fits more than the size cut"
	);
}

#[test]
fn a_diff_budget_keeps_the_standard_margins_around_the_diff() {
	let standard = GroundTruthLimits::STANDARD;
	for budget in [1, GT_DIFF_MAX, 40_000] {
		let limits = GroundTruthLimits::around_diff(budget);
		assert_eq!(limits.diff, budget);
		assert_eq!(limits.changes - limits.diff, standard.changes - standard.diff);
		assert_eq!(limits.total - limits.diff, standard.total - standard.diff);
	}
	assert_eq!(standard.diff, GT_DIFF_MAX);
}
