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
fn test_zero_milliseconds() {
	assert_eq!(format_duration(0), "0ms");
}

#[test]
fn test_sub_second_values() {
	assert_eq!(format_duration(1), "1ms");
	assert_eq!(format_duration(99), "99ms");
	assert_eq!(format_duration(100), "100ms");
	assert_eq!(format_duration(999), "999ms");
}

#[test]
fn test_exact_seconds() {
	assert_eq!(format_duration(1_000), "1s");
	assert_eq!(format_duration(59_000), "59s");
}

#[test]
fn test_small_ms_dropped_when_other_units_present() {
	// ms < 100 is omitted when a larger unit is present
	assert_eq!(format_duration(1_050), "1s");
}

#[test]
fn test_large_ms_kept_when_other_units_present() {
	assert_eq!(format_duration(1_500), "1s 500ms");
	assert_eq!(format_duration(59_999), "59s 999ms");
}

#[test]
fn test_minutes() {
	assert_eq!(format_duration(60_000), "1m");
	assert_eq!(format_duration(61_000), "1m 1s");
	assert_eq!(format_duration(61_500), "1m 1s 500ms");
}

#[test]
fn test_hours() {
	assert_eq!(format_duration(3_600_000), "1h");
	assert_eq!(format_duration(3_661_500), "1h 1m 1s 500ms");
	assert_eq!(format_duration(7_323_000), "2h 2m 3s");
	assert_eq!(format_duration(86_400_000), "24h");
}

#[test]
fn test_boundary_below_one_hour() {
	assert_eq!(format_duration(3_599_999), "59m 59s 999ms");
}
