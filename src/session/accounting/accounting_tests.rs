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
use crate::session::context::with_session_id;
use crate::session::Session;

#[tokio::test]
async fn concurrent_sessions_fold_only_their_own_usage_once() {
	let banks = Arc::new(Mutex::new(Banks::new()));
	let barrier = Arc::new(tokio::sync::Barrier::new(2));
	let mut tasks = Vec::new();
	for (id, cost) in [("accounting-a", 0.25), ("accounting-b", 0.75)] {
		let banks = Arc::clone(&banks);
		let bank = Arc::new(Mutex::new(Bank::default()));
		banks.lock().unwrap().insert(Some(id.into()), Arc::clone(&bank));
		let barrier = Arc::clone(&barrier);
		tasks.push(tokio::spawn(with_session_id(
			id.to_owned(),
			TEST_PENDING.scope(Arc::clone(&banks), async move {
				record_call(CallKind::Gate, 100, 20, 5, 120, cost);
				barrier.wait().await;
				let accounting = take_from(&mut banks.lock().unwrap(), Some(id.into()));
				let usage = &accounting.kinds[&CallKind::Gate];
				assert_eq!(
					(
						usage.calls,
						usage.input_tokens,
						usage.output_tokens,
						usage.reasoning_tokens,
						usage.api_time_ms
					),
					(1, 100, 20, 5, 120)
				);
				assert_eq!(usage.cost, cost);
				assert_eq!(
					take_from(&mut banks.lock().unwrap(), Some(id.into())),
					SupervisorAccounting::default()
				);
			}),
		)));
	}
	for task in tasks {
		task.await.unwrap();
	}
}

#[test]
fn unowned_usage_goes_to_next_fold() {
	let mut banks = Banks::new();
	let mut bank = Bank::default();
	bank.usage.kinds.insert(
		CallKind::Recall,
		Usage {
			calls: 1,
			cost: 0.5,
			..Default::default()
		},
	);
	banks.insert(None, Arc::new(Mutex::new(bank)));
	assert_eq!(
		take_from(&mut banks, Some("receiver".into())).kinds[&CallKind::Recall].cost,
		0.5
	);
	assert_eq!(
		take_from(&mut banks, Some("receiver".into())),
		SupervisorAccounting::default()
	);
}

#[tokio::test]
async fn detached_evolution_keeps_explicit_owner_while_other_session_folds() {
	isolated(async {
		let owner = uuid::Uuid::new_v4().to_string();
		let other = uuid::Uuid::new_v4().to_string();
		let (recorded_tx, recorded_rx) = tokio::sync::oneshot::channel();
		let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
		let expected_owner = owner.clone();
		let task = tokio::spawn(run_background(owner.clone(), None, async move {
			assert_eq!(current_session_id(), Some(expected_owner));
			assert!(BACKGROUND.with(|value| *value));
			record_call(CallKind::Evolution, 123, 45, 6, 700, 0.25);
			recorded_tx.send(()).unwrap();
			finish_rx.await.unwrap();
			Ok(())
		}));
		recorded_rx.await.unwrap();
		let mut receiver = Session::new(other.clone(), "model".into());
		for _ in 0..10 {
			receiver.fold_external_spend();
			assert!(receiver.info.supervisor_accounting.background_runs.is_empty());
			assert!(take_completed(&owner).is_empty(), "a running job cannot be live-folded");
		}
		finish_tx.send(()).unwrap();
		task.await.unwrap().unwrap();
		assert!(take_completed(&other).is_empty());
		let batches = take_completed(&owner);
		assert_eq!(batches.len(), 1);
		let usage = batches.values().next().unwrap();
		assert_eq!(usage.kinds[&CallKind::Evolution].cost, 0.25);
		assert_eq!(usage.blocking_wall_time_ms, 0);
	})
	.await;
}

#[tokio::test]
async fn detached_completion_after_last_save_survives_resume_and_live_fold_once() {
	isolated(async {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("session.jsonl.zst");
		let owner = uuid::Uuid::new_v4().to_string();
		let mut session = Session::new(owner.clone(), "model".into());
		session.session_file = Some(file.clone());
		session.save().unwrap();
		let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
		let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
		let task = tokio::spawn(run_background(owner.clone(), Some(file.clone()), async move {
			record_call(CallKind::Distill, 100, 20, 3, 50, 0.25);
			ready_tx.send(()).unwrap();
			finish_rx.await.unwrap();
			Ok(())
		}));
		ready_rx.await.unwrap();
		session.fold_external_spend();
		session.save().unwrap();
		finish_tx.send(()).unwrap();
		task.await.unwrap().unwrap();
		let mut resumed = crate::session::persistence::load_session(&file).unwrap();
		assert_eq!(resumed.info.supervisor_accounting.kinds[&CallKind::Distill].calls, 1);
		let saved = resumed.info.supervisor_accounting.clone();
		let cost = resumed.info.total_cost;
		resumed.fold_external_spend();
		assert_eq!(
			resumed.info.supervisor_accounting.background_runs,
			saved.background_runs
		);
		assert_eq!(resumed.info.supervisor_accounting.kinds[&CallKind::Distill].calls, 1);
		assert_eq!(resumed.info.total_cost, cost);
		resumed.save().unwrap();
		assert_eq!(
			crate::session::persistence::load_session(&file)
				.unwrap()
				.info
				.total_cost,
			cost
		);
	})
	.await;
}

#[test]
fn fold_keeps_a_bank_already_acquired_by_a_recorder() {
	let owner = Some("recorder".to_owned());
	let bank = Arc::new(Mutex::new(Bank::default()));
	let mut banks = Banks::new();
	banks.insert(owner.clone(), Arc::clone(&bank));
	assert_eq!(take_from(&mut banks, owner.clone()), SupervisorAccounting::default());
	bank.lock().unwrap().usage.kinds.insert(
		CallKind::Gate,
		Usage {
			calls: 1,
			cost: 0.25,
			..Default::default()
		},
	);
	drop(bank);
	assert_eq!(take_from(&mut banks, owner).kinds[&CallKind::Gate].cost, 0.25);
	assert!(banks.is_empty());
}

#[tokio::test]
async fn shutdown_between_paid_detached_calls_resumes_without_loss_or_double_counting() {
	isolated(async {
		for fold_before_shutdown in [false, true] {
			let dir = tempfile::tempdir().unwrap();
			let file = dir.path().join("session.jsonl.zst");
			let owner = uuid::Uuid::new_v4().to_string();
			let mut session = Session::new(owner.clone(), "model".into());
			session.session_file = Some(file.clone());
			session.save().unwrap();
			let (paid_tx, paid_rx) = tokio::sync::oneshot::channel();
			let task = tokio::spawn(run_background(owner.clone(), Some(file.clone()), async move {
				record_call(CallKind::Distill, 100, 20, 3, 50, 0.25);
				checkpoint().unwrap();
				paid_tx.send(()).unwrap();
				std::future::pending::<()>().await;
				record_call(CallKind::Distill, 200, 40, 6, 100, 0.5);
				checkpoint().unwrap();
				Ok(())
			}));
			paid_rx.await.unwrap();
			if fold_before_shutdown {
				session.fold_external_spend();
				assert_eq!(session.info.total_cost, 0.25);
				session.save().unwrap();
			}
			task.abort();
			assert!(task.await.unwrap_err().is_cancelled());
			// Discard process-local receipts as runtime shutdown would.
			take_completed(&owner);
			let mut resumed = crate::session::persistence::load_session(&file).unwrap();
			assert_eq!(resumed.info.total_cost, 0.25);
			assert_eq!(resumed.info.supervisor_accounting.kinds[&CallKind::Distill].calls, 1);
			assert_eq!(
				resumed.info.supervisor_accounting.kinds[&CallKind::Distill].input_tokens,
				100
			);
			assert_eq!(resumed.info.supervisor_accounting.background_runs.len(), 1);

			run_background(owner.clone(), Some(file.clone()), async {
				record_call(CallKind::Evaluate, 200, 40, 0, 100, 0.5);
				checkpoint()?;
				Ok(())
			})
			.await
			.unwrap();
			// Load the journal first, then consume the identical live receipts.
			resumed = crate::session::persistence::load_session(&file).unwrap();
			resumed.fold_external_spend();
			assert_eq!(resumed.info.total_cost, 0.75);
			assert_eq!(resumed.info.supervisor_accounting.totals().calls, 2);
			resumed.save().unwrap();
			let reloaded = crate::session::persistence::load_session(&file).unwrap();
			assert_eq!(reloaded.info.total_cost, 0.75);
			assert_eq!(reloaded.info.supervisor_accounting, resumed.info.supervisor_accounting);
		}
	})
	.await;
}

#[tokio::test]
async fn reported_gate_findings_keep_only_the_latest_run_and_survive_snapshot_merges() {
	isolated(async {
		for owner in ["gate-a", "gate-b"] {
			with_session_id(owner.to_string(), async {
				for run in ["first", "latest"] {
					record(
						CallKind::Gate,
						Usage {
							outcome: BTreeMap::from([(Outcome::Reported, 1)]),
							reported_findings: vec![format!("{owner}: {run} unseen output")],
							..Default::default()
						},
					);
				}
			})
			.await;
		}
		for owner in ["gate-a", "gate-b"] {
			let delta = take_for(owner);
			let mut merged = SupervisorAccounting::default();
			merged.add(&delta);
			merged.merge_max(&delta);
			// A snapshot without findings never erases the live ones.
			merged.merge_max(&SupervisorAccounting::default());
			let restored: SupervisorAccounting =
				serde_json::from_value(serde_json::to_value(&merged).unwrap()).unwrap();
			let gate = &restored.kinds[&CallKind::Gate];
			assert_eq!(gate.reported_findings, vec![format!("{owner}: latest unseen output")]);
			assert_eq!(gate.outcome[&Outcome::Reported], 2);
			assert!(take_for(owner).kinds.is_empty());
		}
	})
	.await;
}

#[test]
fn clean_gate_run_replaces_findings_but_unrelated_empty_usage_keeps_them() {
	let findings = Usage {
		outcome: BTreeMap::from([(Outcome::Reported, 1)]),
		reported_findings: vec!["finding 1".to_string()],
		..Default::default()
	};
	let clean_run = Usage {
		outcome: BTreeMap::from([(Outcome::Reported, 0)]),
		..Default::default()
	};
	// add: a clean latest gate run clears the previous findings.
	let mut usage = findings.clone();
	usage.add(&clean_run);
	assert!(usage.reported_findings.is_empty());
	// merge_max: same replacement, and an unrelated empty Usage never wipes findings.
	let mut usage = findings.clone();
	usage.merge_max(&clean_run);
	assert!(usage.reported_findings.is_empty());
	let mut usage = findings.clone();
	usage.merge_max(&Usage::default());
	assert_eq!(usage.reported_findings, vec!["finding 1".to_string()]);
	let mut usage = findings.clone();
	usage.add(&Usage::default());
	assert_eq!(usage.reported_findings, vec!["finding 1".to_string()]);
}
