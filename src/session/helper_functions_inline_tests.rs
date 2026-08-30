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

#[tokio::test]
async fn prompt_without_placeholders_is_returned_verbatim() {
	let dir = Path::new(".");
	let prompt = "just a plain instruction with no substitutions";
	assert_eq!(
		process_placeholders_async(prompt, dir).await,
		prompt,
		"the no-placeholder fast path must not alter the prompt"
	);
}

#[tokio::test]
async fn escaped_braces_survive_the_no_placeholder_fast_path() {
	// `{{{{foo}}}}` means a literal `{{foo}}`. The fast path protects braces
	// before deciding there is nothing to substitute, so it must restore them
	// or the NUL sentinels reach the model.
	let out = process_placeholders_async("write {{{{foo}}}} verbatim", Path::new(".")).await;
	assert_eq!(out, "write {{foo}} verbatim");
	assert!(!out.contains('\x00'), "sentinel leaked: {out:?}");
}

#[tokio::test]
async fn known_placeholders_are_substituted() {
	let dir = Path::new("/tmp/some-project");
	let out = process_placeholders_async("cwd is {{CWD}}", dir).await;
	assert_eq!(out, "cwd is /tmp/some-project");
}

#[tokio::test]
async fn role_placeholder_falls_back_when_no_role_is_given() {
	let dir = Path::new(".");
	assert_eq!(
		process_placeholders_async_with_role("role={{ROLE}}", dir, Some("developer")).await,
		"role=developer"
	);
	assert_eq!(
		process_placeholders_async_with_role("role={{ROLE}}", dir, None).await,
		"role=unknown"
	);
}

#[tokio::test]
async fn unknown_placeholders_are_left_alone() {
	let out =
		process_placeholders_async("keep {{NOT_A_PLACEHOLDER}} and {{CWD}}", Path::new("/x")).await;
	assert_eq!(out, "keep {{NOT_A_PLACEHOLDER}} and /x");
}
