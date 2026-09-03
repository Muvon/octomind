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

//! Working-tree fingerprint — observational ground truth for "did anything
//! change". Classifying tools by name guesses at effects; measuring the tree
//! observes them, so a change made through ANY tool (an editor, a shell
//! `sed -i`, a sub-agent) is caught identically.

use std::hash::{Hash, Hasher};
use std::path::Path;

/// Entries the directory walk will stat before it gives up. A tree this large
/// cannot be measured on every tool round, and a partial measurement is worse
/// than none — it would report "unchanged" for a subtree it never looked at.
/// Past the cap the walk reports the honest answer, "unknown".
const WALK_ENTRY_CAP: usize = 4096;

/// Hash of the session's working tree, or `None` when the tree cannot be
/// measured — callers then degrade to shape-based signals.
///
/// Measured at the session ANCHOR, not the process cwd and not the agent's
/// current directory: the anchor is the one directory that stays put for the
/// whole session, so a `workdir` switch mid-task cannot masquerade as the tree
/// changing under the agent. The process cwd is never it — nothing in the
/// runtime chdir()s when a session moves, so a session rooted anywhere else was
/// having a different tree measured than the one it was editing.
///
/// Two measurements, strongest first. Under version control, `git status
/// --porcelain -uall` folded with (size, mtime) of every dirty path, so
/// re-modifying an already-dirty file still moves the hash even though its
/// status line does not. Everywhere else — a docs folder, a data directory, any
/// working tree that is not a checkout — a bounded walk of the same anchor.
/// Version control is one way to observe a filesystem, not a precondition for
/// having one.
pub fn fingerprint() -> Option<u64> {
	let root = crate::mcp::workdir::get_thread_original_working_directory();
	git_fingerprint(&root).or_else(|| walk_fingerprint(&root, WALK_ENTRY_CAP))
}

/// Cost: one git spawn plus a stat per dirty file.
fn git_fingerprint(root: &Path) -> Option<u64> {
	let out = match std::process::Command::new("git")
		.arg("-C")
		.arg(root)
		.args(["status", "--porcelain", "-uall"])
		.output()
	{
		Ok(o) => o,
		Err(e) => {
			crate::log_debug!("workdir fingerprint: git spawn failed: {}", e);
			return None;
		}
	};
	if !out.status.success() {
		crate::log_debug!(
			"workdir fingerprint: git status rc={:?}: {}",
			out.status.code(),
			String::from_utf8_lossy(&out.stderr).trim()
		);
		return None;
	}
	let text = String::from_utf8_lossy(&out.stdout);
	let mut h = std::collections::hash_map::DefaultHasher::new();
	text.hash(&mut h);
	for line in text.lines() {
		// porcelain v1: `XY <path>`; renames render as `XY old -> new`.
		let path = line.get(3..).unwrap_or("");
		let path = path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"');
		// Porcelain paths are relative to the repository root, which `-C` made
		// the working directory of git alone — this process may be anywhere.
		if let Ok(md) = std::fs::metadata(root.join(path)) {
			md.len().hash(&mut h);
			if let Ok(mt) = md.modified() {
				mt.hash(&mut h);
			}
		}
	}
	Some(h.finish())
}

/// Fallback for a tree under no version control: hash every entry's relative
/// path, size and mtime. Directory order is not stable across reads, so each
/// level is sorted before hashing. Symlinks are hashed by their own metadata
/// and never followed, so a link cycle cannot trap the walk. Cost: one stat per
/// entry, bounded by `cap`.
fn walk_fingerprint(root: &Path, cap: usize) -> Option<u64> {
	let mut h = std::collections::hash_map::DefaultHasher::new();
	let mut stack = vec![root.to_path_buf()];
	let mut seen = 0usize;
	while let Some(dir) = stack.pop() {
		let mut entries: Vec<_> = match std::fs::read_dir(&dir) {
			Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
			Err(e) => {
				// An unreadable directory is itself a fact about the tree: fold
				// its identity in and keep the rest of the walk observable.
				crate::log_debug!("workdir fingerprint: read_dir {:?}: {}", dir, e);
				dir.to_string_lossy().hash(&mut h);
				continue;
			}
		};
		entries.sort();
		for path in entries {
			seen += 1;
			if seen > cap {
				crate::log_debug!("workdir fingerprint: tree exceeds {} entries", cap);
				return None;
			}
			let Ok(md) = std::fs::symlink_metadata(&path) else {
				continue;
			};
			path.strip_prefix(root)
				.unwrap_or(&path)
				.to_string_lossy()
				.hash(&mut h);
			if md.is_dir() {
				stack.push(path);
				continue;
			}
			md.len().hash(&mut h);
			if let Ok(mt) = md.modified() {
				mt.hash(&mut h);
			}
		}
	}
	Some(h.finish())
}

#[cfg(test)]
#[path = "workdir_tests.rs"]
mod tests;
