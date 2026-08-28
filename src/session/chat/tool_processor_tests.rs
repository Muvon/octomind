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
fn test_new_wires_error_tracker_with_threshold_three() {
	let processor = ToolProcessor::new();
	assert_eq!(processor.error_tracker.max_consecutive_errors(), 3);
}

#[test]
fn test_default_matches_new() {
	let processor = ToolProcessor::default();
	assert_eq!(processor.error_tracker.max_consecutive_errors(), 3);
}

#[test]
fn test_error_tracker_trips_after_three_consecutive_errors() {
	let mut processor = ToolProcessor::new();
	assert!(!processor.error_tracker.record_error("shell"));
	assert!(!processor.error_tracker.record_error("shell"));
	assert!(processor.error_tracker.record_error("shell"));
	assert_eq!(processor.error_tracker.get_error_count("shell"), 3);
}

#[test]
fn test_success_resets_error_streak() {
	let mut processor = ToolProcessor::new();
	assert!(!processor.error_tracker.record_error("shell"));
	assert!(!processor.error_tracker.record_error("shell"));
	processor.error_tracker.record_success("shell");
	assert_eq!(processor.error_tracker.get_error_count("shell"), 0);
	// Streak restarts from zero after a success
	assert!(!processor.error_tracker.record_error("shell"));
}
