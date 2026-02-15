// Copyright 2025 Muvon Un Limited
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

//! Global animation manager - ensures only one animation runs at a time
//!
//! This module provides a centralized animation management system that:
//! - Ensures only one animation runs at a time (prevents overlapping animations)
//! - Dynamically updates cost and context values in real-time
//! - Provides clean cancellation and cleanup
//! - Prevents animation stuck bugs

use crate::log_debug;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
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
	/// Cancellation flag for current animation
	cancel_flag: Arc<AtomicBool>,
	/// Shared animation state for dynamic updates
	state: AnimationState,
}

impl AnimationManager {
	/// Create new animation manager
	pub fn new() -> Self {
		Self {
			current_task: Arc::new(std::sync::Mutex::new(None)),
			cancel_flag: Arc::new(AtomicBool::new(false)),
			state: AnimationState::new(),
		}
	}

	/// Get shared animation state for external updates
	pub fn get_state(&self) -> AnimationState {
		self.state.clone()
	}

	/// Stop current animation (if any)
	pub async fn stop_current(&self) {
		// Set cancellation flag
		self.cancel_flag.store(true, Ordering::SeqCst);

		// Wait for current task to complete
		let task = {
			let mut guard = self.current_task.lock().unwrap();
			guard.take()
		};

		if let Some(task) = task {
			log_debug!("Stopping current animation...");
			let _ = task.await;
			log_debug!("Animation stopped");
		}

		// Reset cancellation flag for next animation
		self.cancel_flag.store(false, Ordering::SeqCst);
	}

	/// Start new animation (stops any existing animation first)
	///
	/// This ensures only one animation runs at a time, preventing:
	/// - Overlapping animations
	/// - Animation stuck bugs
	/// - Stale cost/context values
	///
	/// **Pro-level feature**: Dynamically reads live cost/context from shared state
	/// during animation loop for real-time updates during long operations.
	pub async fn start_animation(&self, mode: &crate::session::output::OutputMode) {
		// Stop any existing animation first
		self.stop_current().await;

		// Don't show animation in non-interactive modes
		if mode.should_suppress_cli_output() {
			return;
		}

		// Clone references for animation task
		let cancel_flag = self.cancel_flag.clone();
		let current_task = self.current_task.clone();
		let state = self.state.clone();

		// Spawn new animation task with dynamic updates
		let task = tokio::spawn(async move {
			// Animation loop with truly dynamic cost/context updates
			let mut spinner: Option<indicatif::ProgressBar> = None;
			let start_time = std::time::Instant::now();

			while !cancel_flag.load(Ordering::SeqCst) {
				// Read live cost/context from shared state (dynamic updates!)
				let current_cost = state.get_cost();
				let current_context_tokens = state.get_context_tokens();
				let max_threshold = state.get_max_threshold();

				// Calculate dynamic base message with live cost/context
				let base_message = if current_cost > 0.0 && max_threshold > 0 {
					let percentage =
						(current_context_tokens as f64 / max_threshold as f64 * 100.0).min(100.0);
					format!("[${:.2}|{:.1}%] Working …", current_cost, percentage)
				} else if current_cost > 0.0 {
					format!("[${:.2}|∞] Working …", current_cost)
				} else {
					"Working …".to_string()
				};

				// Create spinner on first iteration
				if spinner.is_none() {
					use indicatif::{ProgressBar, ProgressStyle};
					use std::time::Duration;

					let s = ProgressBar::new_spinner();
					s.set_style(
						ProgressStyle::default_spinner()
							.template(" {spinner:.cyan} {msg:.cyan}")
							.unwrap()
							.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
					);
					s.set_message(base_message.clone());
					s.enable_steady_tick(Duration::from_millis(50));
					spinner = Some(s);
				}

				// Update message with elapsed time and dynamic cost/context
				if let Some(ref s) = spinner {
					let elapsed = start_time.elapsed();
					let elapsed_secs = elapsed.as_secs();
					let message = if elapsed_secs > 0 {
						use colored::Colorize;
						let time_and_hint = format!(
							"({} • Ctrl+C to interrupt)",
							crate::session::chat::animation::format_elapsed_time(elapsed)
						);
						format!("{} {}", base_message, time_and_hint.dimmed())
					} else {
						use colored::Colorize;
						format!("{} {}", base_message, "(Ctrl+C to interrupt)".dimmed())
					};
					s.set_message(message);
				}

				// Sleep before next check
				tokio::time::sleep(Duration::from_millis(100)).await;
			}

			// Clean up spinner when done
			if let Some(s) = spinner {
				s.finish_and_clear();
			}

			// Ensure terminal is flushed
			use std::io::Write;
			let _ = std::io::stdout().flush();
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

/// Global animation manager instance
static GLOBAL_ANIMATION_MANAGER: std::sync::OnceLock<AnimationManager> = std::sync::OnceLock::new();

/// Get global animation manager instance
pub fn get_animation_manager() -> &'static AnimationManager {
	GLOBAL_ANIMATION_MANAGER.get_or_init(AnimationManager::new)
}
