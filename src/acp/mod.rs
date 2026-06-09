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

//! ACP (Agent Client Protocol) server implementation.
//!
//! Runs Octomind as an ACP agent over stdio, compatible with clients
//! like Zed editor and JetBrains IDEs.

mod agent;
pub mod commands;

use anyhow::Result;

use crate::config::Config;

/// Runtime options for the ACP server, mirroring the relevant subset of `octomind run` flags.
///
/// Values like `name`/`resume`/`resume_recent` are consumed once on the first client
/// `new_session` request, then revert to defaults. `model` and `hooks` apply to every
/// session created or loaded during the agent's lifetime.
#[derive(Debug, Clone, Default)]
pub struct AcpRunOptions {
	pub name: Option<String>,
	pub resume: Option<String>,
	pub resume_recent: bool,
	pub model: Option<String>,
	pub hooks: Vec<String>,
}

/// Run the ACP agent over stdio until the client disconnects.
pub async fn run(config: Config, role: String, options: AcpRunOptions) -> Result<()> {
	// In ACP mode stdout/stderr are reserved for JSON-RPC, so init failures
	// are written to a fallback file instead of eprintln.
	let write_init_error = |msg: String| {
		if let Ok(logs_dir) = crate::directories::get_logs_dir() {
			let log_file = logs_dir.join("acp-init-errors.log");
			if let Ok(mut file) = std::fs::OpenOptions::new()
				.create(true)
				.append(true)
				.open(&log_file)
			{
				use std::io::Write;
				let _ = writeln!(file, "{msg}");
			}
		}
	};

	// Initialize tracing for ACP mode - logs to file, not stdout/stderr
	let log_level = config.log_level.as_str();
	if let Err(e) = crate::logging::tracing_setup::init_tracing(
		crate::logging::tracing_setup::LoggingMode::Acp,
		log_level,
	) {
		write_init_error(format!("Failed to initialize tracing: {e}"));
	}

	// Initialize ACP error sink for structured error logging
	if let Err(e) = crate::logging::AcpErrorSink::initialize() {
		write_init_error(format!("Failed to initialize ACP error sink: {e}"));
	}

	// Run the actor and the ACP event loop together on a single-threaded LocalSet.
	// The 0.14 SDK requires `Send` handlers while our session machinery is `!Send`;
	// the bridge lives in `agent::serve` (see the module-level notes there).
	let local = tokio::task::LocalSet::new();
	local.run_until(agent::serve(config, role, options)).await
}
