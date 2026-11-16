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

/// One of octomind's own rows, read back from the record.
fn row(report: &SupervisorReport, mechanic: Mechanic) -> MechanicUsage {
	serde_json::from_value(report.mechanics[mechanic.name()].clone()).unwrap()
}

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
	info.supervisor_accounting.seams.insert(Seam::Gate, decision);
	info.supervisor_accounting
		.kinds
		.insert(CallKind::Resolve, Usage::default());
	info.supervisor_accounting.blocking_wall_time_ms = 1_400;
	let report = SupervisorReport::for_session(&info);
	let gate = row(&report, Mechanic::Gate);
	assert_eq!(
		(gate.chat_calls, gate.decision_calls, gate.wall_blocking_ms, gate.api_ms),
		(4, 2, 1_400, 1_300)
	);
	assert_eq!(
		(gate.input_tokens, gate.output_tokens, gate.reasoning_tokens),
		(150, 30, 5)
	);
	assert_eq!(gate.outcome["runs"], 2);
	assert_eq!(gate.outcome["refuted_gaps"], 2);
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
	// One row per `Seam` variant: the table below must stay in step with the enum.
	const SEAM_COUNT: usize = 12;
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
		(Seam::Advise, Mechanic::Advise),
		(Seam::Taps, Mechanic::Taps),
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
	assert_eq!(report.mechanics.len(), SEAM_COUNT);
	assert_eq!(report.totals.decision_calls, SEAM_COUNT as u64);
	assert_eq!(report.totals.chat_calls, 0);
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
	assert_eq!(row(&report, Mechanic::Distill).wall_background_ms, 100);
}

#[tokio::test]
async fn resume_preserves_the_same_mechanic_outcomes() {
	crate::session::accounting::isolated(async {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("outcomes.jsonl.zst");
		let id = uuid::Uuid::new_v4().to_string();
		crate::session::context::with_session_id(id.clone(), async {
			let mut session = crate::session::Session::new(id, "model".into());
			session.session_file = Some(path.clone());
			crate::session::accounting::record(
				CallKind::Gate,
				Usage {
					outcome: [(Outcome::Runs, 1), (Outcome::Fail, 1), (Outcome::RefutedGaps, 2)].into(),
					..Default::default()
				},
			);
			session.fold_external_spend();
			let expected = SupervisorReport::for_session(&session.info);
			session.save().unwrap();
			let mut resumed = crate::session::persistence::load_session(&path).unwrap();
			resumed.fold_external_spend();
			assert_eq!(SupervisorReport::for_session(&resumed.info), expected);
			assert_eq!(row(&expected, Mechanic::Gate).outcome["refuted_gaps"], 2);
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
			reported_findings: vec!["output outside gate view".to_string()],
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
	assert!(row(&restored, Mechanic::Gate).reported_findings.is_empty());
	let usage = serde_json::to_value(Usage::default()).unwrap();
	assert!(usage.get("reported_findings").is_none());
	assert!(serde_json::from_value::<Usage>(usage)
		.unwrap()
		.reported_findings
		.is_empty());
}

#[test]
fn include_replaces_findings_on_clean_gate_run_and_keeps_them_for_unrelated_usage() {
	let mut row = MechanicUsage::default();
	row.include(&Usage {
		outcome: BTreeMap::from([(Outcome::Reported, 1)]),
		reported_findings: vec!["finding 1".to_string()],
		..Default::default()
	});
	assert_eq!(row.reported_findings, vec!["finding 1".to_string()]);
	// A clean latest gate run clears the stale findings.
	row.include(&Usage {
		outcome: BTreeMap::from([(Outcome::Reported, 0)]),
		..Default::default()
	});
	assert!(row.reported_findings.is_empty());
	// An unrelated empty Usage (no gate run) keeps them.
	row.include(&Usage {
		outcome: BTreeMap::from([(Outcome::Reported, 1)]),
		reported_findings: vec!["finding 2".to_string()],
		..Default::default()
	});
	row.include(&Usage::default());
	assert_eq!(row.reported_findings, vec!["finding 2".to_string()]);
}

/// What a plugin sends: a row of its own shape, and the sums.
fn plugin_report() -> protocol::Report {
	protocol::Report {
		mechanics: BTreeMap::from([
			(
				"completion_check".to_string(),
				serde_json::json!({"chat_calls": 3, "cost": 0.5, "outcome": {"kept_going": 2}, "extra": "x"}),
			),
			("odd".to_string(), serde_json::json!("not an object")),
		]),
		totals: protocol::ReportTotals {
			chat_calls: 3,
			wall_blocking_ms: 700,
			wall_background_ms: Some(50),
			input_tokens: 90,
			cost: 0.5,
			..Default::default()
		},
	}
}

fn info_with_gate_row() -> crate::session::SessionInfo {
	let mut info = crate::session::SessionInfo::default();
	info.supervisor_accounting.kinds.insert(
		CallKind::Gate,
		Usage {
			calls: 1,
			input_tokens: 10,
			cost: 0.25,
			blocking_wall_time_ms: 100,
			..Default::default()
		},
	);
	info.supervisor_accounting.blocking_wall_time_ms = 100;
	info
}

#[test]
fn plugin_rows_appear_unchanged_under_their_names_and_totals_add_up() {
	let mut info = info_with_gate_row();
	let before = SupervisorReport::for_session(&info);
	info.supervisor_accounting
		.plugin_reports
		.insert("besydr".to_string(), plugin_report());
	let report = SupervisorReport::for_session(&info);
	let sent = plugin_report();
	assert_eq!(report.mechanics["completion_check"], sent.mechanics["completion_check"]);
	assert_eq!(report.mechanics["odd"], sent.mechanics["odd"]);
	assert_eq!(report.mechanics["gate"], before.mechanics["gate"]);
	assert_eq!(report.totals.chat_calls, 4);
	assert_eq!(report.totals.input_tokens, 100);
	assert_eq!(report.totals.cost, 0.75);
	assert_eq!(report.totals.wall_blocking_ms, 800);
	assert_eq!(report.totals.wall_background_ms, Some(50));

	// One side that cannot tell its background time makes the sum unknown.
	info.supervisor_accounting.background_wall_time_ms = None;
	assert_eq!(SupervisorReport::for_session(&info).totals.wall_background_ms, None);
}

#[test]
fn a_plugin_row_replaces_a_row_of_the_same_name() {
	let mut info = info_with_gate_row();
	let theirs = serde_json::json!({"chat_calls": 9});
	info.supervisor_accounting.plugin_reports.insert(
		"besydr".to_string(),
		protocol::Report {
			mechanics: BTreeMap::from([("gate".to_string(), theirs.clone())]),
			..Default::default()
		},
	);
	assert_eq!(SupervisorReport::for_session(&info).mechanics["gate"], theirs);
}

#[test]
fn usage_alone_makes_rows_only_when_no_report_exists() {
	let mut info = info_with_gate_row();
	info.supervisor_accounting.plugin_usage.insert(
		"checks".to_string(),
		Usage {
			calls: 2,
			input_tokens: 30,
			output_tokens: 4,
			cost: 0.125,
			blocking_wall_time_ms: 100,
			background_wall_time_ms: 40,
			..Default::default()
		},
	);
	let report = SupervisorReport::for_session(&info);
	let checks: MechanicUsage = serde_json::from_value(report.mechanics["checks"].clone()).unwrap();
	assert_eq!(
		(
			checks.chat_calls,
			checks.input_tokens,
			checks.output_tokens,
			checks.cost
		),
		(2, 30, 4, 0.125)
	);
	assert_eq!((checks.wall_blocking_ms, checks.wall_background_ms), (100, 40));
	assert_eq!(report.totals.chat_calls, 3);
	assert_eq!(report.totals.cost, 0.375);
	// The blocking time of plugin usage is already in the session's own total.
	assert_eq!(report.totals.wall_blocking_ms, 100);
	assert_eq!(report.totals.wall_background_ms, Some(40));

	info.supervisor_accounting
		.plugin_reports
		.insert("besydr".to_string(), plugin_report());
	assert!(!SupervisorReport::for_session(&info).mechanics.contains_key("checks"));
}

#[test]
fn a_report_replaces_the_blocking_time_booked_from_the_plugins_usage() {
	let mut info = info_with_gate_row();
	info.supervisor_accounting.plugin_usage.insert(
		"checks".to_string(),
		Usage {
			blocking_wall_time_ms: 600,
			..Default::default()
		},
	);
	// Booking a plugin's usage adds its blocking time to the session's own total.
	info.supervisor_accounting.blocking_wall_time_ms += 600;
	info.supervisor_accounting
		.plugin_reports
		.insert("besydr".to_string(), plugin_report());

	// 100 of octomind's own and the report's 700; the 600 is part of the 700.
	assert_eq!(SupervisorReport::for_session(&info).totals.wall_blocking_ms, 800);
}

#[test]
fn a_session_file_without_plugin_reports_gives_the_record_it_gave_before() {
	let info = info_with_gate_row();
	let json = serde_json::to_value(&info).unwrap();
	assert!(json["supervisor_accounting"].get("plugin_reports").is_none());
	let restored: crate::session::SessionInfo = serde_json::from_value(json).unwrap();
	assert!(restored.supervisor_accounting.plugin_reports.is_empty());
	assert_eq!(
		serde_json::to_value(SupervisorReport::for_session(&restored)).unwrap(),
		serde_json::to_value(SupervisorReport::for_session(&info)).unwrap()
	);
}

#[test]
fn adding_and_merging_accounting_replaces_a_plugins_report() {
	let report_of = |chat_calls| protocol::Report {
		totals: protocol::ReportTotals {
			chat_calls,
			..Default::default()
		},
		..Default::default()
	};
	let mut older = crate::session::accounting::SupervisorAccounting::default();
	older.plugin_reports.insert("besydr".to_string(), report_of(1));
	let mut newer = crate::session::accounting::SupervisorAccounting::default();
	newer.plugin_reports.insert("besydr".to_string(), report_of(5));
	let mut added = older.clone();
	added.add(&newer);
	assert_eq!(added.plugin_reports["besydr"].totals.chat_calls, 5);
	let mut merged = older.clone();
	merged.merge_max(&newer);
	assert_eq!(merged.plugin_reports["besydr"].totals.chat_calls, 5);
}
