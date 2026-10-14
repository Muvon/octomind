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
#[serial_test::serial]
async fn concurrent_sessions_fold_only_their_own_usage_once() {
	take();
	let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
	let mut tasks = Vec::new();
	for (id, cost) in [("accounting-a", 0.25), ("accounting-b", 0.75)] {
		let barrier = barrier.clone();
		tasks.push(tokio::spawn(with_session_id(id.to_string(), async move {
			let _timer = WallTimer::start(CallKind::Gate);
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
			drop(_timer);
			let mut session = Session::new(id.to_string(), "model".to_string());
			session.fold_external_spend();
			let usage = &session.info.supervisor_accounting.kinds[&CallKind::Gate];
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
				session.info.supervisor_accounting.seams[&Seam::Gate].input_tokens,
				7
			);
			let saved = session.info.supervisor_accounting.clone();
			session.fold_external_spend();
			assert_eq!(session.info.supervisor_accounting, saved);
		})));
	}
	for task in tasks {
		task.await.expect("accounting task");
	}
}

#[tokio::test]
#[serial_test::serial]
async fn nested_timers_count_blocking_union_and_background_separately() {
	take();
	with_session_id("accounting-nested".into(), async {
		let outer = WallTimer::start(CallKind::Gate);
		let inner = WallTimer::seam(Seam::Gate);
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		drop(inner);
		let first = take();
		assert!(first.seams[&Seam::Gate].blocking_wall_time_ms > 0);
		drop(outer);
		let mut usage = first;
		usage.add(&take());
		assert!(usage.blocking_wall_time_ms <= usage.totals().blocking_wall_time_ms);
		assert!(usage.blocking_wall_time_ms >= usage.kinds[&CallKind::Gate].blocking_wall_time_ms);
		background(async {
			tokio::spawn(inherit(with_session_id(
				"accounting-nested".into(),
				async {
					let _timer = WallTimer::start(CallKind::Distill);
					let _seam = WallTimer::seam(Seam::Distill);
					tokio::time::sleep(std::time::Duration::from_millis(10)).await;
				},
			)))
			.await
			.expect("awaited tool inherits background mode");
		})
		.await;
		let background = take();
		assert_eq!(background.blocking_wall_time_ms, 0);
		assert!(background.kinds[&CallKind::Distill].background_wall_time_ms > 0);
		assert!(background.seams[&Seam::Distill].background_wall_time_ms > 0);
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn cancelled_mechanic_records_wall_time_without_usage() {
	take();
	with_session_id("accounting-cancelled".into(), async {
		let result = tokio::time::timeout(std::time::Duration::from_millis(10), async {
			let _timer = WallTimer::start(CallKind::Resolve);
			std::future::pending::<()>().await;
		})
		.await;
		assert!(result.is_err());
		let usage = take();
		assert!(usage.kinds[&CallKind::Resolve].blocking_wall_time_ms > 0);
		assert_eq!(usage.kinds[&CallKind::Resolve].calls, 0);
	})
	.await;
}

#[tokio::test]
#[serial_test::serial]
async fn unowned_usage_goes_to_next_fold() {
	take();
	record(
		CallKind::Recall,
		Usage {
			calls: 1,
			cost: 0.5,
			..Default::default()
		},
	);
	with_session_id("accounting-unowned".into(), async {
		assert_eq!(take().kinds[&CallKind::Recall].cost, 0.5);
		assert_eq!(take(), SupervisorAccounting::default());
	})
	.await;
}
