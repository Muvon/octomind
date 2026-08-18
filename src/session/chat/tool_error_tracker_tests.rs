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
fn test_threshold_trips_at_max_errors() {
	let mut tracker = ToolErrorTracker::new(3);
	assert_eq!(tracker.max_consecutive_errors(), 3);
	assert!(!tracker.record_error("shell"));
	assert!(!tracker.record_error("shell"));
	assert!(tracker.record_error("shell"));
	assert_eq!(tracker.get_error_count("shell"), 3);
}

#[test]
fn test_success_resets_counter() {
	let mut tracker = ToolErrorTracker::new(2);
	assert!(!tracker.record_error("shell"));
	tracker.record_success("shell");
	assert_eq!(tracker.get_error_count("shell"), 0);
	// Streak restarts from zero after a success
	assert!(!tracker.record_error("shell"));
	assert!(tracker.record_error("shell"));
}

#[test]
fn test_tools_are_tracked_independently() {
	let mut tracker = ToolErrorTracker::new(2);
	assert!(!tracker.record_error("shell"));
	assert_eq!(tracker.get_error_count("read"), 0);
	assert!(!tracker.record_error("read"));
	// A success on one tool must not reset the other
	tracker.record_success("read");
	assert_eq!(tracker.get_error_count("shell"), 1);
}
