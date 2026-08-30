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
fn interval_renders_every_non_zero_unit() {
	assert_eq!(format_interval(45), "45s");
	assert_eq!(format_interval(600), "10m");
	assert_eq!(format_interval(605), "10m 5s");
	assert_eq!(format_interval(3_600), "1h");
	assert_eq!(format_interval(5_400), "1h 30m");
}

#[test]
fn interval_keeps_units_when_hours_and_seconds_coexist() {
	// `every = "1h30m45s"` parses fine, so the label must not collapse
	// back to a raw second count.
	assert_eq!(format_interval(5_445), "1h 30m 45s");
	assert_eq!(format_interval(3_605), "1h 5s");
}

#[test]
fn interval_zero_is_seconds() {
	assert_eq!(format_interval(0), "0s");
}

#[test]
fn interval_round_trips_parsed_durations() {
	for input in ["45s", "10m", "1h", "1h30m", "2h 30m 10s"] {
		let secs = parse_duration_secs(input).expect("parses");
		let label = format_interval(secs);
		assert_eq!(
			parse_duration_secs(&label.replace(' ', "")).unwrap(),
			secs,
			"{input} -> {label} did not round-trip"
		);
	}
}
