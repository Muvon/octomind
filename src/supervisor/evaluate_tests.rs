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

//! Seam runner behavior with the in-process fake provider: switches, the
//! single bounded attempt, the state cap, fallback accounting, and the
//! question builders. The stats sink is process-global, so every counter
//! assertion is a delta around the call; `install_fake_evaluation` holds
//! `ENV_LOCK` so fake-driven tests never overlap.

use super::*;
use crate::session::chat::test_support::{
	choice, evaluate_counter, install_fake_evaluation, nouls, FakeEvaluationStep,
};

fn config(recall: bool, skills: bool, authorizer: bool) -> crate::supervisor::SupervisorConfig {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../config-templates/default.toml"))
			.expect("default template must deserialize");
	config.supervisor.enabled = true;
	config.supervisor.evaluate.recall = recall;
	config.supervisor.evaluate.skills = skills;
	config.supervisor.evaluate.authorizer = authorizer;
	config.supervisor
}

fn snapshot_u64(key: &str) -> u64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get(key)?.as_u64())
		.unwrap_or(0)
}

fn snapshot_f64(key: &str) -> f64 {
	crate::supervisor::stats::snapshot()
		.and_then(|s| s.get(key)?.as_f64())
		.unwrap_or(0.0)
}

#[test]
fn template_defaults_every_seam_off_on_cloudflare_jev() {
	let config = config(false, false, false);
	assert_eq!(config.evaluate.model, "cloudflare:typesafe/jev");
	assert!(!config.evaluate.recall && !config.evaluate.skills && !config.evaluate.authorizer);
	for seam in Seam::ALL {
		assert!(!enabled(&config, seam));
	}
}

#[test]
fn master_switch_off_disables_every_seam() {
	let mut config = config(true, true, true);
	config.evaluate.condense = true;
	config.evaluate.compression = true;
	for seam in Seam::ALL {
		assert!(enabled(&config, seam));
	}
	config.enabled = false;
	for seam in Seam::ALL {
		assert!(!enabled(&config, seam));
	}
}

#[tokio::test]
async fn seams_off_make_no_call_and_touch_no_counter() {
	let fake =
		install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[("c0", 0.9)]))]).await;
	let before: Vec<u64> = Seam::ALL
		.iter()
		.map(|seam| evaluate_counter(*seam, "calls") + evaluate_counter(*seam, "unavailable"))
		.collect();
	let mut config = config(false, false, false);
	for seam in Seam::ALL {
		let answers = run(
			&config,
			seam,
			serde_json::json!("state"),
			recall_questions(1),
		)
		.await;
		assert!(answers.is_none());
	}
	// Master switch off wins even with every seam switched on.
	config.evaluate.recall = true;
	config.evaluate.skills = true;
	config.evaluate.authorizer = true;
	config.enabled = false;
	for seam in Seam::ALL {
		assert!(run(
			&config,
			seam,
			serde_json::json!("state"),
			recall_questions(1)
		)
		.await
		.is_none());
	}
	assert!(fake.requests().is_empty());
	let after: Vec<u64> = Seam::ALL
		.iter()
		.map(|seam| evaluate_counter(*seam, "calls") + evaluate_counter(*seam, "unavailable"))
		.collect();
	assert_eq!(before, after);
}

#[tokio::test]
async fn answers_come_back_keyed_by_question_id_and_are_attributed() {
	let fake = install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[
		("c0", 0.91),
		("c1", 0.08),
	]))])
	.await;
	let config = config(true, false, false);
	let calls = evaluate_counter(Seam::Recall, "calls");
	let evaluate_calls = snapshot_u64("evaluate_calls");
	let input_tokens = snapshot_u64("input_tokens");
	let cost = snapshot_f64("cost");
	let spend = crate::session::context::with_session_id("__evaluate_spend".to_string(), async {
		let _ = crate::session::external_spend::take();
		let answers = run(
			&config,
			Seam::Recall,
			serde_json::json!({"request": "add retry"}),
			recall_questions(2),
		)
		.await
		.expect("answers");
		assert_eq!(answers["c0"], Answer::Noul { noul: 0.91 });
		assert_eq!(answers["c1"], Answer::Noul { noul: 0.08 });
		crate::session::external_spend::take()
	})
	.await;
	let request = &fake.requests()[0];
	assert_eq!(request.model, "typesafe/jev");
	assert_eq!(request.max_retries, 0, "one attempt, never a retry chain");
	assert_eq!(request.timeout, std::time::Duration::from_secs(5));
	assert_eq!(request.questions.len(), 2);
	assert_eq!(evaluate_counter(Seam::Recall, "calls"), calls + 1);
	assert_eq!(snapshot_u64("evaluate_calls"), evaluate_calls + 1);
	assert_eq!(snapshot_u64("input_tokens"), input_tokens + 100);
	assert!((snapshot_f64("cost") - cost - 0.000_004_2).abs() < 1e-12);
	assert!(
		spend >= 0.000_004_2,
		"session external spend must include the call: {spend}"
	);
}

#[tokio::test]
async fn missing_key_falls_through_with_one_unavailable() {
	let fake =
		install_fake_evaluation(vec![FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY")]).await;
	let config = config(true, false, false);
	let unavailable = evaluate_counter(Seam::Recall, "unavailable");
	let calls = evaluate_counter(Seam::Recall, "calls");
	let answers = run(
		&config,
		Seam::Recall,
		serde_json::json!("state"),
		recall_questions(1),
	)
	.await;
	assert!(answers.is_none());
	assert_eq!(fake.requests().len(), 1);
	assert_eq!(
		evaluate_counter(Seam::Recall, "unavailable"),
		unavailable + 1
	);
	assert_eq!(evaluate_counter(Seam::Recall, "calls"), calls + 1);
}

#[tokio::test]
async fn a_hung_provider_is_cut_at_the_timeout() {
	let _fake = install_fake_evaluation(vec![FakeEvaluationStep::Sleep(
		std::time::Duration::from_secs(6),
	)])
	.await;
	let config = config(false, true, false);
	let unavailable = evaluate_counter(Seam::Skills, "unavailable");
	let started = std::time::Instant::now();
	let answers = run(
		&config,
		Seam::Skills,
		serde_json::json!("state"),
		std::collections::BTreeMap::from([(
			SKILL_QUESTION_ID.to_string(),
			skill_question([("a", "A"), ("b", "B")]),
		)]),
	)
	.await;
	assert!(answers.is_none());
	assert!(started.elapsed() <= std::time::Duration::from_millis(5_500));
	assert_eq!(
		evaluate_counter(Seam::Skills, "unavailable"),
		unavailable + 1
	);
}

#[tokio::test]
async fn oversized_state_is_never_sent() {
	let fake =
		install_fake_evaluation(vec![FakeEvaluationStep::Answers(nouls(&[("c0", 0.9)]))]).await;
	let config = config(true, false, false);
	let unavailable = evaluate_counter(Seam::Recall, "unavailable");
	let calls = evaluate_counter(Seam::Recall, "calls");
	// ~25k tokens of distinct words; well above the 24,000 cap.
	let state = (0..25_000)
		.map(|i| format!("w{i}"))
		.collect::<Vec<_>>()
		.join(" ");
	assert!(crate::session::estimate_tokens(&state) > MAX_STATE_TOKENS);
	let answers = run(
		&config,
		Seam::Recall,
		serde_json::json!(state),
		recall_questions(1),
	)
	.await;
	assert!(answers.is_none());
	assert!(fake.requests().is_empty());
	assert_eq!(
		evaluate_counter(Seam::Recall, "unavailable"),
		unavailable + 1
	);
	assert_eq!(evaluate_counter(Seam::Recall, "calls"), calls);
}

#[tokio::test]
async fn missing_or_mistyped_answers_are_an_invalid_response() {
	let _fake = install_fake_evaluation(vec![
		FakeEvaluationStep::Answers(nouls(&[("c0", 0.9)])),
		FakeEvaluationStep::Answers(choice("c0", "x", 1.0)),
	])
	.await;
	let config = config(true, false, false);
	let unavailable = evaluate_counter(Seam::Recall, "unavailable");
	// Two questions asked, one answered.
	assert!(run(
		&config,
		Seam::Recall,
		serde_json::json!("s"),
		recall_questions(2)
	)
	.await
	.is_none());
	// Right id, wrong answer type.
	assert!(run(
		&config,
		Seam::Recall,
		serde_json::json!("s"),
		recall_questions(1)
	)
	.await
	.is_none());
	assert_eq!(
		evaluate_counter(Seam::Recall, "unavailable"),
		unavailable + 2
	);
}

#[tokio::test]
async fn snapshot_lists_every_active_seam_with_its_three_counters() {
	let _lock = install_fake_evaluation(Vec::new()).await;
	crate::supervisor::stats::evaluate_call(Seam::Recall);
	crate::supervisor::stats::evaluate_applied(Seam::Recall, 1);
	let snapshot = crate::supervisor::stats::snapshot().expect("non-idle");
	let recall = &snapshot["evaluate"]["recall"];
	for key in ["calls", "unavailable", "applied"] {
		assert!(
			recall.get(key).and_then(|v| v.as_u64()).is_some(),
			"missing {key}"
		);
	}
	assert!(recall["calls"].as_u64().unwrap() >= 1);
	assert!(recall["applied"].as_u64().unwrap() >= 1);
}

#[test]
fn question_builders_use_stable_ids_and_documented_options() {
	let recall = recall_questions(3);
	assert_eq!(
		recall.keys().cloned().collect::<Vec<_>>(),
		vec!["c0", "c1", "c2"]
	);
	assert!(recall.values().all(|q| matches!(q, Question::Noul { .. })));

	match skill_question([("git-workflow", "Git"), ("code-review", "Review")]) {
		Question::Choice { criteria, .. } => {
			assert_eq!(
				criteria.keys().cloned().collect::<Vec<_>>(),
				vec!["code-review", "git-workflow", "none"]
			);
			assert_eq!(
				criteria["none"],
				Some(serde_json::Value::String(
					"No skill in this list applies to the request".into()
				))
			);
		}
		other => panic!("expected a choice, got {other:?}"),
	}

	let authorizer = authorizer_questions(&["0".into(), "3".into()]);
	assert_eq!(authorizer.len(), 6);
	for id in [
		"0.prohibited",
		"0.destructive",
		"0.external",
		"3.prohibited",
	] {
		assert!(
			matches!(authorizer.get(id), Some(Question::Noul { .. })),
			"missing {id}"
		);
	}
}

// ---------------------------------------------------------------------------
// Window packing and the all-or-nothing multi-window runner
// ---------------------------------------------------------------------------

#[test]
fn windows_respect_the_state_cap_and_the_question_bound_in_order() {
	// 200 items of 100 tokens: the state cap allows ~237 per window, so the
	// question bound splits first; then 40 items of 1,000 tokens hit the cap.
	let small: Vec<(usize, usize)> = (0..200).map(|i| (i, 100)).collect();
	let packed = windows(small, 500, |(_, tokens)| *tokens);
	assert_eq!(packed.len(), 3);
	assert!(packed.iter().all(|w| w.len() <= MAX_WINDOW_QUESTIONS));
	let order: Vec<usize> = packed.iter().flatten().map(|(i, _)| *i).collect();
	assert_eq!(order, (0..200).collect::<Vec<_>>());

	let large: Vec<(usize, usize)> = (0..40).map(|i| (i, 1_000)).collect();
	let packed = windows(large, 500, |(_, tokens)| *tokens);
	assert!(packed.len() >= 2);
	for window in &packed {
		let tokens: usize = window.iter().map(|(_, t)| *t).sum();
		assert!(
			tokens + 500 <= MAX_STATE_TOKENS,
			"window over the cap: {tokens}"
		);
	}
	assert!(windows(Vec::<usize>::new(), 0, |_| 1).is_empty());
}

#[tokio::test]
async fn run_windows_answers_every_window_in_order_or_falls_back_once() {
	let fake = install_fake_evaluation(vec![
		FakeEvaluationStep::Answers(nouls(&[("k0", 0.9)])),
		FakeEvaluationStep::Answers(nouls(&[("k0", 0.1)])),
		FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY"),
		FakeEvaluationStep::MissingKey("CLOUDFLARE_API_KEY"),
	])
	.await;
	let mut config = config(false, false, false);
	config.evaluate.condense = true;
	let seam = Seam::Condense;
	let calls_before = evaluate_counter(seam, "calls");
	let unavailable_before = evaluate_counter(seam, "unavailable");

	let answers = run_windows(
		&config,
		seam,
		vec![
			(serde_json::json!({"w": 0}), condense_questions(1)),
			(serde_json::json!({"w": 1}), condense_questions(1)),
		],
	)
	.await
	.expect("both windows answered");
	assert_eq!(answers.len(), 2);
	assert!(matches!(answers[0].get("k0"), Some(Answer::Noul { noul }) if *noul > 0.5));
	assert!(matches!(answers[1].get("k0"), Some(Answer::Noul { noul }) if *noul < 0.5));
	assert_eq!(fake.requests()[0].state["w"], 0);
	assert_eq!(fake.requests()[1].state["w"], 1);

	// Two failing windows count one fallback for the round, not two.
	let failed = run_windows(
		&config,
		seam,
		vec![
			(serde_json::json!({"w": 2}), condense_questions(1)),
			(serde_json::json!({"w": 3}), condense_questions(1)),
		],
	)
	.await;
	assert!(failed.is_none());
	assert_eq!(evaluate_counter(seam, "calls") - calls_before, 4);
	assert_eq!(
		evaluate_counter(seam, "unavailable") - unavailable_before,
		1
	);

	// Seam off: no call, no counter, and an empty round is not a fallback.
	config.evaluate.condense = false;
	assert!(run_windows(
		&config,
		seam,
		vec![(serde_json::json!({}), condense_questions(1))]
	)
	.await
	.is_none());
	config.evaluate.condense = true;
	assert_eq!(
		run_windows(&config, seam, Vec::new()).await,
		Some(Vec::new())
	);
	assert_eq!(fake.requests().len(), 4);
}

#[tokio::test]
async fn run_windows_issues_every_window_concurrently() {
	let delay = std::time::Duration::from_millis(300);
	let _fake = install_fake_evaluation(vec![
		FakeEvaluationStep::Sleep(delay),
		FakeEvaluationStep::Sleep(delay),
		FakeEvaluationStep::Sleep(delay),
	])
	.await;
	let mut config = config(false, false, false);
	config.evaluate.compression = true;
	let started = std::time::Instant::now();
	let answers = run_windows(
		&config,
		Seam::Compression,
		(0..3)
			.map(|w| (serde_json::json!({"w": w}), compression_questions(1)))
			.collect(),
	)
	.await;
	assert!(answers.is_none());
	assert!(
		started.elapsed() < delay * 2,
		"windows ran one after another: {:?}",
		started.elapsed()
	);
}

#[test]
fn condense_and_compression_questions_are_nouls_keyed_by_slot() {
	let condense = condense_questions(3);
	assert_eq!(
		condense.keys().cloned().collect::<Vec<_>>(),
		["k0", "k1", "k2"]
	);
	assert!(condense
		.values()
		.all(|question| matches!(question, Question::Noul { .. })));
	let compression = compression_questions(2);
	assert_eq!(
		compression.keys().cloned().collect::<Vec<_>>(),
		["u0", "u1"]
	);
	assert!(compression
		.values()
		.all(|question| matches!(question, Question::Noul { .. })));
	assert_eq!(Seam::ALL.len(), SEAM_COUNT);
	for (i, seam) in Seam::ALL.iter().enumerate() {
		assert_eq!(seam.index(), i);
	}
}
