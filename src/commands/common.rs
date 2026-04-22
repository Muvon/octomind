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

//! Shared helpers used by run, acp, and server commands.
//!
//! Tag resolution lives in `octomind::agent::resolver` (library-visible so
//! the runtime `/role` command can reuse it). This module wraps it with the
//! spinner-driven startup UX specific to the binary.

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use octomind::agent::resolver;
use octomind::config::Config;
use std::time::Duration;

/// Run the full startup sequence (tap/dep resolution + MCP init) under a single spinner.
///
/// Interactive mode: shows an animated spinner with live status messages.
/// Non-interactive mode: silent (errors still propagate).
pub async fn startup(
	tag: Option<&str>,
	config: &Config,
	is_interactive: bool,
) -> Result<(Config, String)> {
	if is_interactive {
		let spinner = make_spinner();

		// Phase 1: resolve config + deps (spinner shows tap/dep status)
		let spinner_ref = &spinner;
		let status_cb = |msg: &str| spinner_ref.set_message(msg.to_string());
		let resolve_result = resolver::resolve_config_and_role(tag, config, Some(&status_cb)).await;
		let (run_config, role) = match resolve_result {
			Ok(v) => v,
			Err(e) => {
				spinner.finish_and_clear();
				print!("\x1B[2K\r");
				std::io::Write::flush(&mut std::io::stdout()).ok();
				return Err(e);
			}
		};

		// Phase 2: MCP init under the same spinner
		if let Err(e) = mcp_init_with_spinner(&role, &run_config, &spinner).await {
			spinner.finish_and_clear();
			print!("\x1B[2K\r");
			std::io::Write::flush(&mut std::io::stdout()).ok();
			return Err(e);
		}
		spinner.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		Ok((run_config, role))
	} else {
		// Non-interactive: silent
		let (run_config, role) = resolver::resolve_config_and_role(tag, config, None).await?;
		octomind::mcp::initialize_mcp_for_role(&role, &run_config).await?;
		Ok((run_config, role))
	}
}

/// Initialize MCP servers only (no tap/dep resolution). Used by the server command,
/// which sets up tracing between config resolution and MCP init.
/// Shows a spinner in interactive mode, silent otherwise.
pub async fn startup_mcp_only(role: &str, config: &Config, is_interactive: bool) -> Result<()> {
	if is_interactive {
		let spinner = make_spinner();
		let result = mcp_init_with_spinner(role, config, &spinner).await;
		spinner.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		result
	} else {
		octomind::mcp::initialize_mcp_for_role(role, config).await
	}
}

fn make_spinner() -> ProgressBar {
	let spinner = ProgressBar::new_spinner();
	spinner.set_style(
		ProgressStyle::default_spinner()
			.template(" {spinner:.cyan} {msg:.cyan}")
			.unwrap()
			.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
	);
	spinner.enable_steady_tick(Duration::from_millis(80));
	spinner
}

async fn mcp_init_with_spinner(role: &str, config: &Config, spinner: &ProgressBar) -> Result<()> {
	use octomind::mcp::McpInitProgress;
	use std::sync::{Arc, Mutex};

	let pending: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
	let total = Arc::new(Mutex::new(0usize));

	let cb = |progress: McpInitProgress| match &progress {
		McpInitProgress::Starting { servers } => {
			*total.lock().unwrap() = servers.len();
			if servers.is_empty() {
				spinner.set_message("Starting MCP...".to_string());
			} else {
				*pending.lock().unwrap() = servers.clone();
				spinner.set_message(format!(
					"Starting MCP: {} [0/{}]",
					servers.join(", "),
					servers.len()
				));
			}
		}
		McpInitProgress::Completed { server, .. } => {
			let mut pending_guard = pending.lock().unwrap();
			pending_guard.retain(|s| s != server);
			let done = *total.lock().unwrap() - pending_guard.len();
			let total_count = *total.lock().unwrap();
			if pending_guard.is_empty() {
				spinner.set_message(format!("Starting MCP: done [{}/{}]", done, total_count));
			} else {
				spinner.set_message(format!(
					"Starting MCP: {} [{}/{}]",
					pending_guard.join(", "),
					done,
					total_count
				));
			}
		}
	};

	octomind::mcp::initialize_mcp_for_role_with_callback(role, config, Some(&cb)).await
}
