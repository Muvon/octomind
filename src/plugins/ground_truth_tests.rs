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

//! Tests run real git in temporary directories.

use super::*;
use tempfile::TempDir;

const GENEROUS_LIMIT: usize = 10_000;

fn git(dir: &Path, args: &[&str]) {
	let status = std::process::Command::new("git")
		.args([
			"-c",
			"user.name=test",
			"-c",
			"user.email=test@example.com",
			"-c",
			"commit.gpgsign=false",
		])
		.args(args)
		.current_dir(dir)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.status()
		.expect("git runs");
	assert!(status.success(), "git {args:?} failed");
}

/// A repository whose one commit holds `kept.txt` and `gone.txt`.
fn repository() -> TempDir {
	let dir = tempfile::tempdir().unwrap();
	git(dir.path(), &["init", "-q"]);
	std::fs::write(dir.path().join("kept.txt"), "one\ntwo\n").unwrap();
	std::fs::write(dir.path().join("gone.txt"), "bye\n").unwrap();
	git(dir.path(), &["add", "."]);
	git(dir.path(), &["commit", "-q", "-m", "first"]);
	dir
}

fn paths(names: &[&str]) -> Vec<String> {
	names.iter().map(|name| name.to_string()).collect()
}

#[tokio::test]
async fn a_tracked_change_carries_its_diff() {
	let dir = repository();
	std::fs::write(dir.path().join("kept.txt"), "one\nthree\n").unwrap();

	let changes = file_changes(dir.path(), &paths(&["kept.txt"]), Some(GENEROUS_LIMIT)).await;

	assert_eq!(changes.len(), 1);
	assert_eq!(changes[0].path, "kept.txt");
	assert!(changes[0].exists);
	let diff = changes[0].diff.as_deref().expect("a diff");
	assert!(diff.contains("-two") && diff.contains("+three"), "{diff}");
	assert_eq!(changes[0].head, None);
	assert!(!changes[0].truncated);
}

#[tokio::test]
async fn an_untracked_file_is_an_addition() {
	let dir = repository();
	std::fs::write(dir.path().join("fresh.txt"), "brand new\n").unwrap();

	let changes = file_changes(dir.path(), &paths(&["fresh.txt"]), Some(GENEROUS_LIMIT)).await;

	assert!(changes[0].exists);
	let diff = changes[0].diff.as_deref().expect("a diff");
	assert!(diff.contains("+brand new"), "{diff}");
	assert_eq!(changes[0].head, None);
}

#[tokio::test]
async fn a_deleted_file_does_not_exist_and_shows_the_removal() {
	let dir = repository();
	std::fs::remove_file(dir.path().join("gone.txt")).unwrap();

	let changes = file_changes(dir.path(), &paths(&["gone.txt"]), Some(GENEROUS_LIMIT)).await;

	assert!(!changes[0].exists);
	let diff = changes[0].diff.as_deref().expect("a diff");
	assert!(diff.contains("-bye"), "{diff}");
	assert_eq!(changes[0].head, None);
}

#[tokio::test]
async fn an_unchanged_file_carries_its_head() {
	let dir = repository();

	let changes = file_changes(dir.path(), &paths(&["kept.txt"]), Some(GENEROUS_LIMIT)).await;

	assert!(changes[0].exists);
	assert_eq!(changes[0].diff, None);
	assert_eq!(changes[0].head.as_deref(), Some("one\ntwo"));
	assert!(!changes[0].truncated);
}

#[tokio::test]
async fn a_head_stops_at_its_line_limit() {
	let dir = repository();
	let long: String = (0..HEAD_LINES * 2).map(|n| format!("line {n}\n")).collect();
	std::fs::write(dir.path().join("kept.txt"), &long).unwrap();
	git(dir.path(), &["commit", "-q", "-am", "long"]);

	let changes = file_changes(dir.path(), &paths(&["kept.txt"]), Some(GENEROUS_LIMIT)).await;

	let head = changes[0].head.as_deref().expect("a head");
	assert_eq!(head.lines().count(), HEAD_LINES);
	assert!(head.ends_with(&format!("line {}", HEAD_LINES - 1)));
}

#[tokio::test]
async fn a_long_diff_is_cut_at_the_limit_and_marked() {
	let dir = repository();
	std::fs::write(dir.path().join("kept.txt"), "x".repeat(500)).unwrap();

	let changes = file_changes(dir.path(), &paths(&["kept.txt"]), Some(40)).await;

	assert_eq!(changes[0].diff.as_deref().map(|diff| diff.chars().count()), Some(40));
	assert!(changes[0].truncated);
}

#[tokio::test]
async fn a_cut_never_splits_a_character() {
	let dir = repository();
	std::fs::write(dir.path().join("kept.txt"), "é".repeat(400)).unwrap();

	// The limit lands inside the run of two-byte characters, past the diff header.
	let changes = file_changes(dir.path(), &paths(&["kept.txt"]), Some(300)).await;

	assert_eq!(changes[0].diff.as_deref().map(|diff| diff.chars().count()), Some(300));
	assert!(changes[0].truncated);
}

#[tokio::test]
async fn without_a_limit_only_existence_is_reported() {
	let dir = repository();
	std::fs::write(dir.path().join("kept.txt"), "changed\n").unwrap();
	std::fs::remove_file(dir.path().join("gone.txt")).unwrap();

	let changes = file_changes(dir.path(), &paths(&["kept.txt", "gone.txt"]), None).await;

	assert_eq!(changes.len(), 2);
	assert!(changes[0].exists);
	assert!(!changes[1].exists);
	for change in &changes {
		assert_eq!((&change.diff, &change.head, change.truncated), (&None, &None, false));
	}
}

#[tokio::test]
async fn an_absolute_path_is_read_as_it_is() {
	let dir = repository();
	let absolute = dir.path().join("kept.txt");
	std::fs::write(&absolute, "one\nchanged\n").unwrap();

	let changes = file_changes(
		dir.path(),
		&[absolute.to_string_lossy().into_owned()],
		Some(GENEROUS_LIMIT),
	)
	.await;

	assert!(changes[0].exists);
	assert!(changes[0].diff.as_deref().expect("a diff").contains("+changed"));
}

#[tokio::test]
async fn outside_a_repository_a_file_has_a_head_and_no_diff() {
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("note.txt"), "hello\n").unwrap();

	let changes = file_changes(dir.path(), &paths(&["note.txt", "absent.txt"]), Some(GENEROUS_LIMIT)).await;

	assert_eq!(changes[0].diff, None);
	assert_eq!(changes[0].head.as_deref(), Some("hello"));
	assert!(!changes[1].exists);
	assert_eq!((&changes[1].diff, &changes[1].head), (&None, &None));
}

#[tokio::test]
async fn tree_status_lists_porcelain_lines() {
	let dir = repository();
	std::fs::write(dir.path().join("kept.txt"), "changed\n").unwrap();
	std::fs::write(dir.path().join("fresh.txt"), "new\n").unwrap();

	let mut lines = tree_status(dir.path()).await;
	lines.sort();

	assert_eq!(lines, vec![" M kept.txt", "?? fresh.txt"]);
}

#[tokio::test]
async fn tree_status_is_capped() {
	let dir = repository();
	for n in 0..TREE_STATUS_MAX_LINES + 10 {
		std::fs::write(dir.path().join(format!("extra-{n}.txt")), "x\n").unwrap();
	}

	assert_eq!(tree_status(dir.path()).await.len(), TREE_STATUS_MAX_LINES);
}

#[tokio::test]
async fn tree_status_is_empty_outside_a_work_tree() {
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join("note.txt"), "hello\n").unwrap();

	assert!(tree_status(dir.path()).await.is_empty());
}

#[test]
fn cutting_text_that_fits_changes_nothing() {
	assert_eq!(cut_chars(Some("abc".to_string()), 3), (Some("abc".to_string()), false));
	assert_eq!(cut_chars(Some("abcd".to_string()), 3), (Some("abc".to_string()), true));
	assert_eq!(cut_chars(None, 3), (None, false));
}
