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

//! Lifecycle tests on a private AnimationManager instance (not the global
//! singleton), so nothing here races the rest of the suite. In a non-tty
//! test environment indicatif renders to a hidden target — the state
//! machine still runs for real.

use super::*;

#[test]
fn test_animation_state_roundtrip() {
	let state = AnimationState::new();
	assert_eq!(state.get_cost(), 0.0);

	state.update_cost(1.2345);
	assert!((state.get_cost() - 1.2345).abs() < 0.001);
	state.update_context_tokens(4321);
	assert_eq!(state.get_context_tokens(), 4321);
	state.update_max_threshold(100_000);
	assert_eq!(state.get_max_threshold(), 100_000);
}

#[tokio::test]
async fn test_spinner_lifecycle() {
	// In a non-tty test environment the start path prints a static status
	// line instead of creating a live spinner, so the lifecycle contract
	// here is "state churn never panics and stop always lands on
	// not-running" — not spinner visibility.
	let manager = AnimationManager::new();
	assert!(!manager.is_running());

	manager.start_with_params(0.5, 1000, 10_000).await;
	manager.set_phase("Validating …").await;
	manager.update_state(0.6, 2000, 10_000);
	manager.clear_phase();
	manager.set_label(Some("Working (agent)".to_string()));
	manager.set_label(None);

	manager.stop_current().await;
	assert!(!manager.is_running());
}

#[tokio::test]
async fn test_suspend_blocks_start() {
	let manager = AnimationManager::new();
	manager.suspend().await;
	assert!(manager.is_suspended());

	// Suspended manager refuses to start — a spinner over a user prompt
	// would eat the input line.
	manager.start_with_params(0.0, 0, 0).await;
	assert!(!manager.is_running());
	manager.set_phase("ignored while suspended").await;
	assert!(!manager.is_running());

	manager.resume();
	assert!(!manager.is_suspended());
	manager.start_with_params(0.0, 0, 0).await;
	manager.stop_current().await;
	assert!(!manager.is_running());
}

#[test]
fn test_with_suspended_spinner_returns_closure_value() {
	let manager = AnimationManager::new();
	let value = manager.with_suspended_spinner(|| 41 + 1);
	assert_eq!(value, 42);
}

#[test]
fn test_set_phase_if_running_needs_a_live_spinner() {
	// The MCP notification drain calls this from a spawned task; without a
	// spinner on screen it must stay silent rather than conjure one.
	let manager = AnimationManager::new();
	manager.set_phase_if_running("[octofs] command still running");
	assert!(!manager.is_running());
	assert!(manager.phase.lock().unwrap().is_none());
}

#[test]
fn stop_clears_spinner_before_yielding_to_tick_cleanup() {
	use std::future::Future;
	use std::task::Poll;

	let runtime = tokio::runtime::Builder::new_current_thread()
		.enable_time()
		.max_blocking_threads(1)
		.build()
		.unwrap();
	runtime.block_on(async {
		let manager = AnimationManager::new();
		let pb = ProgressBar::hidden();
		pb.enable_steady_tick(Duration::from_millis(100));
		manager.install_test_spinner(Some(pb.clone()));

		// Hold the blocking worker so cleanup cannot race this observation.
		let (release_tx, release_rx) = std::sync::mpsc::channel();
		let (started_tx, started_rx) = tokio::sync::oneshot::channel();
		let blocker = tokio::task::spawn_blocking(move || {
			started_tx.send(()).unwrap();
			release_rx.recv().unwrap();
		});
		started_rx.await.unwrap();

		let mut stop = std::pin::pin!(manager.stop_current());
		let pending = std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending())).await;
		let finished_before_yield = pb.is_finished();
		let running_before_yield = manager.is_running();
		release_tx.send(()).unwrap();
		blocker.await.unwrap();
		if pending {
			stop.await;
		}

		assert!(
			finished_before_yield,
			"the spinner must be cleared before other output can run"
		);
		assert!(!running_before_yield);
	});
}
