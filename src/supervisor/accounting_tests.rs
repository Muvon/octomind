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
		banks
			.lock()
			.unwrap()
			.insert(Some(id.into()), Arc::clone(&bank));
		let barrier = Arc::clone(&barrier);
		tasks.push(tokio::spawn(with_session_id(
			id.to_owned(),
			DETACHED_USAGE.scope(bank, async move {
				let timer = WallTimer::start(CallKind::Gate);
				super::super::stats::record_call(CallKind::Gate, 100, 20, 5, 120, cost);
				record_seam(
					Seam::Gate,
					Usage {
						calls: 1,
						input_tokens: 7,
						..Default::default()
					},
				);
				barrier.wait().await;
				drop(timer);
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
				assert_eq!(accounting.seams[&Seam::Gate].input_tokens, 7);
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

#[tokio::test]
async fn nested_timers_count_blocking_union_and_background_separately() {
	isolated(async {
		let bank = Arc::new(Mutex::new(Bank::default()));
		DETACHED_USAGE
			.scope(Arc::clone(&bank), async {
				let outer = WallTimer::start(CallKind::Gate);
				let inner = WallTimer::seam(Seam::Gate);
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
				drop(inner);
				let mut usage = bank.lock().unwrap().take();
				assert!(usage.seams[&Seam::Gate].blocking_wall_time_ms > 0);
				drop(outer);
				usage.add(&bank.lock().unwrap().take());
				assert!(usage.blocking_wall_time_ms <= usage.totals().blocking_wall_time_ms);
				assert!(
					usage.blocking_wall_time_ms
						>= usage.kinds[&CallKind::Gate].blocking_wall_time_ms
				);
				background(async {
					tokio::spawn(TEST_PENDING.scope(
						TEST_PENDING.with(Arc::clone),
						inherit(async {
							let _timer = WallTimer::start(CallKind::Distill);
							let _seam = WallTimer::seam(Seam::Distill);
							tokio::time::sleep(std::time::Duration::from_millis(10)).await;
						}),
					))
					.await
					.unwrap();
				})
				.await;
				let usage = bank.lock().unwrap().take();
				assert_eq!(usage.blocking_wall_time_ms, 0);
				assert!(usage.kinds[&CallKind::Distill].background_wall_time_ms > 0);
				assert!(usage.seams[&Seam::Distill].background_wall_time_ms > 0);
			})
			.await;
	})
	.await;
}

#[tokio::test]
async fn cancelled_mechanic_records_wall_time_without_usage() {
	isolated(async {
		let bank = Arc::new(Mutex::new(Bank::default()));
		let result = tokio::time::timeout(
			std::time::Duration::from_millis(10),
			DETACHED_USAGE.scope(Arc::clone(&bank), async {
				let _timer = WallTimer::start(CallKind::Resolve);
				std::future::pending::<()>().await;
			}),
		)
		.await;
		assert!(result.is_err());
		let usage = bank.lock().unwrap().take();
		assert!(usage.kinds[&CallKind::Resolve].blocking_wall_time_ms > 0);
		assert_eq!(usage.kinds[&CallKind::Resolve].calls, 0);
	})
	.await;
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
			let _timer = WallTimer::start(CallKind::Evolution);
			super::super::stats::record_call(CallKind::Evolution, 123, 45, 6, 700, 0.25);
			recorded_tx.send(()).unwrap();
			finish_rx.await.unwrap();
			Ok(())
		}));
		recorded_rx.await.unwrap();
		let mut receiver = Session::new(other.clone(), "model".into());
		for _ in 0..10 {
			receiver.fold_external_spend();
			assert!(receiver
				.info
				.supervisor_accounting
				.background_runs
				.is_empty());
			assert!(
				take_completed(&owner).is_empty(),
				"a running job cannot be live-folded"
			);
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
		let task = tokio::spawn(run_background(
			owner.clone(),
			Some(file.clone()),
			async move {
				let _timer = WallTimer::start(CallKind::Distill);
				super::super::stats::record_call(CallKind::Distill, 100, 20, 3, 50, 0.25);
				ready_tx.send(()).unwrap();
				finish_rx.await.unwrap();
				Ok(())
			},
		));
		ready_rx.await.unwrap();
		session.fold_external_spend();
		session.save().unwrap();
		finish_tx.send(()).unwrap();
		task.await.unwrap().unwrap();
		let mut resumed = crate::session::persistence::load_session(&file).unwrap();
		assert_eq!(
			resumed.info.supervisor_accounting.kinds[&CallKind::Distill].calls,
			1
		);
		let saved = resumed.info.supervisor_accounting.clone();
		let cost = resumed.info.total_cost;
		resumed.fold_external_spend();
		assert_eq!(
			resumed.info.supervisor_accounting.background_runs,
			saved.background_runs
		);
		assert_eq!(
			resumed.info.supervisor_accounting.kinds[&CallKind::Distill].calls,
			1
		);
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
	assert_eq!(
		take_from(&mut banks, owner.clone()),
		SupervisorAccounting::default()
	);
	bank.lock().unwrap().usage.kinds.insert(
		CallKind::Gate,
		Usage {
			calls: 1,
			cost: 0.25,
			..Default::default()
		},
	);
	drop(bank);
	assert_eq!(
		take_from(&mut banks, owner).kinds[&CallKind::Gate].cost,
		0.25
	);
	assert!(banks.is_empty());
}

#[test]
fn mechanic_and_total_wall_time_are_unions_of_nested_and_parallel_intervals() {
	let start = Instant::now();
	let mut clock = Clock::default();
	let mut usage = SupervisorAccounting::default();
	clock.enter(Mechanic::Gate, false, start);
	clock.tick(start + std::time::Duration::from_millis(5), &mut usage);
	clock.enter(
		Mechanic::Gate,
		false,
		start + std::time::Duration::from_millis(5),
	);
	clock.tick(start + std::time::Duration::from_millis(10), &mut usage);
	clock.leave(Mechanic::Gate, false);
	clock.tick(start + std::time::Duration::from_millis(20), &mut usage);
	clock.leave(Mechanic::Gate, false);
	assert_eq!(usage.blocking_wall_time_ms, 20);
	assert_eq!(usage.kinds[&CallKind::Gate].blocking_wall_time_ms, 20);
	let mut a = SupervisorAccounting::default();
	let mut b = SupervisorAccounting::default();
	clock.enter(Mechanic::Distill, true, start);
	clock.tick(start + std::time::Duration::from_millis(5), &mut b);
	clock.enter(
		Mechanic::Distill,
		true,
		start + std::time::Duration::from_millis(5),
	);
	clock.tick(start + std::time::Duration::from_millis(10), &mut a);
	clock.leave(Mechanic::Distill, true);
	clock.tick(start + std::time::Duration::from_millis(20), &mut b);
	clock.leave(Mechanic::Distill, true);
	a.add(&b);
	assert_eq!(a.background_wall_time_ms, Some(20));
	assert_eq!(a.kinds[&CallKind::Distill].background_wall_time_ms, 20);
}

#[tokio::test]
async fn outcome_counters_are_isolated_between_sessions_and_survive_fold() {
	isolated(async {
		let barrier = tokio::sync::Barrier::new(2);
		let a = with_session_id("outcome-a".into(), async {
			super::super::stats::gate_run();
			super::super::stats::gate_fail();
			record_outcome(Mechanic::Gate, Outcome::RefutedGaps, 2);
			record_outcome(Mechanic::Gate, Outcome::Applied, 1);
			barrier.wait().await;
			let mut session = Session::new("outcome-a".into(), "model".into());
			session.fold_external_spend();
			let outcome = &session.info.supervisor_accounting.kinds[&CallKind::Gate].outcome;
			assert_eq!(outcome[&Outcome::Runs], 1);
			assert_eq!(outcome[&Outcome::Fail], 1);
			assert_eq!(outcome[&Outcome::RefutedGaps], 2);
			assert!(!outcome.contains_key(&Outcome::Pass));
		});
		let b = with_session_id("outcome-b".into(), async {
			super::super::stats::gate_run();
			super::super::stats::gate_pass();
			super::super::stats::condensed(2, 5_700);
			barrier.wait().await;
			let mut session = Session::new("outcome-b".into(), "model".into());
			session.fold_external_spend();
			let outcome = &session.info.supervisor_accounting.kinds[&CallKind::Gate].outcome;
			assert_eq!(outcome[&Outcome::Pass], 1);
			assert!(!outcome.contains_key(&Outcome::RefutedGaps));
			assert_eq!(
				session.info.supervisor_accounting.kinds[&CallKind::Condense].outcome
					[&Outcome::SavedTokens],
				5_700
			);
		});
		tokio::join!(a, b);
	})
	.await;
}
