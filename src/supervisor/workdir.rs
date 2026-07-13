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

/// Hash of the working tree's dirty state: the `git status --porcelain -uall`
/// output folded with (size, mtime) of every dirty path — so re-modifying an
/// already-dirty file still moves the fingerprint even though its status line
/// is unchanged. `None` outside a git repo (or git missing/failing); callers
/// degrade to shape-based signals. Cost: one git spawn plus a stat per dirty
/// file — run at most once per tool round.
pub fn fingerprint() -> Option<u64> {
	let out = std::process::Command::new("git")
		.args(["status", "--porcelain", "-uall"])
		.output()
		.ok()?;
	if !out.status.success() {
		return None;
	}
	let text = String::from_utf8_lossy(&out.stdout);
	let mut h = std::collections::hash_map::DefaultHasher::new();
	text.hash(&mut h);
	for line in text.lines() {
		// porcelain v1: `XY <path>`; renames render as `XY old -> new`.
		let path = line.get(3..).unwrap_or("");
		let path = path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"');
		if let Ok(md) = std::fs::metadata(path) {
			md.len().hash(&mut h);
			if let Ok(mt) = md.modified() {
				mt.hash(&mut h);
			}
		}
	}
	Some(h.finish())
}
