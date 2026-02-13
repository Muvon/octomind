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

// Animation module for loading indicators using indicatif

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::time::Duration;

// Format elapsed time in human-readable format
fn format_elapsed_time(elapsed: Duration) -> String {
	let total_secs = elapsed.as_secs();

	if total_secs < 60 {
		// Less than 1 minute: show seconds
		format!("{}s", total_secs)
	} else if total_secs < 3600 {
		// Less than 1 hour: show minutes and seconds
		let mins = total_secs / 60;
		let secs = total_secs % 60;
		if secs > 0 {
			format!("{}m {}s", mins, secs)
		} else {
			format!("{}m", mins)
		}
	} else {
		// 1 hour or more: show hours, minutes, and seconds
		let hours = total_secs / 3600;
		let mins = (total_secs % 3600) / 60;
		let secs = total_secs % 60;
		if mins > 0 && secs > 0 {
			format!("{}h {}m {}s", hours, mins, secs)
		} else if mins > 0 {
			format!("{}h {}m", hours, mins)
		} else if secs > 0 {
			format!("{}h {}s", hours, secs)
		} else {
			format!("{}h", hours)
		}
	}
}

// Show loading animation while waiting for response (interactive mode)
pub async fn show_loading_animation(
	cancel_flag: Arc<AtomicBool>,
	cost: f64,
	current_context_tokens: u64,
	max_session_tokens_threshold: usize,
) -> Result<()> {
	// Create a spinner with cost-aware message in prompt format
	let spinner = ProgressBar::new_spinner();

	// Set a clean style with spinner and elapsed time
	spinner.set_style(
		ProgressStyle::default_spinner()
			.template(" {spinner:.cyan} {msg:.cyan}")
			.unwrap()
			.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
	);

	// Start time tracking
	let start_time = std::time::Instant::now();

	// Format initial message with cost and context percentage
	let base_message = if cost > 0.0 {
		if max_session_tokens_threshold > 0 {
			let percentage = (current_context_tokens as f64 / max_session_tokens_threshold as f64
				* 100.0)
				.min(100.0);
			format!("[${:.2}|{:.1}%] Working …", cost, percentage)
		} else {
			format!("[${:.2}|∞] Working …", cost)
		}
	} else {
		"Working …".to_string()
	};
	spinner.set_message(base_message.clone());
	spinner.enable_steady_tick(Duration::from_millis(50));

	// Wait for cancellation with faster polling and update elapsed time
	while !cancel_flag.load(Ordering::SeqCst) {
		tokio::time::sleep(Duration::from_millis(100)).await;

		// Update message with elapsed time every 100ms
		let elapsed = start_time.elapsed();
		let elapsed_secs = elapsed.as_secs();
		let message = if elapsed_secs > 0 {
			use colored::Colorize;
			let time_and_hint = format!("({} • Ctrl+C to interrupt)", format_elapsed_time(elapsed));
			format!("{} {}", base_message, time_and_hint.dimmed())
		} else {
			use colored::Colorize;
			format!("{} {}", base_message, "(Ctrl+C to interrupt)".dimmed())
		};
		spinner.set_message(message);
	}

	// Clean finish - removes the spinner completely
	spinner.finish_and_clear();

	// CRITICAL: Ensure terminal is fully flushed before returning
	// This prevents race conditions where new animations start before old ones are cleared
	use std::io::Write;
	let _ = std::io::stdout().flush();

	// Small delay to ensure terminal has processed the clear
	tokio::time::sleep(Duration::from_millis(10)).await;

	Ok(())
}

// Show static pricing line for non-interactive mode
pub async fn show_no_animation(cancel_flag: Arc<AtomicBool>, cost: f64) -> Result<()> {
	// Display static pricing line for non-interactive mode
	// Skip in JSONL mode to avoid mixing plain text with JSON output
	if !std::io::stdin().is_terminal() {
		use crate::config::with_thread_config;
		let should_print =
			with_thread_config(|config| config.runtime_output_mode.as_deref() != Some("jsonl"))
				.unwrap_or(true);

		if should_print {
			println!(
				" ── cost: ${:.5} ────────────────────────────────────────",
				cost
			);
		}
	}

	// Wait for cancellation without showing any visual animation (faster polling)
	while !cancel_flag.load(Ordering::SeqCst) {
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	Ok(())
}

// Smart animation that automatically detects interactive vs non-interactive mode
pub async fn show_smart_animation(
	cancel_flag: Arc<AtomicBool>,
	cost: f64,
	current_context_tokens: u64,
	max_session_tokens_threshold: usize,
) -> Result<()> {
	if std::io::stdin().is_terminal() {
		// Interactive mode - show spinner animation
		show_loading_animation(
			cancel_flag,
			cost,
			current_context_tokens,
			max_session_tokens_threshold,
		)
		.await
	} else {
		// Non-interactive mode - show static cost line
		show_no_animation(cancel_flag, cost).await
	}
}

// Display generation message for non-interactive mode (without animation)
pub fn show_generation_message_static(cost: f64) {
	if !std::io::stdin().is_terminal() {
		// Non-interactive mode - show static pricing line
		// Skip in JSONL mode to avoid mixing plain text with JSON output
		use crate::config::with_thread_config;
		let should_print =
			with_thread_config(|config| config.runtime_output_mode.as_deref() != Some("jsonl"))
				.unwrap_or(true);

		if should_print {
			println!(
				" ── cost: ${:.5} ────────────────────────────────────────",
				cost
			);
		}
	}
	// Interactive mode - do nothing (animation will handle it)
}
