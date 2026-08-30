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
fn upsert_replaces_the_key_and_keeps_the_rest() {
	let before = "OPENROUTER_API_KEY=abc\nOCTOHUB_API_KEY=old\nFOO=bar\n";
	let after = upsert(before, "new");
	assert_eq!(
		after,
		"OPENROUTER_API_KEY=abc\nFOO=bar\nOCTOHUB_API_KEY=new\n"
	);
	// Empty file, exported form, and repeated logins all converge on one line.
	assert_eq!(upsert("", "k"), "OCTOHUB_API_KEY=k\n");
	assert_eq!(
		upsert("export OCTOHUB_API_KEY=old", "k"),
		"OCTOHUB_API_KEY=k\n"
	);
	assert_eq!(
		upsert(&after, "third"),
		"OPENROUTER_API_KEY=abc\nFOO=bar\nOCTOHUB_API_KEY=third\n"
	);
}

#[test]
fn repoint_swaps_origin_keeps_path_and_query() {
	assert_eq!(
		repoint(
			"https://octomind.run/app/login/cli?code=AB12-CD34",
			"http://localhost:5199"
		),
		"http://localhost:5199/app/login/cli?code=AB12-CD34"
	);
}

#[test]
fn repoint_leaves_unrecognized_urls_alone() {
	assert_eq!(
		repoint("https://example.com/x", "http://localhost:1"),
		"https://example.com/x"
	);
}
