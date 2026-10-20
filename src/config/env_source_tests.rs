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

#[test]
#[serial_test::serial]
fn dotenv_source_detected_for_new_variable() {
	let mut tracker = EnvTracker::new();
	env::set_var("OCTOMIND_TEST_DOTENV_NEW", "from-file");
	tracker.dotenv_loaded = true;
	assert_eq!(tracker.get_source("OCTOMIND_TEST_DOTENV_NEW"), EnvSource::DotEnv);
	assert_eq!(
		tracker.get_source_description("OCTOMIND_TEST_DOTENV_NEW"),
		"environment/.env file"
	);
	env::remove_var("OCTOMIND_TEST_DOTENV_NEW");
}

#[test]
#[serial_test::serial]
fn unchanged_pre_existing_variable_stays_system_even_with_dotenv() {
	env::set_var("OCTOMIND_TEST_DOTENV_SAME", "value");
	let mut tracker = EnvTracker::new();
	tracker.dotenv_loaded = true;
	assert_eq!(tracker.get_source("OCTOMIND_TEST_DOTENV_SAME"), EnvSource::System);
	env::remove_var("OCTOMIND_TEST_DOTENV_SAME");
}

#[test]
#[serial_test::serial]
fn changed_pre_existing_variable_becomes_dotenv() {
	env::set_var("OCTOMIND_TEST_DOTENV_CHANGED", "before");
	let mut tracker = EnvTracker::new();
	env::set_var("OCTOMIND_TEST_DOTENV_CHANGED", "after");
	tracker.dotenv_loaded = true;
	assert_eq!(tracker.get_source("OCTOMIND_TEST_DOTENV_CHANGED"), EnvSource::DotEnv);
	env::remove_var("OCTOMIND_TEST_DOTENV_CHANGED");
}

#[test]
#[serial_test::serial]
fn empty_value_is_not_found_even_when_set() {
	env::set_var("OCTOMIND_TEST_DOTENV_EMPTY", "  ");
	let tracker = EnvTracker::new();
	assert_eq!(tracker.get_source("OCTOMIND_TEST_DOTENV_EMPTY"), EnvSource::NotFound);
	env::remove_var("OCTOMIND_TEST_DOTENV_EMPTY");
}
