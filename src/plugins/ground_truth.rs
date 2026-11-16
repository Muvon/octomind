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

//! What is on disk for the files a turn changed, in the wire form of the
//! plugin protocol.

use super::protocol::FileChange;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Git answers in milliseconds; this only stops a stuck repository (a lock, a
/// network file system) from holding the end of a turn.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Lines of an unchanged file sent as its `head`.
const HEAD_LINES: usize = 80;

/// Lines of `git status --porcelain` sent with a turn's end.
const TREE_STATUS_MAX_LINES: usize = 40;

/// One entry per path, in the given order. Paths are as the caller has them:
/// absolute, or relative to `workdir`. Without `diff_chars` an entry only
/// says whether the file exists. With it, a changed file carries its `diff`
/// against the last commit and any other existing file its first lines as
/// `head`; both are cut at `diff_chars` characters.
pub async fn file_changes(workdir: &Path, paths: &[String], diff_chars: Option<usize>) -> Vec<FileChange> {
	let mut changes = Vec::with_capacity(paths.len());
	for path in paths {
		let exists = tokio::fs::try_exists(workdir.join(path)).await.unwrap_or(false);
		let mut change = FileChange {
			path: path.clone(),
			exists,
			diff: None,
			head: None,
			truncated: false,
		};
		if let Some(limit) = diff_chars {
			let diff = diff_of(workdir, path).await;
			let head = match (&diff, exists) {
				(None, true) => head_of(&workdir.join(path)).await,
				_ => None,
			};
			let (diff, diff_cut) = cut_chars(diff, limit);
			let (head, head_cut) = cut_chars(head, limit);
			change.diff = diff;
			change.head = head;
			change.truncated = diff_cut || head_cut;
		}
		changes.push(change);
	}
	changes
}

/// Lines of `git status --porcelain`, at most [`TREE_STATUS_MAX_LINES`].
/// Empty outside a work tree and when git fails.
pub async fn tree_status(workdir: &Path) -> Vec<String> {
	let Some(output) = run_git(workdir, &["status", "--porcelain"]).await else {
		return Vec::new();
	};
	if !output.status.success() {
		return Vec::new();
	}
	String::from_utf8_lossy(&output.stdout)
		.lines()
		.take(TREE_STATUS_MAX_LINES)
		.map(str::to_string)
		.collect()
}

/// Fingerprint of the working tree now, as the text a plugin compares.
pub fn tree_fingerprint() -> Option<String> {
	super::workdir::fingerprint().map(|fingerprint| fingerprint.to_string())
}

/// Run git in `workdir`. `None` when git is missing, fails to start or runs
/// out of time; the child is killed when the timeout drops it.
pub(super) async fn run_git(workdir: &Path, args: &[&str]) -> Option<Output> {
	let mut command = Command::new("git");
	command
		.args(args)
		.current_dir(workdir)
		.stdin(Stdio::null())
		.kill_on_drop(true);
	tokio::time::timeout(GIT_TIMEOUT, command.output()).await.ok()?.ok()
}

/// The file's diff against the last commit, or its whole content as an
/// addition when git does not track it yet. `None` for an unchanged or
/// ignored file and whenever git fails.
async fn diff_of(workdir: &Path, path: &str) -> Option<String> {
	let tracked = run_git(workdir, &["diff", "HEAD", "--", path]).await?;
	if tracked.status.success() && !tracked.stdout.is_empty() {
		return Some(String::from_utf8_lossy(&tracked.stdout).into_owned());
	}
	untracked_diff(workdir, path).await
}

/// A file git neither tracks nor ignores, diffed against nothing.
/// `git diff --no-index` exits with 1 when its inputs differ.
async fn untracked_diff(workdir: &Path, path: &str) -> Option<String> {
	let listing = run_git(workdir, &["ls-files", "--others", "--exclude-standard", "--", path]).await?;
	if !listing.status.success() || listing.stdout.is_empty() {
		return None;
	}
	let diff = run_git(workdir, &["diff", "--no-index", "--", "/dev/null", path]).await?;
	(diff.status.code() == Some(1) && !diff.stdout.is_empty())
		.then(|| String::from_utf8_lossy(&diff.stdout).into_owned())
}

/// The first [`HEAD_LINES`] lines of a text file. `None` for a missing,
/// empty or unreadable file; reading stops at the first line that is not
/// valid text.
async fn head_of(file: &Path) -> Option<String> {
	let file = tokio::fs::File::open(file).await.ok()?;
	let mut lines = BufReader::new(file).lines();
	let mut head = Vec::new();
	while head.len() < HEAD_LINES {
		match lines.next_line().await {
			Ok(Some(line)) => head.push(line),
			_ => break,
		}
	}
	(!head.is_empty()).then(|| head.join("\n"))
}

/// `text` cut at `limit` characters, and whether it was cut.
fn cut_chars(text: Option<String>, limit: usize) -> (Option<String>, bool) {
	let Some(mut text) = text else {
		return (None, false);
	};
	match text.char_indices().nth(limit) {
		Some((byte_index, _)) => {
			text.truncate(byte_index);
			(Some(text), true)
		}
		None => (Some(text), false),
	}
}

#[cfg(test)]
#[path = "ground_truth_tests.rs"]
mod tests;
