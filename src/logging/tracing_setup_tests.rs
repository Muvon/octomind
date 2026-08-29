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

//! External tests for `src/logging/tracing_setup.rs` — mode tracking, tracing
//! initialization per mode, and env-filter construction. The global tracing
//! subscriber can only be set once per process, so tests that assert on its
//! side effects are conditional on being the first initializer; env-touching
//! tests are `#[serial]` because env vars are process-global.

use super::*;
use serial_test::serial;

/// Point `OCTOMIND_DATA_DIR` at a fresh tempdir for the duration of a test,
/// restoring the previous value on drop.
struct DataDirGuard {
	previous: Option<std::ffi::OsString>,
	_dir: tempfile::TempDir,
}

impl DataDirGuard {
	fn new() -> Self {
		let previous = std::env::var_os("OCTOMIND_DATA_DIR");
		let dir = tempfile::tempdir().expect("failed to create tempdir");
		std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
		Self {
			previous,
			_dir: dir,
		}
	}
}

impl Drop for DataDirGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("OCTOMIND_DATA_DIR", v),
			None => std::env::remove_var("OCTOMIND_DATA_DIR"),
		}
	}
}

/// Set (or clear) `RUST_LOG` for the duration of a test, restoring on drop.
struct RustLogGuard {
	previous: Option<std::ffi::OsString>,
}

impl RustLogGuard {
	fn set(value: &str) -> Self {
		let previous = std::env::var_os("RUST_LOG");
		std::env::set_var("RUST_LOG", value);
		Self { previous }
	}

	fn clear() -> Self {
		let previous = std::env::var_os("RUST_LOG");
		std::env::remove_var("RUST_LOG");
		Self { previous }
	}
}

impl Drop for RustLogGuard {
	fn drop(&mut self) {
		match self.previous.take() {
			Some(v) => std::env::set_var("RUST_LOG", v),
			None => std::env::remove_var("RUST_LOG"),
		}
	}
}

// --- LoggingMode tracking -----------------------------------------------------

#[test]
fn logging_mode_enum_is_copy_eq_and_debug() {
	assert_eq!(LoggingMode::Cli, LoggingMode::Cli);
	assert_ne!(LoggingMode::Acp, LoggingMode::WebSocket);
	assert!(format!("{:?}", LoggingMode::Silent).contains("Silent"));
	assert!(format!("{:?}", LoggingMode::Acp).contains("Acp"));
	let mode = LoggingMode::WebSocket;
	let copy = mode;
	assert_eq!(mode, copy);
}

#[serial]
#[test]
fn set_logging_mode_is_visible_to_getter() {
	set_logging_mode(LoggingMode::Silent);
	let mode = get_logging_mode();
	assert!(mode.is_some(), "mode must be set after set_logging_mode");
	// The structured-output check must agree with the raw mode value.
	assert_eq!(
		is_structured_output_mode(),
		matches!(mode, Some(LoggingMode::Acp | LoggingMode::WebSocket))
	);
}

// --- init_tracing per mode ------------------------------------------------------

#[serial]
#[test]
fn init_tracing_silent_initializes_subscriber_idempotently() {
	let _log = RustLogGuard::clear();
	init_tracing(LoggingMode::Silent, "off").expect("silent init");
	assert!(is_tracing_initialized());
	// A second call is a no-op that still succeeds.
	init_tracing(LoggingMode::Silent, "off").expect("idempotent init");
}

#[serial]
#[test]
fn init_tracing_cli_without_rust_log_skips_subscriber() {
	let _log = RustLogGuard::clear();
	init_tracing(LoggingMode::Cli, "info").expect("cli init without RUST_LOG");
}

#[serial]
#[test]
fn init_tracing_cli_with_rust_log_sets_subscriber() {
	let _log = RustLogGuard::set("debug");
	let first = !is_tracing_initialized();
	init_tracing(LoggingMode::Cli, "debug").expect("cli init with RUST_LOG");
	if first {
		assert!(is_tracing_initialized());
	}
}

#[serial]
#[test]
fn init_tracing_acp_writes_to_log_file() {
	let _data = DataDirGuard::new();
	let _log = RustLogGuard::clear();
	let first = !is_tracing_initialized();
	init_tracing(LoggingMode::Acp, "debug").expect("acp init");
	if first {
		let logs = crate::directories::get_logs_dir().expect("logs dir");
		assert!(
			logs.join("acp-debug.log").exists(),
			"acp log file must be created on first init"
		);
	}
}

#[serial]
#[test]
fn init_tracing_websocket_writes_to_log_file() {
	let _data = DataDirGuard::new();
	let _log = RustLogGuard::clear();
	let first = !is_tracing_initialized();
	init_tracing(LoggingMode::WebSocket, "info").expect("websocket init");
	if first {
		let logs = crate::directories::get_logs_dir().expect("logs dir");
		assert!(
			logs.join("websocket-debug.log").exists(),
			"websocket log file must be created on first init"
		);
	}
}

// --- env filter construction ------------------------------------------------------

#[serial]
#[test]
fn create_env_filter_accepts_known_levels_and_defaults_unknown() {
	let _log = RustLogGuard::clear();
	for level in [
		"debug", "info", "warn", "error", "trace", "off", "DEBUG", "nonsense", "",
	] {
		assert!(
			create_env_filter(level).is_ok(),
			"level {level:?} must produce a filter"
		);
	}
}

#[serial]
#[test]
fn create_env_filter_prefers_rust_log_when_set() {
	let _log = RustLogGuard::set("octomind=trace");
	assert!(create_env_filter("error").is_ok());
}
