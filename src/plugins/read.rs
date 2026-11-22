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

//! Reads project files for a plugin that asked for them with `read`. Only
//! regular text files inside the Git top-level of the working directory that
//! Git does not ignore are read; every other file is reported with the reason
//! it was left out.

use super::ground_truth::run_git;
use super::protocol::{FileText, PluginOptions};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Most files sent for one answer, whatever the plugin declares.
pub const READ_MAX_FILES: usize = 200;
/// Largest file read, whatever the plugin declares.
pub const READ_MAX_FILE_BYTES: usize = 65536;

const OUTSIDE: &str = "outside";
const MISSING: &str = "missing";
const IGNORED: &str = "ignored";
const TOO_LARGE: &str = "too_large";
const NOT_TEXT: &str = "not_text";
const VERBATIM_PREFIX: &str = r"\\?\";

/// The limits of one plugin: what it declared, capped by the host's own.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
	max_files: usize,
	max_file_bytes: usize,
}

impl Limits {
	pub fn of(options: &PluginOptions) -> Self {
		Self {
			max_files: options
				.read_max_files
				.map_or(READ_MAX_FILES, |own| own.min(READ_MAX_FILES)),
			max_file_bytes: options
				.read_max_file_bytes
				.map_or(READ_MAX_FILE_BYTES, |own| own.min(READ_MAX_FILE_BYTES)),
		}
	}
}

/// What the host knew of a file when it sent it, to tell later whether it
/// changed. A file left out as too large or not text has an empty hash.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamp {
	size: u64,
	modified: Option<SystemTime>,
	hash: String,
}

/// An entry with a wildcard is a glob; any other entry is one path.
pub fn is_glob(entry: &str) -> bool {
	entry.contains(['*', '?', '['])
}

/// Read what `entries` name and record a stamp for every file read.
pub async fn read_entries(
	workdir: &Path,
	entries: &[String],
	stamps: &mut BTreeMap<String, Stamp>,
	limits: Limits,
) -> Vec<FileText> {
	let tree = Tree::of(workdir).await;
	// `(path, whether Git is asked if it ignores the path)`, in the order given.
	let mut wanted: Vec<(String, bool)> = Vec::new();
	let mut texts = Vec::new();
	for entry in entries {
		let found = if is_glob(entry) {
			tree.expand(entry).await.into_iter().map(|path| (path, false)).collect()
		} else {
			match normalize(entry) {
				Some(path) => vec![(path, true)],
				None => {
					texts.push(skipped(entry, OUTSIDE));
					continue;
				}
			}
		};
		for (path, check) in found {
			if !wanted.iter().any(|(known, _)| *known == path) {
				wanted.push((path, check));
			}
		}
	}
	// Refused entries count against the limit like the files read.
	wanted.truncate(limits.max_files.saturating_sub(texts.len()));
	for (path, check) in wanted {
		let (text, stamp) = tree.read_one(&path, check, limits).await;
		if let Some(stamp) = stamp {
			stamps.insert(path, stamp);
		}
		texts.push(text);
	}
	texts
}

/// Compare every stamped file with the disk and return the texts of those whose
/// content changed. A file that is gone comes back once as `missing`. With
/// `globs`, files they match that were not sent yet come back too.
pub async fn refresh(
	workdir: &Path,
	stamps: &mut BTreeMap<String, Stamp>,
	globs: &[String],
	limits: Limits,
) -> Vec<FileText> {
	let mut texts = Vec::new();
	let mut tree: Option<Tree> = None;
	for (path, stamp) in stamps.clone() {
		let on_disk = tokio::fs::metadata(workdir.join(&path))
			.await
			.ok()
			.filter(|meta| meta.is_file());
		let Some(meta) = on_disk else {
			texts.push(skipped(&path, MISSING));
			stamps.remove(&path);
			continue;
		};
		if meta.len() == stamp.size && meta.modified().ok() == stamp.modified {
			continue;
		}
		if tree.is_none() {
			tree = Some(Tree::of(workdir).await);
		}
		let current = tree.as_ref().expect("the tree was just set");
		let (text, now) = current.read_one(&path, true, limits).await;
		match now {
			Some(now) => {
				let changed = now.hash != stamp.hash;
				stamps.insert(path, now);
				if changed {
					texts.push(text);
				}
			}
			None => {
				texts.push(text);
				stamps.remove(&path);
			}
		}
	}
	if globs.is_empty() {
		return texts;
	}
	if tree.is_none() {
		tree = Some(Tree::of(workdir).await);
	}
	let tree = tree.as_ref().expect("the tree was just set");
	let mut fresh: Vec<String> = Vec::new();
	for glob in globs {
		for path in tree.expand(glob).await {
			if !stamps.contains_key(&path) && !fresh.contains(&path) {
				fresh.push(path);
			}
		}
	}
	fresh.truncate(limits.max_files);
	for path in fresh {
		// A match that cannot be read would come again with every prompt, so only stamped ones are sent.
		if let (text, Some(stamp)) = tree.read_one(&path, false, limits).await {
			stamps.insert(path, stamp);
			texts.push(text);
		}
	}
	texts
}

fn skipped(path: &str, reason: &str) -> FileText {
	FileText {
		path: path.to_string(),
		hash: String::new(),
		text: None,
		skipped: Some(reason.to_string()),
	}
}

/// `entry` as a path relative to the working directory with `/` separators, with
/// `.` and `..` resolved by name. `None` for an absolute path or one that starts with `~`.
fn normalize(entry: &str) -> Option<String> {
	if entry.starts_with('~') || Path::new(entry).is_absolute() {
		return None;
	}
	let mut parents = 0;
	let mut names: Vec<String> = Vec::new();
	for component in Path::new(entry).components() {
		match component {
			Component::Normal(name) => names.push(name.to_string_lossy().into_owned()),
			Component::ParentDir => {
				if names.pop().is_none() {
					parents += 1;
				}
			}
			Component::CurDir => {}
			_ => return None,
		}
	}
	Some(format!("{}{}", "../".repeat(parents), names.join("/")))
}

/// `relative` below `base`, with `..` resolved by name and no disk access.
fn resolved(base: &Path, relative: &str) -> PathBuf {
	let mut path = base.to_path_buf();
	for component in Path::new(relative).components() {
		match component {
			Component::ParentDir => {
				path.pop();
			}
			Component::Normal(name) => path.push(name),
			_ => {}
		}
	}
	path
}

/// `path` without the `\\?\` that `canonicalize` puts before a drive letter on
/// Windows: the glob crate matches nothing below a path that starts with it.
fn without_verbatim_prefix(path: &Path) -> PathBuf {
	match path.to_string_lossy().strip_prefix(VERBATIM_PREFIX) {
		Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
		_ => path.to_path_buf(),
	}
}

/// Where a session's files are: the working directory and the Git top-level above it.
struct Tree {
	workdir: PathBuf,
	/// The Git top-level; the working directory itself outside a repository.
	root: PathBuf,
	in_repository: bool,
}

impl Tree {
	async fn of(workdir: &Path) -> Self {
		let top = match run_git(workdir, &["rev-parse", "--show-toplevel"]).await {
			Some(output) if output.status.success() => {
				Some(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
			}
			_ => None,
		};
		let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
		Self {
			workdir: canonical(workdir),
			in_repository: top.is_some(),
			root: canonical(top.as_deref().unwrap_or(workdir)),
		}
	}

	/// The files a glob matches, as paths relative to the working directory.
	/// In a repository, Git lists them so ignored files never match.
	async fn expand(&self, glob: &str) -> Vec<String> {
		if self.in_repository {
			let pathspec = format!(":(glob){glob}");
			let args = [
				"ls-files",
				"-z",
				"--cached",
				"--others",
				"--exclude-standard",
				"--",
				pathspec.as_str(),
			];
			let Some(output) = run_git(&self.workdir, &args)
				.await
				.filter(|output| output.status.success())
			else {
				return Vec::new();
			};
			return String::from_utf8_lossy(&output.stdout)
				.split('\0')
				.filter(|path| !path.is_empty())
				.map(str::to_string)
				.collect();
		}
		let workdir = without_verbatim_prefix(&self.workdir);
		let pattern = format!("{}/{glob}", glob::Pattern::escape(&workdir.to_string_lossy()));
		tokio::task::spawn_blocking(move || {
			let Ok(paths) = glob::glob(&pattern) else {
				return Vec::new();
			};
			paths
				.filter_map(Result::ok)
				.filter(|path| path.is_file())
				.filter_map(|path| {
					let relative = path.strip_prefix(&workdir).ok()?;
					Some(relative.to_string_lossy().replace('\\', "/"))
				})
				.collect()
		})
		.await
		.unwrap_or_default()
	}

	/// Read one path. `check_ignore` asks Git whether it ignores the file; a
	/// glob match needs no such check. The stamp is `None` when nothing was
	/// found to stamp.
	async fn read_one(&self, path: &str, check_ignore: bool, limits: Limits) -> (FileText, Option<Stamp>) {
		let outside = || (skipped(path, OUTSIDE), None);
		let missing = || (skipped(path, MISSING), None);
		if !resolved(&self.workdir, path).starts_with(&self.root) {
			return outside();
		}
		let Ok(canonical) = tokio::fs::canonicalize(self.workdir.join(path)).await else {
			return missing();
		};
		// A symlink may lead out of the repository.
		if !canonical.starts_with(&self.root) {
			return outside();
		}
		let Ok(meta) = tokio::fs::metadata(&canonical).await else {
			return missing();
		};
		if !meta.is_file() {
			return missing();
		}
		if check_ignore && self.in_repository && self.ignored(path).await {
			return (skipped(path, IGNORED), None);
		}
		let stamp = |hash: String| Stamp {
			size: meta.len(),
			modified: meta.modified().ok(),
			hash,
		};
		if meta.len() > limits.max_file_bytes as u64 {
			return (skipped(path, TOO_LARGE), Some(stamp(String::new())));
		}
		let Ok(bytes) = tokio::fs::read(&canonical).await else {
			return missing();
		};
		let hash = hex::encode(Sha256::digest(&bytes));
		match String::from_utf8(bytes) {
			Ok(text) => (
				FileText {
					path: path.to_string(),
					hash: hash.clone(),
					text: Some(text),
					skipped: None,
				},
				Some(stamp(hash)),
			),
			Err(_) => (skipped(path, NOT_TEXT), Some(stamp(String::new()))),
		}
	}

	async fn ignored(&self, path: &str) -> bool {
		run_git(&self.workdir, &["check-ignore", "-q", "--", path])
			.await
			.is_some_and(|output| output.status.success())
	}
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
