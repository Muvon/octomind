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

//! Global animation manager — owns a single persistent indicatif ProgressBar.
//!
//! Design rules (follow indicatif idioms, not against them):
//!
//! 1. One `ProgressBar` lives across the entire working period.  Between
//!    transitions (assistant response → tool execution → tool results → next
//!    API call) we only call `set_message()` to refresh cost/context.  We do
//!    NOT destroy and recreate the bar — that was the root cause of the
//!    "shadow render" where orphan tick threads drew the spinner line AFTER
//!    `stop_current()` returned.
//!
//! 2. All non-spinner output (tool results, assistant text, errors) flows
//!    through the spinner-aware print macros defined in `src/lib.rs`.  Those
//!    macros call `with_suspended_spinner` → `ProgressBar::suspend()` which
//!    is indicatif's documented mechanism for interleaving output with a
//!    live spinner.  No races, no ghost lines.
//!
//! 3. Real teardown (`stop_current`) is reserved for genuine boundaries:
//!    user prompt imminent (suspend/resume), API error, operation cancelled,
//!    session exit.  Teardown is fully awaited (not fire-and-forgotten) so
//!    indicatif's steady-tick thread is joined before we return and no
//!    ghost tick can redraw on top of subsequent output.
//!
//! 4. Elapsed time is rendered via indicatif's `with_key` so the timer is
//!    computed inside the steady-tick thread's draw call — immune to any
//!    mutex starvation from `suspend()` under heavy output.

use crate::log_debug;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// Shared animation state for dynamic updates
#[derive(Clone)]
pub struct AnimationState {
	/// Current cost (stored as u64; multiply by 10000 for precision)
	pub cost: Arc<AtomicU64>,
	/// Current context tokens
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

	pub fn update_cost(&self, cost: f64) {
		self.cost.store((cost * 10000.0) as u64, Ordering::Relaxed);
	}

	pub fn get_cost(&self) -> f64 {
		self.cost.load(Ordering::Relaxed) as f64 / 10000.0
	}

	pub fn update_context_tokens(&self, tokens: u64) {
		self.context_tokens.store(tokens, Ordering::Relaxed);
	}

	pub fn get_context_tokens(&self) -> u64 {
		self.context_tokens.load(Ordering::Relaxed)
	}

	pub fn update_max_threshold(&self, threshold: usize) {
		self.max_threshold
			.store(threshold as u64, Ordering::Relaxed);
	}

	pub fn get_max_threshold(&self) -> usize {
		self.max_threshold.load(Ordering::Relaxed) as usize
	}
}

impl Default for AnimationState {
	fn default() -> Self {
		Self::new()
	}
}

/// Global animation manager — singleton.
pub struct AnimationManager {
	/// The single live progress bar (None = not running).
	spinner: Arc<Mutex<Option<ProgressBar>>>,
	/// Shared animation state (cost/context/threshold) updated by callers.
	state: AnimationState,
	/// Optional session-level cancellation receiver for instant Ctrl+C response.
	cancel_rx: Arc<Mutex<Option<watch::Receiver<bool>>>>,
	/// Handle for the cancellation watcher task (so we can abort it on stop).
	cancel_watcher: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
	/// Suspended flag — prevents animation from starting during user prompts.
	suspended: Arc<AtomicBool>,
}

impl AnimationManager {
	pub fn new() -> Self {
		Self {
			spinner: Arc::new(Mutex::new(None)),
			state: AnimationState::new(),
			cancel_rx: Arc::new(Mutex::new(None)),
			cancel_watcher: Arc::new(Mutex::new(None)),
			suspended: Arc::new(AtomicBool::new(false)),
		}
	}

	/// Get shared animation state for external updates.
	pub fn get_state(&self) -> AnimationState {
		self.state.clone()
	}

	/// Set cancellation receiver for instant Ctrl+C stop.
	pub fn set_cancel_receiver(&self, rx: watch::Receiver<bool>) {
		*self.cancel_rx.lock().unwrap() = Some(rx);
	}

	/// Clear cancellation receiver.
	pub fn clear_cancel_receiver(&self) {
		*self.cancel_rx.lock().unwrap() = None;
	}

	/// Suspend animation — stops current animation and blocks new ones from starting.
	/// Use this before displaying interactive user prompts.
	pub async fn suspend(&self) {
		self.suspended.store(true, Ordering::SeqCst);
		self.stop_current().await;
		log_debug!("Animation suspended — user prompt imminent");
	}

	/// Resume animation — allows animation to start again.
	pub fn resume(&self) {
		self.suspended.store(false, Ordering::SeqCst);
		log_debug!("Animation resumed");
	}

	/// Check if animation is suspended.
	pub fn is_suspended(&self) -> bool {
		self.suspended.load(Ordering::SeqCst)
	}

	/// Execute a closure while the spinner is suspended (indicatif handles
	/// draw/restore).  If no spinner is active, just runs the closure.
	///
	/// This is what the spinner-aware print macros in `src/lib.rs` call.
	pub fn with_suspended_spinner<F, R>(&self, f: F) -> R
	where
		F: FnOnce() -> R,
	{
		let spinner_guard = self.spinner.lock().unwrap();
		if let Some(ref pb) = *spinner_guard {
			pb.suspend(f)
		} else {
			drop(spinner_guard);
			f()
		}
	}

	/// Start the spinner (idempotent).  If already running, just refreshes
	/// the message from current shared state.  Respects suspended flag and
	/// output mode.
	pub async fn start_animation(&self, mode: &crate::session::output::OutputMode) {
		if self.is_suspended() {
			log_debug!("start_animation: manager suspended — skipping");
			return;
		}
		if !mode.should_show_animations() {
			return;
		}
		self.ensure_started_internal();
	}

	/// Start with explicit cost/context values.  Automatically detects
	/// interactive vs non-interactive mode.
	///
	/// In non-interactive terminal modes this prints a single static status line.
	pub async fn start_with_params(&self, cost: f64, context_tokens: u64, max_threshold: usize) {
		if self.is_suspended() {
			log_debug!("start_with_params: manager suspended — skipping");
			return;
		}

		let output_mode = crate::config::with_thread_config(|config| config.output_mode())
			.unwrap_or(crate::session::output::OutputMode::NonInteractive);

		// Non-interactive: static status line, no spinner.
		if !output_mode.should_show_animations() {
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

		self.state.update_cost(cost);
		self.state.update_context_tokens(context_tokens);
		self.state.update_max_threshold(max_threshold);

		self.ensure_started_internal();
	}

	/// Refresh the displayed cost/context message on the live spinner.
	///
	/// Cheap and allocation-light: only calls `set_message` when values
	/// actually changed since last update.  Call this from hot paths
	/// (multi-hop tool loops) instead of stop/start churn.
	pub fn update_state(&self, cost: f64, context_tokens: u64, max_threshold: usize) {
		self.state.update_cost(cost);
		self.state.update_context_tokens(context_tokens);
		self.state.update_max_threshold(max_threshold);

		let guard = self.spinner.lock().unwrap();
		if let Some(ref pb) = *guard {
			let cost_bits = self.state.cost.load(Ordering::Relaxed);
			let ctx = self.state.context_tokens.load(Ordering::Relaxed);
			let thresh = self.state.max_threshold.load(Ordering::Relaxed);
			pb.set_message(build_base_message(cost_bits, ctx, thresh));
		}
	}

	/// Set a transient phase label on the spinner (e.g. "Validating (rust)…").
	///
	/// Starts the spinner if not running (respects output mode / suspended flag),
	/// then replaces the standard "Working …" message with the given phase until
	/// `clear_phase()` is called or the spinner is stopped. Cost/context prefix
	/// is preserved so the user still sees `[$1.23|45%] Validating (rust)…`.
	pub async fn set_phase(&self, phase: &str) {
		if self.is_suspended() {
			return;
		}
		let output_mode = crate::config::with_thread_config(|c| c.output_mode())
			.unwrap_or(crate::session::output::OutputMode::NonInteractive);
		if !output_mode.should_show_animations() {
			return;
		}

		self.ensure_started_internal();

		let guard = self.spinner.lock().unwrap();
		if let Some(ref pb) = *guard {
			let cost_bits = self.state.cost.load(Ordering::Relaxed);
			let ctx = self.state.context_tokens.load(Ordering::Relaxed);
			let thresh = self.state.max_threshold.load(Ordering::Relaxed);
			pb.set_message(build_phase_message(cost_bits, ctx, thresh, phase));
		}
	}

	/// Clear the phase label and restore the standard "Working …" message.
	/// No-op if the spinner isn't running.
	pub fn clear_phase(&self) {
		let guard = self.spinner.lock().unwrap();
		if let Some(ref pb) = *guard {
			let cost_bits = self.state.cost.load(Ordering::Relaxed);
			let ctx = self.state.context_tokens.load(Ordering::Relaxed);
			let thresh = self.state.max_threshold.load(Ordering::Relaxed);
			pb.set_message(build_base_message(cost_bits, ctx, thresh));
		}
	}

	/// Create the bar if not yet running, else refresh its message.
	///
	/// This is the core: we never destroy+recreate on transitions; we only
	/// ever create once per working period and refresh the displayed values.
	fn ensure_started_internal(&self) {
		let mut guard = self.spinner.lock().unwrap();

		if let Some(ref pb) = *guard {
			// Already running — just refresh the message from current state.
			let cost_bits = self.state.cost.load(Ordering::Relaxed);
			let ctx = self.state.context_tokens.load(Ordering::Relaxed);
			let thresh = self.state.max_threshold.load(Ordering::Relaxed);
			pb.set_message(build_base_message(cost_bits, ctx, thresh));
			return;
		}

		// Create the bar.  indicatif's own steady-tick thread handles drawing.
		let pb = ProgressBar::new_spinner();
		pb.set_style(
			ProgressStyle::default_spinner()
				.with_key(
					"elapsed_custom",
					|ps: &ProgressState, w: &mut dyn std::fmt::Write| {
						let elapsed = ps.elapsed();
						if elapsed.as_secs() > 0 {
							let _ = write!(
								w,
								"({} • Ctrl+C to interrupt)",
								crate::session::chat::animation::format_elapsed_time(elapsed)
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

		let cost_bits = self.state.cost.load(Ordering::Relaxed);
		let ctx = self.state.context_tokens.load(Ordering::Relaxed);
		let thresh = self.state.max_threshold.load(Ordering::Relaxed);
		pb.set_message(build_base_message(cost_bits, ctx, thresh));
		pb.enable_steady_tick(Duration::from_millis(100));

		*guard = Some(pb.clone());
		drop(guard);

		// Spawn a small watcher for session cancellation (Ctrl+C).  It only
		// lives as long as the spinner does; stop_current aborts it.
		self.spawn_cancel_watcher(pb);
	}

	/// Spawn a lightweight watcher that clears the spinner when the
	/// session cancellation channel fires.  Event-driven, no busy loop.
	fn spawn_cancel_watcher(&self, pb: ProgressBar) {
		let cancel_rx = self.cancel_rx.lock().unwrap().clone();
		let Some(mut rx) = cancel_rx else {
			return;
		};

		let spinner_ref = self.spinner.clone();
		let cancel_watcher = self.cancel_watcher.clone();

		// Abort any prior watcher before spawning a new one.
		if let Some(prior) = cancel_watcher.lock().unwrap().take() {
			prior.abort();
		}

		let handle = tokio::spawn(async move {
			loop {
				if *rx.borrow() {
					break;
				}
				if rx.changed().await.is_err() {
					return;
				}
			}

			// Cancellation fired — clear the bar synchronously inside
			// spawn_blocking so we never block the async runtime.
			let pb_for_block = pb.clone();
			let _ = tokio::task::spawn_blocking(move || {
				pb_for_block.finish_and_clear();
				pb_for_block.disable_steady_tick();
			})
			.await;

			// Drop shared reference so subsequent print macros don't try
			// to suspend a dead bar.
			*spinner_ref.lock().unwrap() = None;
			log_debug!("Animation cancelled via session cancellation channel");
		});

		*cancel_watcher.lock().unwrap() = Some(handle);
	}

	/// Stop the current animation fully — indicatif's tick thread is joined
	/// BEFORE this function returns so no ghost draw can race with
	/// subsequent output.
	pub async fn stop_current(&self) {
		// Take the bar out of shared state first so print macros don't use it.
		let pb = self.spinner.lock().unwrap().take();

		// Abort the cancellation watcher if any.
		if let Some(handle) = self.cancel_watcher.lock().unwrap().take() {
			handle.abort();
		}

		// Clear cancel receiver (clean slate for next cycle).
		self.clear_cancel_receiver();

		let Some(pb) = pb else {
			return;
		};

		// Join indicatif's steady-tick thread in spawn_blocking.  We AWAIT
		// this so no ghost tick can draw after we return.  Bound with a
		// timeout so a hung tick thread can't block the runtime forever.
		let join = tokio::task::spawn_blocking(move || {
			pb.finish_and_clear();
			pb.disable_steady_tick();
		});

		match tokio::time::timeout(Duration::from_millis(500), join).await {
			Ok(_) => {}
			Err(_) => {
				log_debug!("stop_current: disable_steady_tick timed out — leaving detached");
			}
		}
	}

	/// True if a spinner is currently live.
	pub fn is_running(&self) -> bool {
		self.spinner.lock().unwrap().is_some()
	}
}

impl Default for AnimationManager {
	fn default() -> Self {
		Self::new()
	}
}

/// Build a spinner message: status body (plain, no embedded ANSI) + label.
/// The template's `{msg:.cyan}` directive paints the entire message cyan —
/// matching the original spinner appearance. Inline ANSI in the body would
/// locally override the cyan and break uniform line coloring, so we use the
/// plain body here and rely on glyph contrast for the bar.
fn build_spinner_message(cost_bits: u64, ctx: u64, thresh: u64, label: &str) -> String {
	let cost = cost_bits as f64 / 10000.0;
	let body = crate::session::chat::status_prefix::build_status_body_plain(cost, ctx, thresh);
	if body.is_empty() {
		label.to_string()
	} else {
		format!("{} {}", body, label)
	}
}

fn build_base_message(cost_bits: u64, ctx: u64, thresh: u64) -> String {
	build_spinner_message(cost_bits, ctx, thresh, "Working …")
}

fn build_phase_message(cost_bits: u64, ctx: u64, thresh: u64, phase: &str) -> String {
	build_spinner_message(cost_bits, ctx, thresh, phase)
}

/// Global animation manager instance.
pub static GLOBAL_ANIMATION_MANAGER: std::sync::OnceLock<AnimationManager> =
	std::sync::OnceLock::new();

/// Get global animation manager instance.
pub fn get_animation_manager() -> &'static AnimationManager {
	GLOBAL_ANIMATION_MANAGER.get_or_init(AnimationManager::new)
}
