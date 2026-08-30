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

// ---------------------------------------------------------------------------
// fingerprint(): degradation and the metadata-mixing path.
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn fingerprint_is_none_when_git_cannot_spawn() {
	let old_path = std::env::var_os("PATH");
	std::env::set_var("PATH", "");
	assert_eq!(fingerprint(), None, "no git binary means no fingerprint");
	match old_path {
		Some(v) => std::env::set_var("PATH", v),
		None => std::env::remove_var("PATH"),
	}
}

#[test]
#[serial_test::serial]
fn fingerprint_is_none_when_git_status_fails() {
	let old_dir = std::env::var_os("GIT_DIR");
	let old_tree = std::env::var_os("GIT_WORK_TREE");
	std::env::set_var("GIT_DIR", "/definitely/not/a/repo");
	std::env::remove_var("GIT_WORK_TREE");
	assert_eq!(
		fingerprint(),
		None,
		"a failing git status means no fingerprint, not a stale one"
	);
	match old_dir {
		Some(v) => std::env::set_var("GIT_DIR", v),
		None => std::env::remove_var("GIT_DIR"),
	}
	match old_tree {
		Some(v) => std::env::set_var("GIT_WORK_TREE", v),
		None => std::env::remove_var("GIT_WORK_TREE"),
	}
}

/// An untracked scratch file at the repo root (not gitignored target/) appears in `git status -uall` of
/// this checkout and exists relative to the process cwd, so its size and
/// mtime are mixed into the hash — and changing it changes the fingerprint.
#[test]
#[serial_test::serial]
fn fingerprint_mixes_file_metadata_and_tracks_changes() {
	let name = format!("workdir-fp-{}.txt", std::process::id());
	std::fs::write(&name, "first content").expect("write scratch file");
	let first = fingerprint();
	let _ = std::fs::remove_file(&name);
	assert!(first.is_some(), "a readable status yields a fingerprint");

	std::fs::write(&name, "much longer second content that changes the size").expect("rewrite");
	let second = fingerprint();
	let _ = std::fs::remove_file(&name);
	assert!(second.is_some());
	assert_ne!(
		first, second,
		"a size/mtime change must move the fingerprint"
	);
}
