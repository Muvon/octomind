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

// Show loading animation while waiting for response (interactive mode)
pub async fn show_loading_animation(cancel_flag: Arc<AtomicBool>, cost: f64) -> Result<()> {
	// Create a spinner with cost-aware message in prompt format
	let spinner = ProgressBar::new_spinner();

	// Set a clean style with spinner and cost-aware message
	spinner.set_style(
		ProgressStyle::default_spinner()
			.template(" {spinner:.cyan} {msg:.cyan}")
			.unwrap()
			.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
	);

	// Format message with cost in prompt-like format
	let message = if cost > 0.0 {
		format!("[~${:.2}] Generating response...", cost)
	} else {
		"Generating response...".to_string()
	};
	spinner.set_message(message);
	spinner.enable_steady_tick(Duration::from_millis(50));

	// Wait for cancellation with faster polling
	while !cancel_flag.load(Ordering::SeqCst) {
		tokio::time::sleep(Duration::from_millis(10)).await;
	}

	// Clean finish - removes the spinner completely
	spinner.finish_and_clear();
	Ok(())
}

// Show static pricing line for non-interactive mode
pub async fn show_no_animation(cancel_flag: Arc<AtomicBool>, cost: f64) -> Result<()> {
	// Display static pricing line for non-interactive mode
	if !std::io::stdin().is_terminal() {
		println!(
			" ── cost: ${:.5} ────────────────────────────────────────",
			cost
		);
	}

	// Wait for cancellation without showing any visual animation (faster polling)
	while !cancel_flag.load(Ordering::SeqCst) {
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	Ok(())
}

// Smart animation that automatically detects interactive vs non-interactive mode
pub async fn show_smart_animation(cancel_flag: Arc<AtomicBool>, cost: f64) -> Result<()> {
	if std::io::stdin().is_terminal() {
		// Interactive mode - show spinner animation
		show_loading_animation(cancel_flag, cost).await
	} else {
		// Non-interactive mode - show static cost line
		show_no_animation(cancel_flag, cost).await
	}
}

// Display generation message for non-interactive mode (without animation)
pub fn show_generation_message_static(cost: f64) {
	if !std::io::stdin().is_terminal() {
		// Non-interactive mode - show static pricing line
		println!(
			" ── cost: ${:.5} ────────────────────────────────────────",
			cost
		);
	}
	// Interactive mode - do nothing (animation will handle it)
}
