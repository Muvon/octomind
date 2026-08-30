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
fn duration_short_picks_the_two_largest_units() {
	assert_eq!(format_duration_short(0), "0m00s");
	assert_eq!(format_duration_short(48), "0m48s");
	assert_eq!(format_duration_short(192), "3m12s");
	assert_eq!(format_duration_short(3_900), "1h05m");
	assert_eq!(format_duration_short(183_600), "2d03h");
}

#[test]
fn duration_short_pads_at_unit_boundaries() {
	assert_eq!(format_duration_short(59), "0m59s");
	assert_eq!(format_duration_short(60), "1m00s");
	assert_eq!(format_duration_short(3_599), "59m59s");
	assert_eq!(format_duration_short(3_600), "1h00m");
	assert_eq!(format_duration_short(86_399), "23h59m");
	assert_eq!(format_duration_short(86_400), "1d00h");
}

#[test]
fn ago_thresholds() {
	assert_eq!(format_ago(0), "just now");
	assert_eq!(format_ago(4), "just now");
	assert_eq!(format_ago(5), "5s ago");
	assert_eq!(format_ago(59), "59s ago");
	assert_eq!(format_ago(60), "1m ago");
	assert_eq!(format_ago(3_599), "59m ago");
	assert_eq!(format_ago(3_600), "1h ago");
	assert_eq!(format_ago(86_399), "23h ago");
	assert_eq!(format_ago(86_400), "1d ago");
	assert_eq!(format_ago(432_000), "5d ago");
}

#[test]
fn now_secs_is_a_plausible_wall_clock() {
	// Sanity floor: 2026-01-01. Guards against a unit mix-up (ms vs s).
	assert!(now_secs() > 1_767_225_600);
}
