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
fn formats_elapsed_time_at_unit_boundaries() {
	let cases = [
		(Duration::from_secs(0), "0s"),
		(Duration::from_secs(59), "59s"),
		(Duration::from_secs(60), "1m"),
		(Duration::from_secs(61), "1m 1s"),
		(Duration::from_secs(3_599), "59m 59s"),
		(Duration::from_secs(3_600), "1h"),
		(Duration::from_secs(3_601), "1h 1s"),
		(Duration::from_secs(3_660), "1h 1m"),
		(Duration::from_secs(3_661), "1h 1m 1s"),
		(Duration::from_millis(999), "0s"),
		(Duration::from_secs(90_000), "25h"),
	];

	for (elapsed, expected) in cases {
		assert_eq!(format_elapsed_time(elapsed), expected, "elapsed={elapsed:?}");
	}
}
