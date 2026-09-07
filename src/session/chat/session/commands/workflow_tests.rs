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
fn split_input_keeps_everything_after_the_name_verbatim() {
	assert_eq!(
		split_input("/workflow summarize-thread https://a.example/t?x=1  second  part"),
		Some("https://a.example/t?x=1  second  part")
	);
	assert_eq!(
		split_input("  /workflow watch-page   https://b.example "),
		Some("https://b.example")
	);
}

#[test]
fn split_input_rejects_missing_input() {
	assert_eq!(split_input("/workflow"), None);
	assert_eq!(split_input("/workflow watch-page"), None);
	assert_eq!(split_input("/workflow watch-page   "), None);
}

#[tokio::test]
async fn missing_input_is_a_usage_error_not_a_run() {
	let res = handle_workflow("/workflow watch-page", &["watch-page"])
		.await
		.unwrap();
	let CommandResult::HandledWithOutput(out) = res else {
		panic!("expected output");
	};
	let CommandOutput::Workflow { data } = *out else {
		panic!("expected workflow output");
	};
	assert_eq!(data["subcommand"], "error");
	assert!(data["message"]
		.as_str()
		.unwrap()
		.starts_with("usage: /workflow watch-page"));
}
