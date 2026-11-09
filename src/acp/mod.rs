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

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::config::Config;

/// Per-session async exclusion locks shared by `prompt()`, the inbox monitor,
/// and ext commands (single-threaded LocalSet, hence `Rc`/`RefCell`).
pub type SessionLocks = Rc<RefCell<HashMap<String, Rc<tokio::sync::Mutex<()>>>>>;

/// Stack for the thread that polls every ACP session future. The default main
/// thread stack is 1 MiB on Windows, and a full tool round there overflows it.
const ACP_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Runtime options for the ACP server, mirroring the relevant subset of `octomind run` flags.
///
/// Values like `name`/`resume`/`resume_recent` are consumed once on the first client
/// `new_session` request, then revert to defaults. `model` applies to every
/// session created or loaded during the agent's lifetime.
#[derive(Debug, Clone, Default)]
pub struct AcpRunOptions {
	pub name: Option<String>,
	pub resume: Option<String>,
	pub resume_recent: bool,
	pub model: Option<String>,
	/// Why the role could not start (tap resolution, a missing INPUT/ENV value,
	/// MCP init). When set, the agent still answers `initialize` and fails every
	/// session request with this text, so the client learns the cause instead
	/// of seeing the pipe close.
	pub startup_error: Option<String>,
}

/// Run the ACP agent over stdio until the client disconnects.
pub async fn run(config: Config, role: String, options: AcpRunOptions) -> Result<()> {
	// In ACP mode stdout/stderr are reserved for JSON-RPC, so init failures
	// are written to a fallback file instead of eprintln.
	let write_init_error = |msg: String| {
		if let Ok(logs_dir) = crate::directories::get_logs_dir() {
			let log_file = logs_dir.join("acp-init-errors.log");
			if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_file) {
				use std::io::Write;
				let _ = writeln!(file, "{msg}");
			}
		}
	};

	// Initialize tracing for ACP mode - logs to file, not stdout/stderr
	let log_level = config.log_level.as_str();
	if let Err(e) =
		crate::logging::tracing_setup::init_tracing(crate::logging::tracing_setup::LoggingMode::Acp, log_level)
	{
		write_init_error(format!("Failed to initialize tracing: {e}"));
	}

	// Initialize ACP error sink for structured error logging
	if let Err(e) = crate::logging::AcpErrorSink::initialize() {
		write_init_error(format!("Failed to initialize ACP error sink: {e}"));
	}

	// Run the actor and the ACP event loop together on a single-threaded LocalSet.
	// The SDK requires `Send` handlers while our session machinery is `!Send`;
	// the bridge lives in `agent::serve` (see the module-level notes there).
	// The LocalSet gets its own thread because polling it on the main thread
	// ties the deepest session futures to the platform's main-thread stack size.
	let runtime = tokio::runtime::Handle::current();
	let acp_thread = std::thread::Builder::new()
		.name("acp".into())
		.stack_size(ACP_THREAD_STACK_BYTES)
		.spawn(move || {
			let local = tokio::task::LocalSet::new();
			runtime.block_on(local.run_until(agent::serve(config, role, options)))
		})
		.context("spawn ACP thread")?;
	match tokio::task::spawn_blocking(move || acp_thread.join())
		.await
		.context("join ACP thread")?
	{
		Ok(result) => result,
		Err(panic) => std::panic::resume_unwind(panic),
	}
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
