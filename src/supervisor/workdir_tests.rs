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

// ---------------------------------------------------------------------------
// git_fingerprint(): only the tree's own content may move the hash.
// ---------------------------------------------------------------------------

fn git_in(root: &std::path::Path, args: &[&str]) {
	let out = std::process::Command::new("git")
		.arg("-C")
		.arg(root)
		.args(args)
		.output()
		.expect("git spawns");
	assert!(
		out.status.success(),
		"git {:?}: {}",
		args,
		String::from_utf8_lossy(&out.stderr).trim()
	);
}

/// Move a file's mtime forward without touching a byte of it.
fn bump_mtime(path: &std::path::Path) {
	let modified =
		std::fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(60);
	std::fs::File::options()
		.write(true)
		.open(path)
		.unwrap()
		.set_times(std::fs::FileTimes::new().set_modified(modified))
		.expect("set mtime");
}

/// A repo the test owns outright: staging and rewriting inside the real
/// checkout would move the user's index.
fn temp_repo() -> tempfile::TempDir {
	let tmp = tempfile::tempdir().unwrap();
	git_in(tmp.path(), &["init"]);
	crate::mcp::workdir::set_session_working_directory(tmp.path().to_path_buf());
	tmp
}

/// Staging changes how git FILES a path, not what the tree holds under it.
/// Hashing the porcelain status letters made `git add` read as a mutation, so a
/// turn that staged its work after a passing check was sent back as unverified.
#[test]
#[serial_test::serial]
fn staging_a_file_does_not_move_the_fingerprint() {
	let tmp = temp_repo();
	let file = tmp.path().join("note.md");
	std::fs::write(&file, "content").unwrap();

	let untracked = fingerprint().expect("temp repo fingerprint");
	git_in(tmp.path(), &["add", "note.md"]);
	let staged = fingerprint().expect("temp repo fingerprint");
	std::fs::write(&file, "content, and then some").unwrap();
	let edited = fingerprint().expect("temp repo fingerprint");

	assert_eq!(untracked, staged, "`git add` changes no byte of the tree");
	assert_ne!(staged, edited, "an edit after staging is still observed");
}

/// A tool that rewrites a file with the bytes already in it changed nothing.
/// Folding mtime made that no-op read as a mutation.
#[test]
#[serial_test::serial]
fn an_identical_rewrite_does_not_move_the_fingerprint() {
	let tmp = temp_repo();
	let file = tmp.path().join("note.md");
	std::fs::write(&file, "same bytes").unwrap();

	let first = fingerprint().expect("temp repo fingerprint");
	std::fs::write(&file, "same bytes").unwrap();
	bump_mtime(&file);
	let rewritten = fingerprint().expect("temp repo fingerprint");

	assert_eq!(first, rewritten, "identical bytes are the same tree");
}

/// Above the per-file cap the bytes are not read, so the path falls back to
/// metadata: still moves on a real edit, at the price of moving on an identical
/// rewrite too. The cheap direction to be wrong in.
#[test]
#[serial_test::serial]
fn a_file_above_the_content_cap_folds_by_metadata() {
	let tmp = temp_repo();
	let file = tmp.path().join("big.bin");
	std::fs::write(&file, vec![7u8; CONTENT_BYTE_CAP as usize + 1]).unwrap();

	let first = fingerprint().expect("temp repo fingerprint");
	bump_mtime(&file);
	let touched = fingerprint().expect("temp repo fingerprint");

	assert_ne!(first, touched, "an unread path is tracked by its mtime");
}

/// Deleting a tracked file is a change to the tree, whatever git's index says
/// about it — the path leaves the walk, and the hash must move with it.
#[test]
#[serial_test::serial]
fn a_deletion_moves_the_fingerprint() {
	let tmp = temp_repo();
	let file = tmp.path().join("note.md");
	std::fs::write(&file, "content").unwrap();
	git_in(tmp.path(), &["add", "note.md"]);
	git_in(
		tmp.path(),
		&[
			"-c",
			"user.email=t@example.com",
			"-c",
			"user.name=t",
			"commit",
			"-m",
			"seed",
		],
	);

	let clean = fingerprint().expect("temp repo fingerprint");
	std::fs::remove_file(&file).unwrap();
	let deleted = fingerprint().expect("temp repo fingerprint");

	assert_ne!(clean, deleted, "a removed file is a changed tree");
}
