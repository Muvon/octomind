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

use super::*;

#[test]
fn folds_chat_decisions_and_outcomes_without_evaluate_rows_or_double_usage() {
	let mut info = crate::session::SessionInfo::default();
	info.supervisor_accounting.kinds.insert(
		CallKind::Gate,
		Usage {
			calls: 4,
			input_tokens: 100,
			output_tokens: 20,
			reasoning_tokens: 5,
			api_time_ms: 800,
			cost: 0.25,
			blocking_wall_time_ms: 1_400,
			outcome: [(Outcome::Runs, 2), (Outcome::RefutedGaps, 2)].into(),
			..Default::default()
		},
	);
	let decision = Usage {
		calls: 2,
		input_tokens: 50,
		output_tokens: 10,
		api_time_ms: 500,
		cost: 0.125,
		blocking_wall_time_ms: 900,
		..Default::default()
	};
	info.supervisor_accounting
		.kinds
		.insert(CallKind::Evaluate, decision.clone());
	info.supervisor_accounting
		.seams
		.insert(Seam::Gate, decision);
	info.supervisor_accounting
		.kinds
		.insert(CallKind::Resolve, Usage::default());
	info.supervisor_accounting.blocking_wall_time_ms = 1_400;
	let report = SupervisorReport::for_session(&info);
	let gate = &report.mechanics[&Mechanic::Gate];
	assert_eq!(
		(
			gate.chat_calls,
			gate.decision_calls,
			gate.wall_blocking_ms,
			gate.api_ms
		),
		(4, 2, 1_400, 1_300)
	);
	assert_eq!(
		(gate.input_tokens, gate.output_tokens, gate.reasoning_tokens),
		(150, 30, 5)
	);
	assert_eq!(gate.outcome[&Outcome::Runs], 2);
	assert_eq!(gate.outcome[&Outcome::RefutedGaps], 2);
	assert_eq!(report.totals.cost, 0.375);
	assert_eq!(report.totals.wall_blocking_ms, 1_400);
	assert_eq!(report.mechanics.len(), 1);
	let json = serde_json::to_value(&report).unwrap();
	assert!(json.get("kinds").is_none());
	assert!(json.get("seams").is_none());
	assert!(json["mechanics"].get("evaluate").is_none());
	assert_eq!(json["mechanics"]["gate"]["outcome"]["runs"], 2);
}

#[test]
fn every_decision_seam_has_exactly_one_mechanic() {
	let mut info = crate::session::SessionInfo::default();
	for (seam, mechanic) in [
		(Seam::Gate, Mechanic::Gate),
		(Seam::Authorizer, Mechanic::Authorize),
		(Seam::Condense, Mechanic::Condense),
		(Seam::Recall, Mechanic::Recall),
		(Seam::Distill, Mechanic::Distill),
		(Seam::Plan, Mechanic::Plan),
		(Seam::Evolution, Mechanic::Evolution),
		(Seam::Skills, Mechanic::Skills),
		(Seam::Capabilities, Mechanic::Capabilities),
		(Seam::Compression, Mechanic::Compression),
	] {
		assert_eq!(Mechanic::from_seam(seam), mechanic);
		info.supervisor_accounting.seams.insert(
			seam,
			Usage {
				calls: 1,
				..Default::default()
			},
		);
	}
	let report = SupervisorReport::for_session(&info);
	assert_eq!(report.mechanics.len(), Seam::ALL.len());
	assert_eq!(report.totals.decision_calls, Seam::ALL.len() as u64);
	assert_eq!(report.totals.chat_calls, 0);
}

#[test]
fn authorizer_uses_the_existing_persisted_state_without_duplicate_counters() {
	let mut info = crate::session::SessionInfo::default();
	info.authorization.checked = 11;
	info.authorization.cached = 7;
	info.authorization.unavailable = 1;
	let report = SupervisorReport::for_session(&info);
	let outcome = &report.mechanics[&Mechanic::Authorize].outcome;
	assert_eq!(outcome[&Outcome::Checked], 11);
	assert_eq!(outcome[&Outcome::Cached], 7);
	assert!(!outcome.contains_key(&Outcome::Blocked));
	assert!(info.supervisor_accounting.kinds.is_empty());
}

#[test]
fn old_background_totals_stay_unknown_instead_of_summing_nested_intervals() {
	let mut info = crate::session::SessionInfo::default();
	info.supervisor_accounting.kinds.insert(
		CallKind::Distill,
		Usage {
			background_wall_time_ms: 100,
			..Default::default()
		},
	);
	let mut json = serde_json::to_value(&info).unwrap();
	json["supervisor_accounting"]
		.as_object_mut()
		.unwrap()
		.remove("background_wall_time_ms");
	let restored: crate::session::SessionInfo = serde_json::from_value(json).unwrap();
	let report = SupervisorReport::for_session(&restored);
	assert_eq!(report.totals.wall_background_ms, None);
	assert_eq!(report.mechanics[&Mechanic::Distill].wall_background_ms, 100);
}

#[tokio::test]
async fn resume_preserves_the_same_mechanic_outcomes_and_authorizer_state() {
	crate::supervisor::accounting::isolated(async {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("outcomes.jsonl.zst");
		let id = uuid::Uuid::new_v4().to_string();
		crate::session::context::with_session_id(id.clone(), async {
			let mut session = crate::session::Session::new(id, "model".into());
			session.session_file = Some(path.clone());
			session.info.authorization.checked = 11;
			crate::supervisor::stats::gate_run();
			crate::supervisor::stats::gate_fail();
			crate::supervisor::accounting::record_outcome(Mechanic::Gate, Outcome::RefutedGaps, 2);
			crate::supervisor::stats::condensed(2, 5_700);
			session.fold_external_spend();
			let expected = SupervisorReport::for_session(&session.info);
			session.save().unwrap();
			let mut resumed = crate::session::persistence::load_session(&path).unwrap();
			resumed.fold_external_spend();
			assert_eq!(SupervisorReport::for_session(&resumed.info), expected);
			assert_eq!(
				expected.mechanics[&Mechanic::Gate].outcome[&Outcome::RefutedGaps],
				2
			);
			assert_eq!(
				expected.mechanics[&Mechanic::Authorize].outcome[&Outcome::Checked],
				11
			);
		})
		.await;
	})
	.await;
}

#[test]
fn advisory_only_gate_outcomes_survive_reporting_and_old_reports_still_load() {
	let mut info = crate::session::SessionInfo::default();
	info.supervisor_accounting.kinds.insert(
		CallKind::Gate,
		Usage {
			reported_findings: ["output outside gate view".to_string()].into(),
			..Default::default()
		},
	);
	let report = SupervisorReport::for_session(&info);
	let mut json = serde_json::to_value(report).unwrap();
	assert_eq!(
		json["mechanics"]["gate"]["reported_findings"],
		serde_json::json!(["output outside gate view"])
	);
	json["mechanics"]["gate"]
		.as_object_mut()
		.unwrap()
		.remove("reported_findings");
	let restored: SupervisorReport = serde_json::from_value(json).unwrap();
	assert!(restored.mechanics[&Mechanic::Gate]
		.reported_findings
		.is_empty());
	let usage = serde_json::to_value(Usage::default()).unwrap();
	assert!(usage.get("reported_findings").is_none());
	assert!(serde_json::from_value::<Usage>(usage)
		.unwrap()
		.reported_findings
		.is_empty());
}
