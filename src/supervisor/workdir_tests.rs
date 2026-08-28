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
#[serial_test::serial]
fn fingerprint_is_some_and_deterministic_inside_git_repo() {
	// The test binary runs from the crate root, which is a git checkout.
	let first = fingerprint();
	let second = fingerprint();
	assert!(first.is_some(), "expected a fingerprint inside the repo");
	assert_eq!(first, second, "an unchanged tree must hash identically");
}

#[test]
#[serial_test::serial]
fn fingerprint_moves_when_the_tree_changes_and_back() {
	let baseline = fingerprint();
	let probe = std::env::current_dir()
		.unwrap()
		.join(".octomind_fingerprint_probe");
	std::fs::write(&probe, b"probe").unwrap();
	let dirty = fingerprint();
	std::fs::remove_file(&probe).unwrap();
	let restored = fingerprint();

	let baseline = baseline.expect("repo fingerprint");
	let dirty = dirty.expect("repo fingerprint");
	let restored = restored.expect("repo fingerprint");
	assert_ne!(
		baseline, dirty,
		"an untracked file must move the fingerprint"
	);
	assert_eq!(
		baseline, restored,
		"removing the probe must restore the baseline"
	);
}

#[test]
#[serial_test::serial]
fn fingerprint_is_none_outside_a_git_repo() {
	let original = std::env::current_dir().unwrap();
	let tmp = tempfile::tempdir().unwrap();
	std::env::set_current_dir(tmp.path()).unwrap();
	let outside = fingerprint();
	// Restore before asserting so a failure cannot leak the cwd swap.
	std::env::set_current_dir(&original).unwrap();
	assert_eq!(outside, None, "git status must fail outside a repo");
}
