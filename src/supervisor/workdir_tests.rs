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

/// Version control is one way to observe a filesystem, not a precondition for
/// having one: a documents folder, a data directory or any other tree an agent
/// works in must be just as observable as a checkout.
#[test]
#[serial_test::serial]
fn fingerprint_observes_a_tree_that_is_not_a_checkout() {
	let original = std::env::current_dir().unwrap();
	let tmp = tempfile::tempdir().unwrap();
	std::env::set_current_dir(tmp.path()).unwrap();
	std::fs::write(tmp.path().join("notes.md"), "first").unwrap();
	let first = fingerprint();
	let stable = fingerprint();
	std::fs::write(tmp.path().join("notes.md"), "second, and longer").unwrap();
	let changed = fingerprint();
	// Restore before asserting so a failure cannot leak the cwd swap.
	std::env::set_current_dir(&original).unwrap();

	assert!(first.is_some(), "a plain directory is still observable");
	assert_eq!(first, stable, "an unchanged tree must hash identically");
	assert_ne!(
		first, changed,
		"an edit outside version control must move the fingerprint"
	);
}

/// The tree that gets measured is the session's anchor. Nothing in the runtime
/// chdir()s when a session moves, so measuring the process cwd watched the
/// wrong tree for every session not rooted where the binary started.
#[test]
#[serial_test::serial]
fn fingerprint_follows_the_session_anchor_not_the_process_cwd() {
	let cwd_before = std::env::current_dir().unwrap();
	let tmp = tempfile::tempdir().unwrap();
	std::fs::write(tmp.path().join("report.txt"), "one").unwrap();
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());

	let first = fingerprint();
	std::fs::write(tmp.path().join("report.txt"), "one, rather longer").unwrap();
	let second = fingerprint();

	assert_eq!(
		std::env::current_dir().unwrap(),
		cwd_before,
		"the process never moved; only the session did"
	);
	assert!(first.is_some());
	assert_ne!(
		first, second,
		"an edit in the session's own tree must be observed"
	);
}

/// A tree too large to measure reports "unknown", never a partial hash — a
/// hash of the half we walked would read as "unchanged" for the half we did not.
#[test]
fn the_walk_reports_unknown_rather_than_measure_part_of_a_tree() {
	let tmp = tempfile::tempdir().unwrap();
	for i in 0..3 {
		std::fs::write(tmp.path().join(format!("f{i}")), "x").unwrap();
	}
	assert!(walk_fingerprint(tmp.path(), 8).is_some());
	assert_eq!(
		walk_fingerprint(tmp.path(), 2),
		None,
		"past the cap the answer is unknown, not clean"
	);
}

// ---------------------------------------------------------------------------
// fingerprint(): degradation and the metadata-mixing path.
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial]
fn a_missing_git_binary_does_not_blind_the_runtime() {
	let original = std::env::current_dir().unwrap();
	let tmp = tempfile::tempdir().unwrap();
	std::fs::write(tmp.path().join("data.csv"), "a,b").unwrap();
	std::env::set_current_dir(tmp.path()).unwrap();
	let old_path = std::env::var_os("PATH");
	std::env::set_var("PATH", "");

	let without_git = fingerprint();

	match old_path {
		Some(v) => std::env::set_var("PATH", v),
		None => std::env::remove_var("PATH"),
	}
	std::env::set_current_dir(&original).unwrap();
	assert!(
		without_git.is_some(),
		"an unspawnable git leaves the filesystem itself observable"
	);
}

#[test]
#[serial_test::serial]
fn a_failing_git_status_degrades_to_the_walk_not_to_a_stale_value() {
	let original = std::env::current_dir().unwrap();
	let tmp = tempfile::tempdir().unwrap();
	std::env::set_current_dir(tmp.path()).unwrap();
	let old_dir = std::env::var_os("GIT_DIR");
	let old_tree = std::env::var_os("GIT_WORK_TREE");
	std::env::set_var("GIT_DIR", "/definitely/not/a/repo");
	std::env::remove_var("GIT_WORK_TREE");

	let broken = fingerprint();
	std::fs::write(tmp.path().join("added.txt"), "x").unwrap();
	let after_edit = fingerprint();

	match old_dir {
		Some(v) => std::env::set_var("GIT_DIR", v),
		None => std::env::remove_var("GIT_DIR"),
	}
	match old_tree {
		Some(v) => std::env::set_var("GIT_WORK_TREE", v),
		None => std::env::remove_var("GIT_WORK_TREE"),
	}
	std::env::set_current_dir(&original).unwrap();
	assert!(
		broken.is_some(),
		"a broken git falls back, it does not blind"
	);
	assert_ne!(
		broken, after_edit,
		"the fallback must track the tree, not cache a value"
	);
}

/// An untracked scratch file at the repo root (not gitignored target/) appears in `git status -uall` of
/// this checkout and resolves against the measured anchor, so its size and
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
