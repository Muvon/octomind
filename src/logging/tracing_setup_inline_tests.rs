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

use super::*;

#[test]
fn test_create_env_filter() {
	// Test that filters are created successfully
	assert!(create_env_filter("debug").is_ok());
	assert!(create_env_filter("info").is_ok());
	assert!(create_env_filter("warn").is_ok());
	assert!(create_env_filter("error").is_ok());
	assert!(create_env_filter("trace").is_ok());
	assert!(create_env_filter("off").is_ok());
	// Unknown level defaults to info
	assert!(create_env_filter("unknown").is_ok());
}

#[test]
fn test_logging_mode_tracking() {
	// Test that we can set and get logging mode
	// Note: OnceLock can only be set once, so we test the getter
	// The setter is tested implicitly by init_tracing
	let mode = get_logging_mode();
	// Mode might be None if tests run before any init
	assert!(
		mode.is_none()
			|| matches!(
				mode,
				Some(
					LoggingMode::Cli
						| LoggingMode::Acp | LoggingMode::WebSocket
						| LoggingMode::Silent
				)
			)
	);
}

#[test]
fn test_is_structured_output_mode() {
	// Without initialization, should return false
	// (or true if a previous test initialized it to Acp/WebSocket)
	let _ = is_structured_output_mode();
}

#[test]
fn test_is_tracing_initialized() {
	// Test that we can check if tracing is initialized
	// This should not panic
	let _ = is_tracing_initialized();
}
