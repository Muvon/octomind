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

//! Tracing setup for structured logging.
//!
//! Provides initialization for the `tracing` crate with support for:
//! - Console output (CLI mode) - colored stderr
//! - File output (ACP/WebSocket mode) - file-based logging
//! - Global logging mode tracking for macros
//!
//! ## Architecture
//!
//! The logging system uses a two-tier approach:
//! 1. **Global mode tracking** (`LOGGING_MODE`): Set once at startup, read by macros
//! 2. **Tracing subscriber**: Configured based on mode (CLI → stderr, ACP/WebSocket → file)
//!
//! ## Usage
//!
//! At startup (before any logging), call `init_tracing(LoggingMode::Acp,
//! "debug")`. In code, use the mode-aware macros `log_debug!`, `log_info!`,
//! `log_error!` — they route to `tracing::{debug,info,error}!` and, in ACP
//! mode, also write to the file sink.

use anyhow::{Context, Result};
use std::sync::Arc;
use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

/// Logging mode based on how Octomind is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggingMode {
	/// CLI mode - log to stderr with colors
	Cli,
	/// ACP mode - log to file only (stdout/stderr reserved for JSON-RPC)
	Acp,
	/// WebSocket mode - log to file only
	WebSocket,
	/// Silent mode - no logging
	Silent,
}

// ============================================================================
// GLOBAL LOGGING MODE TRACKER
// ============================================================================

/// Global logging mode - set once at startup, read by macros.
/// Uses OnceLock for thread-safe initialization.
static LOGGING_MODE: std::sync::OnceLock<LoggingMode> = std::sync::OnceLock::new();

/// Set the global logging mode. Should be called once at startup.
pub fn set_logging_mode(mode: LoggingMode) {
	let _ = LOGGING_MODE.set(mode);
}

/// Get the current logging mode. Returns None if not initialized.
#[inline]
pub fn get_logging_mode() -> Option<LoggingMode> {
	LOGGING_MODE.get().copied()
}

/// Check if we're in ACP or WebSocket mode (structured output modes).
/// In these modes, stdout/stderr are reserved for protocol communication.
#[inline]
pub fn is_structured_output_mode() -> bool {
	matches!(
		get_logging_mode(),
		Some(LoggingMode::Acp | LoggingMode::WebSocket)
	)
}

/// Check if tracing is initialized (subscriber has been set).
#[inline]
pub fn is_tracing_initialized() -> bool {
	tracing::dispatcher::has_been_set()
}

// ============================================================================
// TRACING INITIALIZATION
// ============================================================================

/// Initialize tracing for the given mode.
///
/// This should be called once at startup.
/// Sets both the global logging mode and initializes the tracing subscriber.
pub fn init_tracing(mode: LoggingMode, log_level: &str) -> Result<()> {
	// Set global logging mode first (even if tracing already initialized)
	set_logging_mode(mode);

	// Check if already initialized
	if tracing::dispatcher::has_been_set() {
		return Ok(());
	}

	match mode {
		LoggingMode::Cli => init_cli_logging(log_level),
		LoggingMode::Acp => init_acp_logging(log_level),
		LoggingMode::WebSocket => init_websocket_logging(log_level),
		LoggingMode::Silent => init_silent_logging(),
	}
}
/// `MakeWriter` that suspends the working spinner around every write.
///
/// The CLI tracing subscriber writes to stderr — the same surface the
/// spinner (indicatif) draws on. A log line emitted while the spinner is
/// live desynchronizes indicatif's line accounting: the bar is never
/// cleared from its row and survives as a stale line above later output
/// (e.g. the `· Supervisor:` notices). Routing each write through
/// `with_suspended_spinner` clears the bar first and redraws it after,
/// keeping the terminal consistent.
///
/// ACP/WebSocket subscribers log to files and don't need this.
#[derive(Clone, Copy, Default)]
struct SpinnerAwareMakeWriter<W>(W);

/// The per-event writer half of [`SpinnerAwareMakeWriter`].
struct SpinnerAwareWriter<W>(W);

impl<W> std::io::Write for SpinnerAwareWriter<W>
where
	W: std::io::Write,
{
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		crate::utils::terminal_output::with_suspended_spinner(|| self.0.write(buf))
	}

	fn flush(&mut self) -> std::io::Result<()> {
		crate::utils::terminal_output::with_suspended_spinner(|| self.0.flush())
	}
}

impl<'a, W> tracing_subscriber::fmt::MakeWriter<'a> for SpinnerAwareMakeWriter<W>
where
	W: tracing_subscriber::fmt::MakeWriter<'a>,
{
	type Writer = SpinnerAwareWriter<W::Writer>;

	fn make_writer(&'a self) -> Self::Writer {
		SpinnerAwareWriter(self.0.make_writer())
	}
}

/// Initialize CLI logging.
///
/// In CLI mode, user-facing output goes through the colored log macros (log_info!, log_debug!,
/// log_error!) which write directly to stdout/stderr. Tracing is only initialized here if
/// RUST_LOG is explicitly set, allowing developers to capture internal tracing events.
fn init_cli_logging(_log_level: &str) -> Result<()> {
	// Only set up a tracing subscriber if the developer explicitly requested it via RUST_LOG.
	// Without RUST_LOG, the log macros use colored println/eprintln — no tracing noise for users.
	if std::env::var("RUST_LOG").is_ok() {
		let filter = EnvFilter::from_env("RUST_LOG");

		let subscriber = tracing_subscriber::fmt()
			.with_writer(SpinnerAwareMakeWriter(std::io::stderr))
			.with_target(false)
			.with_thread_ids(false)
			.with_file(false)
			.with_line_number(false)
			.with_span_events(FmtSpan::CLOSE)
			.with_env_filter(filter)
			.finish();

		tracing::subscriber::set_global_default(subscriber)
			.with_context(|| "Failed to set tracing subscriber")?;
	}

	Ok(())
}

/// Initialize ACP logging (file only, stderr reserved for JSON-RPC).
fn init_acp_logging(log_level: &str) -> Result<()> {
	let logs_dir = crate::directories::get_logs_dir()?;
	let log_file = logs_dir.join("acp-debug.log");

	let file = std::fs::OpenOptions::new()
		.create(true)
		.append(true)
		.open(&log_file)
		.with_context(|| format!("Failed to open log file: {:?}", log_file))?;

	let filter = create_env_filter(log_level)?;

	let subscriber = tracing_subscriber::fmt()
		.with_writer(Arc::new(file))
		.with_target(true)
		.with_thread_ids(true)
		.with_file(true)
		.with_line_number(true)
		.with_span_events(FmtSpan::CLOSE)
		.with_env_filter(filter)
		.finish();

	tracing::subscriber::set_global_default(subscriber)
		.with_context(|| "Failed to set tracing subscriber")?;

	Ok(())
}

/// Initialize WebSocket logging (file only).
fn init_websocket_logging(log_level: &str) -> Result<()> {
	let logs_dir = crate::directories::get_logs_dir()?;
	let log_file = logs_dir.join("websocket-debug.log");

	let file = std::fs::OpenOptions::new()
		.create(true)
		.append(true)
		.open(&log_file)
		.with_context(|| format!("Failed to open log file: {:?}", log_file))?;

	let filter = create_env_filter(log_level)?;

	let subscriber = tracing_subscriber::fmt()
		.with_writer(Arc::new(file))
		.with_target(true)
		.with_thread_ids(true)
		.with_file(true)
		.with_line_number(true)
		.with_span_events(FmtSpan::CLOSE)
		.with_env_filter(filter)
		.finish();

	tracing::subscriber::set_global_default(subscriber)
		.with_context(|| "Failed to set tracing subscriber")?;

	Ok(())
}

/// Initialize silent logging (no output).
fn init_silent_logging() -> Result<()> {
	let subscriber = tracing_subscriber::fmt()
		.with_writer(std::io::sink)
		.with_env_filter(EnvFilter::new("off"))
		.finish();

	tracing::subscriber::set_global_default(subscriber)
		.with_context(|| "Failed to set tracing subscriber")?;

	Ok(())
}

/// Create an environment filter from the log level.
fn create_env_filter(level: &str) -> Result<EnvFilter> {
	let filter_str = match level.to_lowercase().as_str() {
		"debug" => "debug",
		"info" => "info",
		"warn" => "warn",
		"error" => "error",
		"trace" => "trace",
		"off" => "off",
		_ => "info",
	};

	// Allow override via RUST_LOG environment variable
	let filter = if std::env::var("RUST_LOG").is_ok() {
		EnvFilter::from_env("RUST_LOG")
	} else {
		EnvFilter::new(filter_str)
	};

	Ok(filter)
}

#[cfg(test)]
#[path = "tracing_setup_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "tracing_setup_tests.rs"]
mod external_tests;

#[cfg(test)]
#[path = "tracing_setup_spinner_tests.rs"]
mod spinner_tests;
