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
fn ordinary_paths_are_unchanged() {
	assert_eq!(
		escape_profile_str("/Users/dev/project"),
		"/Users/dev/project"
	);
	assert_eq!(escape_profile_str("/tmp/a b/c-d.e"), "/tmp/a b/c-d.e");
}

#[test]
fn quotes_and_backslashes_stay_inside_the_string_literal() {
	// A directory literally named `x") (allow file-write* (subpath "/` would
	// otherwise close the literal and re-open writes to the whole disk.
	let escaped = escape_profile_str("/tmp/x\") (allow file-write* (subpath \"/");
	// Every quote that reaches the profile is escaped — none can terminate it.
	assert_eq!(escaped.matches('"').count(), 2);
	assert_eq!(escaped.matches("\\\"").count(), 2);

	assert_eq!(escape_profile_str(r"a\b"), r"a\\b");
	// Backslashes are escaped before quotes, so `\"` does not become `\\"`.
	assert_eq!(escape_profile_str("a\\\"b"), "a\\\\\\\"b");
}
