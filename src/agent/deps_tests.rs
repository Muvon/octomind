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

//! Behavioral tests for `src/agent/deps.rs` — manifest `[deps]` parsing and
//! dep-script execution against a fabricated `<tap_root>/deps/` tree.

use super::*;
use std::path::Path;

fn write_dep(root: &Path, entry: &str, body: &str) {
	let path = root.join("deps").join(format!("{entry}.sh"));
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).expect("create deps dir");
	}
	std::fs::write(&path, body).expect("write dep script");
}

#[test]
fn parse_dep_entries_returns_empty_without_deps_section() {
	assert!(
		parse_dep_entries("[[roles]]\nname = \"x\"\n")
			.expect("manifest without [deps] parses")
			.is_empty()
	);
	assert!(
		parse_dep_entries("[deps]\n")
			.expect("[deps] without require parses")
			.is_empty()
	);
}

#[test]
fn parse_dep_entries_collects_string_entries() {
	let entries = parse_dep_entries("[deps]\nrequire = [\"org/one\", \"org/two\"]\n")
		.expect("valid manifest parses");
	assert_eq!(entries, vec!["org/one", "org/two"]);
}

#[test]
fn parse_dep_entries_rejects_invalid_shapes() {
	let err = parse_dep_entries("not = = toml").expect_err("invalid TOML must fail");
	assert!(
		err.to_string()
			.contains("Failed to parse manifest TOML for deps"),
		"got: {err:#}"
	);

	let err = parse_dep_entries("[deps]\nrequire = \"flat\"").expect_err("non-array require");
	assert!(
		err.to_string().contains("must be an array of strings"),
		"got: {err:#}"
	);

	let err = parse_dep_entries("[deps]\nrequire = [1, 2]").expect_err("non-string entries");
	assert!(
		err.to_string().contains("entries must be strings"),
		"got: {err:#}"
	);
}

#[tokio::test]
async fn resolve_deps_without_deps_section_is_a_noop() {
	let tmp = tempfile::tempdir().expect("tempdir");
	resolve_deps("[[roles]]\nname = \"x\"\n", tmp.path(), None)
		.await
		.expect("manifest without deps resolves");
}

#[tokio::test]
async fn run_dep_entries_reports_status_and_runs_scripts() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_dep(tmp.path(), "acme/probe", "exit 0\n");

	let statuses: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
	run_dep_entries(
		&["acme/probe".to_string()],
		tmp.path(),
		Some(&|status| {
			statuses
				.lock()
				.expect("statuses lock")
				.push(status.to_string())
		}),
	)
	.await
	.expect("successful dep script resolves");
	assert_eq!(
		statuses.into_inner().expect("statuses lock"),
		vec!["Checking dep: acme/probe"]
	);
}

#[tokio::test]
async fn run_dep_entries_missing_script_is_an_error() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let err = run_dep_entries(&["acme/ghost".to_string()], tmp.path(), None)
		.await
		.expect_err("missing dep script must fail");
	assert!(
		format!("{err:#}").contains("Dep script not found"),
		"got: {err:#}"
	);
	assert!(
		format!("{err:#}").contains("Dependency 'acme/ghost' failed"),
		"got: {err:#}"
	);
}

#[tokio::test]
async fn run_dep_entries_failure_without_stderr_omits_stderr_trailer() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_dep(tmp.path(), "acme/quiet", "exit 4\n");

	let err = run_dep_entries(&["acme/quiet".to_string()], tmp.path(), None)
		.await
		.expect_err("failing dep script must fail");
	let message = format!("{err:#}");
	assert!(
		message.contains("exited with status 4"),
		"got: {message}"
	);
	assert!(
		!message.contains("stderr:"),
		"no stderr trailer without output: {message}"
	);
}

#[tokio::test]
async fn run_dep_entries_failure_includes_captured_stderr() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_dep(tmp.path(), "acme/loud", "echo 'installer exploded' >&2\nexit 3\n");

	let err = run_dep_entries(&["acme/loud".to_string()], tmp.path(), None)
		.await
		.expect_err("failing dep script must fail");
	let message = format!("{err:#}");
	assert!(
		message.contains("exited with status 3"),
		"got: {message}"
	);
	assert!(
		message.contains("installer exploded"),
		"stderr must be surfaced: {message}"
	);
}

#[tokio::test]
async fn resolve_deps_runs_manifest_entries_end_to_end() {
	let tmp = tempfile::tempdir().expect("tempdir");
	write_dep(tmp.path(), "acme/tool", "exit 0\n");

	let manifest = "[[roles]]\nname = \"x\"\n\n[deps]\nrequire = [\"acme/tool\"]\n";
	resolve_deps(manifest, tmp.path(), None)
		.await
		.expect("manifest deps resolve against the tap root");
}
