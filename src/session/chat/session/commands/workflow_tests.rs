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
#[serial_test::serial]
async fn name_without_input_shows_definition_not_a_run() {
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	let dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
	// Pre-create the default tap dir so `load_taps` never tries to clone it.
	let wf_dir = dir
		.path()
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("workflows");
	std::fs::create_dir_all(&wf_dir).expect("workflows dir");
	std::fs::write(wf_dir.join("watch-page.toml"), "description = \"d\"\n").expect("workflow");

	let res = handle_workflow("/workflow watch-page", &["watch-page"]).await;
	match previous {
		Some(old) => std::env::set_var("OCTOMIND_DATA_DIR", old),
		None => std::env::remove_var("OCTOMIND_DATA_DIR"),
	}
	let CommandResult::HandledWithOutput(out) = res.unwrap() else {
		panic!("expected output");
	};
	let CommandOutput::Workflow { data } = *out else {
		panic!("expected workflow output");
	};
	assert_eq!(data["subcommand"], "show");
	assert_eq!(data["name"], "watch-page");
	assert_eq!(data["source_tap"], "muvon/tap");
	assert_eq!(data["definition"], "description = \"d\"\n");
}
