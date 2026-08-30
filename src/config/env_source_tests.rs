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
use std::env;

#[test]
fn test_env_tracker_new() {
	let tracker = EnvTracker::new();
	// Should capture current environment
	assert!(!tracker.pre_dotenv_vars.is_empty());
	assert!(!tracker.dotenv_loaded);
}

#[test]
fn test_source_detection_system_only() {
	// Set a test variable
	env::set_var("TEST_SYSTEM_VAR", "system_value");

	let tracker = EnvTracker::new();

	// Should detect as system source
	assert_eq!(tracker.get_source("TEST_SYSTEM_VAR"), EnvSource::System);
	assert_eq!(
		tracker.get_source_description("TEST_SYSTEM_VAR"),
		"environment variable"
	);

	// Clean up
	env::remove_var("TEST_SYSTEM_VAR");
}

#[test]
fn test_source_detection_not_found() {
	let tracker = EnvTracker::new();

	// Should detect as not found
	assert_eq!(tracker.get_source("NONEXISTENT_VAR"), EnvSource::NotFound);
	assert_eq!(tracker.get_source_description("NONEXISTENT_VAR"), "not set");
}
