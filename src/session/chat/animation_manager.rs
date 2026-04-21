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

//! Global animation manager — ensures only one animation runs at a time.
//!
//! Elapsed time is rendered via indicatif's `with_key` so the timer is computed
//! inside the steady-tick thread's draw call.  This makes it immune to the
//! `Mutex<BarState>` starvation that occurs when `suspend()` (called by every
//! `println!`) holds the lock during terminal I/O — the tick thread already owns
//! the lock when it draws, so the timer always advances.

use crate::log_debug;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

/// Shared animation state for dynamic updates
#[derive(Clone)]
pub struct AnimationState {
	/// Current cost (updated dynamically)
	pub cost: Arc<AtomicU64>, // Store as u64 (multiply by 10000 for precision)
	/// Current context tokens (updated dynamically)
	pub context_tokens: Arc<AtomicU64>,
	/// Max threshold for percentage calculation
	pub max_threshold: Arc<AtomicU64>,
}

impl AnimationState {
	pub fn new() -> Self {
		Self {
			cost: Arc::new(AtomicU64::new(0)),
			context_tokens: Arc::new(AtomicU64::new(0)),
			max_threshold: Arc::new(AtomicU64::new(0)),
		}
	}

	/// Update cost (converts f64 to u64 with 4 decimal precision)
	pub fn update_cost(&self, cost: f64) {
		self.cost.store((cost * 10000.0) as u64, Ordering::Relaxed);
	}

	/// Get cost (converts u64 back to f64)
	pub fn get_cost(&self) -> f64 {
		self.cost.load(Ordering::Relaxed) as f64 / 10000.0
	}

	/// Update context tokens
	pub fn update_context_tokens(&self, tokens: u64) {
		self.context_tokens.store(tokens, Ordering::Relaxed);
	}

	/// Get context tokens
	pub fn get_context_tokens(&self) -> u64 {
		self.context_tokens.load(Ordering::Relaxed)
	}

	/// Update max threshold
	pub fn update_max_threshold(&self, threshold: usize) {
		self.max_threshold
			.store(threshold as u64, Ordering::Relaxed);
	}

	/// Get max threshold
	pub fn get_max_threshold(&self) -> usize {
		self.max_threshold.load(Ordering::Relaxed) as usize
	}
}

impl Default for AnimationState {
	fn default() -> Self {
		Self::new()
	}
}

/// Global animation manager - singleton pattern
pub struct AnimationManager {
	/// Current animation task (if any)
	current_task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
	/// Notify for the *current* animation task — replaced with a fresh one each start
	/// so a leftover notification from stop_current() never kills the next animation
	cancel_notify: Arc<std::sync::Mutex<Arc<Notify>>>,
	/// Shared animation state for dynamic updates
	state: AnimationState,
	/// Optional cancellation receiver from session (for instant Ctrl+C response)
	cancel_rx: Arc<std::sync::Mutex<Option<watch::Receiver<bool>>>>,
	/// Suspended flag - prevents animation from starting during user prompts
	suspended: Arc<AtomicBool>,
	/// Shared spinner reference for suspend/resume operations
	spinner: Arc<std::sync::Mutex<Option<indicatif::ProgressBar>>>,
}

impl AnimationManager {
	/// Create new animation manager
	pub fn new() -> Self {
		Self {
			current_task: Arc::new(std::sync::Mutex::new(None)),
			cancel_notify: Arc::new(std::sync::Mutex::new(Arc::new(Notify::new()))),
			state: AnimationState::new(),
			cancel_rx: Arc::new(std::sync::Mutex::new(None)),
			suspended: Arc::new(AtomicBool::new(false)),
			spinner: Arc::new(std::sync::Mutex::new(None)),
		}
	}

	/// Get shared animation state for external updates
	pub fn get_state(&self) -> AnimationState {
		self.state.clone()
	}

	/// Set cancellation receiver from session (for instant Ctrl+C response)
	/// This allows the animation to respond immediately to Ctrl+C without waiting for stop_current()
	pub fn set_cancel_receiver(&self, rx: watch::Receiver<bool>) {
		*self.cancel_rx.lock().unwrap() = Some(rx);
	}

	/// Clear cancellation receiver (call when animation stops)
	/// Suspend animation - stops current animation and prevents new ones from starting
	/// Use this before displaying user prompts to prevent animation from covering the prompt
	pub async fn suspend(&self) {
		// Set suspended flag FIRST to prevent any race conditions
		self.suspended.store(true, Ordering::SeqCst);
		// Then stop current animation
		self.stop_current().await;
		log_debug!("Animation suspended - user prompt imminent");
	}

	/// Resume animation - allows animation to start again
	/// Call this after user input is complete
	pub fn resume(&self) {
		self.suspended.store(false, Ordering::SeqCst);
		log_debug!("Animation resumed");
	}

	/// Check if animation is suspended
	pub fn is_suspended(&self) -> bool {
		self.suspended.load(Ordering::SeqCst)
	}

	/// Execute a function while temporarily suspending the spinner
	/// This prevents output from interfering with the animation
	/// If no spinner is active, just executes the function normally
	pub fn with_suspended_spinner<F, R>(&self, f: F) -> R
	where
		F: FnOnce() -> R,
	{
		let spinner_guard = self.spinner.lock().unwrap();
		if let Some(ref spinner) = *spinner_guard {
			// Spinner is active - use indicatif's suspend to hide it temporarily
			spinner.suspend(f)
		} else {
			// No spinner active - just execute normally
			drop(spinner_guard);
			f()
		}
	}

	pub fn clear_cancel_receiver(&self) {
		*self.cancel_rx.lock().unwrap() = None;
	}

	/// Stop current animation (if any)
	pub async fn stop_current(&self) {
		// Wake the animation task instantly — zero CPU, no busy-poll
		self.cancel_notify.lock().unwrap().notify_one();

		// Clear the cancellation receiver
		self.clear_cancel_receiver();

		// Wait for task to finish gracefully (cleanup will run properly)
		let task = {
			let mut guard = self.current_task.lock().unwrap();
			guard.take()
		};

		if let Some(task) = task {
			// Wait for graceful shutdown with timeout — never block Ctrl+C forever
			// If indicatif's disable_steady_tick() hangs (thread deadlock), abort the task
			// to prevent leaked spawn_blocking threads from saturating the thread pool
			match tokio::time::timeout(Duration::from_millis(500), task).await {
				Ok(_) => {}
				Err(_) => {
					log_debug!("Animation cleanup timed out — aborting task");
					// The task is detached on drop. The spawn_blocking inside it will
					// eventually complete or be cleaned up when the runtime shuts down.
				}
			}
		}
	}
	/// Start new animation (stops any existing animation first).
	///
	/// Ensures only one animation runs at a time.  Cost/context values are read
	/// from shared atomics; elapsed time is computed at draw time by indicatif's
	/// steady-tick thread via a `with_key` template, so the timer never freezes
	/// even under heavy mutex contention from `suspend()`/`println!`.
	pub async fn start_animation(&self, mode: &crate::session::output::OutputMode) {
		// Check if suspended - don't start animation during user prompts
		if self.is_suspended() {
			log_debug!("Animation start requested but manager is suspended (user prompt active)");
			return;
		}

		// Stop any existing animation first
		self.stop_current().await;

		// Only show animation in interactive mode
		if !mode.should_show_animations() {
			return;
		}

		self.start_internal().await;
	}

	/// Start animation with explicit cost/context values.
	/// Automatically detects interactive vs non-interactive mode.
	pub async fn start_with_params(&self, cost: f64, context_tokens: u64, max_threshold: usize) {
		// Stop any existing animation first
		self.stop_current().await;

		// Resolve output mode from thread config
		let output_mode = crate::config::with_thread_config(|config| config.output_mode())
			.unwrap_or(crate::session::output::OutputMode::NonInteractive);

		// Only show animated spinner in interactive mode
		if !output_mode.should_show_animations() {
			// Show static line for non-interactive terminal modes (not jsonl/websocket)
			if output_mode.is_terminal_mode() {
				if cost > 0.0 {
					println!(
						" ── cost: ${:.5} ────────────────────────────────────────",
						cost
					);
				} else if max_threshold > 0 {
					let percentage =
						(context_tokens as f64 / max_threshold as f64 * 100.0).min(100.0);
					println!(
						" ── context: {:.1}% ────────────────────────────────────────",
						percentage
					);
				}
			}
			return;
		}

		// Update state with provided values
		self.state.update_cost(cost);
		self.state.update_context_tokens(context_tokens);
		self.state.update_max_threshold(max_threshold);

		self.start_internal().await;
	}

	/// Internal animation start logic
	async fn start_internal(&self) {
		// Create a FRESH Notify for this animation cycle — prevents a leftover
		// notify_one() from stop_current() firing immediately on the new task
		let cancel_notify = Arc::new(Notify::new());
		*self.cancel_notify.lock().unwrap() = cancel_notify.clone();

		// Clone references for animation task

		let current_task = self.current_task.clone();
		let state = self.state.clone();
		let cancel_rx = self.cancel_rx.lock().unwrap().clone();
		let spinner_ref = self.spinner.clone();

		let task = tokio::spawn(async move {
			let mut spinner: Option<indicatif::ProgressBar> = None;

			// Track previous cost/context to only call set_message when values change.
			// set_message() contends with indicatif's internal BarState mutex (held by
			// suspend() during every println). Calling it only on change reduces
			// contention from 10/sec to ~1 per API response, preventing the animation
			// task from starving on the mutex and freezing the timer / blocking Ctrl+C.
			let mut prev_cost_bits: u64 = 0;
			let mut prev_context: u64 = 0;
			let mut prev_threshold: u64 = 0;

			'animation: loop {
				// Check session cancellation receiver if available (INSTANT Ctrl+C response)
				if let Some(ref rx) = cancel_rx {
					if *rx.borrow() {
						break 'animation;
					}
				}

				// Read live cost/context from shared atomics
				let cost_bits = state.cost.load(Ordering::Relaxed);
				let ctx = state.context_tokens.load(Ordering::Relaxed);
				let thresh = state.max_threshold.load(Ordering::Relaxed);
				let values_changed =
					cost_bits != prev_cost_bits || ctx != prev_context || thresh != prev_threshold;

				// Create spinner on first iteration
				if spinner.is_none() {
					use indicatif::{ProgressBar, ProgressState, ProgressStyle};
					use std::fmt::Write as FmtWrite;
					use std::time::Duration;

					let s = ProgressBar::new_spinner();
					s.set_style(
						ProgressStyle::default_spinner()
							// Elapsed time is computed AT DRAW TIME by indicatif's steady
							// tick thread — the one thread that always gets the BarState
							// lock. This makes the timer immune to mutex starvation.
							.with_key(
								"elapsed_custom",
								|ps: &ProgressState, w: &mut dyn FmtWrite| {
									let elapsed = ps.elapsed();
									if elapsed.as_secs() > 0 {
										let _ = write!(
											w,
											"({} • Ctrl+C to interrupt)",
											crate::session::chat::animation::format_elapsed_time(
												elapsed
											)
										);
									} else {
										let _ = write!(w, "(Ctrl+C to interrupt)");
									}
								},
							)
							.template(" {spinner:.cyan} {msg:.cyan} {elapsed_custom:.cyan.dim}")
							.unwrap()
							.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
					);

					let base_message = build_base_message(cost_bits, ctx, thresh);
					s.set_message(base_message);
					s.enable_steady_tick(Duration::from_millis(50));

					// Store spinner reference for suspend operations
					*spinner_ref.lock().unwrap() = Some(s.clone());
					spinner = Some(s);

					prev_cost_bits = cost_bits;
					prev_context = ctx;
					prev_threshold = thresh;
				} else if values_changed {
					// Only acquire indicatif's lock when cost/context actually changed
					let base_message = build_base_message(cost_bits, ctx, thresh);
					if let Some(ref s) = spinner {
						s.set_message(base_message);
					}
					prev_cost_bits = cost_bits;
					prev_context = ctx;
					prev_threshold = thresh;
				}

				tokio::select! {
					_ = tokio::time::sleep(Duration::from_millis(100)) => {}
					// INSTANT cancellation from session's watch channel (Ctrl+C)
					_ = async {
						if let Some(ref rx) = cancel_rx {
							let mut rx_clone = rx.clone();
							while !*rx_clone.borrow() {
								if rx_clone.changed().await.is_err() {
									break;
								}
							}
						} else {
							std::future::pending::<()>().await;
						}
					} => {
						log_debug!("Animation cancelled via session cancellation channel");
						break 'animation;
					}
					// INSTANT cancellation from stop_current() — zero CPU, event-driven
					_ = cancel_notify.notified() => {
						log_debug!("Animation cancelled via stop_current()");
						break 'animation;
					}
				}
			}

			// Clear shared spinner reference first so no other code can use it during cleanup.
			*spinner_ref.lock().unwrap() = None;

			if let Some(s) = spinner {
				// `finish_and_clear()` is non-blocking — call it immediately so the spinner
				// line is erased from the terminal BEFORE stop_current() returns and any
				// subsequent println! output is written.  Without this, the fire-and-forget
				// spawn_blocking races with the next println! and clears a line of real output.
				s.finish_and_clear();

				// `disable_steady_tick()` joins indicatif's internal tick thread — it IS a
				// blocking call and must NOT run on the async executor.  Fire-and-forget it
				// so the animation task completes instantly and stop_current() never hangs.
				drop(tokio::task::spawn_blocking(move || {
					s.disable_steady_tick();
				}));
				// Intentionally NOT awaited — prevents stop_current() timeout cascade
			}
		});

		// Store task reference
		*current_task.lock().unwrap() = Some(task);
	}

	/// Check if animation is currently running
	pub fn is_running(&self) -> bool {
		self.current_task.lock().unwrap().is_some()
	}
}

impl Default for AnimationManager {
	fn default() -> Self {
		Self::new()
	}
}

/// Build the cost/context prefix shown before "Working …".
fn build_base_message(cost_bits: u64, ctx: u64, thresh: u64) -> String {
	let cost = cost_bits as f64 / 10000.0;
	if cost > 0.0 && thresh > 0 {
		let pct = (ctx as f64 / thresh as f64 * 100.0).min(100.0);
		format!("[${:.2}|{:.1}%] Working …", cost, pct)
	} else if cost > 0.0 {
		format!("[${:.2}|∞] Working …", cost)
	} else if thresh > 0 {
		let pct = (ctx as f64 / thresh as f64 * 100.0).min(100.0);
		format!("[{:.1}%] Working …", pct)
	} else {
		"Working …".to_string()
	}
}

/// Global animation manager instance
/// Made public so terminal_output module can access it for spinner suspension
pub static GLOBAL_ANIMATION_MANAGER: std::sync::OnceLock<AnimationManager> =
	std::sync::OnceLock::new();

/// Get global animation manager instance
pub fn get_animation_manager() -> &'static AnimationManager {
	GLOBAL_ANIMATION_MANAGER.get_or_init(AnimationManager::new)
}
