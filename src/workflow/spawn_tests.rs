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

//! Tests for tap workflow spawning. `run_tap_workflow` shells out to
//! `current_exe()`, which inside a test is the test binary — it cannot serve
//! `octomind workflow`, so the failure path (bail with a `--dry-run` hint) is
//! the deterministic outcome. `list_tap_workflows` is checked against a
//! throwaway data dir with a pre-created default tap, mirroring the
//! workflow command tests.

use super::*;

#[tokio::test]
async fn run_tap_workflow_failure_includes_dry_run_hint() {
	let err = match run_tap_workflow("definitely-missing-workflow", "input").await {
		Err(err) => err.to_string(),
		// The test binary cannot serve `octomind workflow`; a success here
		// would mean the spawn contract changed.
		Ok(_) => panic!("expected the workflow spawn to fail"),
	};
	let msg = err.to_string();
	assert!(msg.contains("failed"), "{msg}");
	assert!(msg.contains("--dry-run"), "{msg}");
}

#[tokio::test]
#[serial_test::serial]
async fn list_tap_workflows_reads_the_local_registry() {
	let previous = std::env::var_os("OCTOMIND_DATA_DIR");
	let dir = tempfile::tempdir().expect("temp data dir");
	std::env::set_var("OCTOMIND_DATA_DIR", dir.path());
	// Pre-create the default tap dir so tap loading never tries to clone it.
	let wf_dir = dir
		.path()
		.join("taps")
		.join("muvon")
		.join("octomind-tap")
		.join("workflows");
	std::fs::create_dir_all(&wf_dir).expect("workflows dir");
	std::fs::write(wf_dir.join("hello.toml"), "description = \"d\"\n").expect("workflow");

	let listed = list_tap_workflows().expect("registry readable");
	match previous {
		Some(old) => std::env::set_var("OCTOMIND_DATA_DIR", old),
		None => std::env::remove_var("OCTOMIND_DATA_DIR"),
	}
	assert!(
		listed.iter().any(|w| w["name"] == "hello"),
		"expected hello in {listed:?}"
	);
}
