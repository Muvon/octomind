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
fn test_is_dotfile_or_in_dot_directory() {
	// Regular files should not be filtered
	assert!(!is_dotfile_or_in_dot_directory("src/main.rs"));
	assert!(!is_dotfile_or_in_dot_directory(
		"ui/components/Button.svelte"
	));
	assert!(!is_dotfile_or_in_dot_directory("README.md"));

	// Dotfiles should be filtered
	assert!(is_dotfile_or_in_dot_directory(".env"));
	assert!(is_dotfile_or_in_dot_directory(".gitignore"));
	assert!(is_dotfile_or_in_dot_directory(".eslintrc.json"));

	// Files in dot directories should be filtered
	assert!(is_dotfile_or_in_dot_directory(".git/config"));
	assert!(is_dotfile_or_in_dot_directory(".vscode/settings.json"));
	assert!(is_dotfile_or_in_dot_directory("src/.hidden/file.rs"));
	assert!(is_dotfile_or_in_dot_directory(".github/workflows/ci.yml"));

	// Current and parent directory references should not be filtered
	assert!(!is_dotfile_or_in_dot_directory("."));
	assert!(!is_dotfile_or_in_dot_directory(".."));
	assert!(!is_dotfile_or_in_dot_directory("./src/main.rs"));
	assert!(!is_dotfile_or_in_dot_directory("../other/file.rs"));
}

/// Scratch directory with a NON-dot name inside the crate working dir.
/// tempfile::TempDir dirs are dot-named (`.tmpXXX`), which switches the
/// module's dotfile filtering OFF — these tests need it ON, so build a
/// plain-named dir and clean it up on drop. Living under the system temp
/// dir also keeps untracked scratch out of the repo working tree, which
/// other tests hash (git fingerprint).
struct ScratchDir(std::path::PathBuf);

impl Drop for ScratchDir {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.0);
	}
}

fn scratch(tag: &str) -> (ScratchDir, String) {
	let nanos = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let name = format!("glob-test-{tag}-{}-{nanos}", std::process::id());
	let path = std::env::temp_dir().join(&name);
	std::fs::create_dir_all(&path).expect("create scratch dir");
	let dir = path.display().to_string();
	(ScratchDir(path), dir)
}

fn write_file(dir: &str, rel: &str, contents: &str) {
	let path = std::path::Path::new(dir).join(rel);
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).expect("create parent dir");
	}
	std::fs::write(path, contents).expect("write scratch file");
}

fn file_names(paths: &[String]) -> Vec<String> {
	paths
		.iter()
		.map(|p| {
			std::path::Path::new(p)
				.file_name()
				.and_then(|name| name.to_str())
				.unwrap_or(p)
				.to_string()
		})
		.collect()
}

#[test]
fn test_expand_empty_patterns_returns_empty() {
	let (_guard, dir) = scratch("empty");
	let result =
		expand_glob_patterns_filtered(&[], Some(&dir)).expect("empty patterns must succeed");
	assert!(result.is_empty(), "expected no files, got {result:?}");
}

#[test]
fn test_expand_star_glob_matches_files_recursively() {
	// glob::Pattern::matches defaults to require_literal_separator=false,
	// so `*` crosses `/` — `*.rs` picks up nested files too.
	let (_guard, dir) = scratch("star");
	write_file(&dir, "f1.rs", "fn a() {}");
	write_file(&dir, "note.txt", "text");
	write_file(&dir, "sub/f2.rs", "fn b() {}");

	let patterns = vec!["*.rs".to_string()];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	let mut names = file_names(&result);
	names.sort();
	assert_eq!(names, vec!["f1.rs", "f2.rs"], "full paths: {result:?}");
}

#[test]
fn test_expand_question_mark_pattern_matches_single_char() {
	let (_guard, dir) = scratch("question");
	write_file(&dir, "ab.rs", "");
	write_file(&dir, "ac.rs", "");
	write_file(&dir, "ad.txt", "");

	// Patterns match FULL paths — `?` must not eat the scratch-dir
	// prefix, so anchor with `**/`.
	let patterns = vec!["**/a?.rs".to_string()];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	let mut names = file_names(&result);
	names.sort();
	assert_eq!(names, vec!["ab.rs", "ac.rs"], "full paths: {result:?}");
}

#[test]
fn test_expand_character_class_pattern_matches_members() {
	let (_guard, dir) = scratch("class");
	write_file(&dir, "a.rs", "");
	write_file(&dir, "c.rs", "");

	// Full-path matching: anchor the class so it applies to the basename.
	let patterns = vec!["**/[ab].rs".to_string()];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	assert_eq!(file_names(&result), vec!["a.rs"], "full paths: {result:?}");
}

#[test]
fn test_expand_respects_gitignore_rules() {
	let (_guard, dir) = scratch("gitignore");
	write_file(&dir, ".gitignore", "ignored.rs\n");
	write_file(&dir, "kept.rs", "");
	write_file(&dir, "ignored.rs", "");

	let patterns = vec!["*.rs".to_string()];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	assert_eq!(
		file_names(&result),
		vec!["kept.rs"],
		"gitignored files must be excluded: {result:?}"
	);
}

#[test]
fn test_expand_filters_dotfiles_in_regular_directory() {
	let (_guard, dir) = scratch("dotfilter");
	write_file(&dir, "visible.rs", "");
	write_file(&dir, ".hidden.rs", "");
	write_file(&dir, ".hiddendir/inner.rs", "");

	let patterns = vec!["*.rs".to_string()];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	assert_eq!(
		file_names(&result),
		vec!["visible.rs"],
		"dotfiles and dot-dir contents must be filtered: {result:?}"
	);
}

#[test]
fn test_expand_keeps_dotfiles_when_search_dir_is_dot_named() {
	// tempfile dirs are `.tmpXXX` — the module deliberately skips dotfile
	// filtering there, otherwise nothing inside a temp dir would ever match.
	let tmp = tempfile::TempDir::new().expect("create temp dir");
	let dir = tmp.path().to_string_lossy().to_string();
	std::fs::write(tmp.path().join("visible.rs"), "").expect("write");
	std::fs::write(tmp.path().join(".hidden.rs"), "").expect("write");

	let patterns = vec!["*.rs".to_string()];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	let mut names = file_names(&result);
	names.sort();
	assert_eq!(
		names,
		vec![".hidden.rs", "visible.rs"],
		"dot-named search dir must disable dotfile filtering: {result:?}"
	);
}

#[test]
fn test_expand_absolute_double_star_pattern_extracts_base_dir() {
	let tmp = tempfile::TempDir::new().expect("create temp dir");
	std::fs::write(tmp.path().join("f1.rs"), "").expect("write");
	std::fs::create_dir_all(tmp.path().join("sub")).expect("mkdir");
	std::fs::write(tmp.path().join("sub/f2.rs"), "").expect("write");

	let pattern = format!("{}/sub/**/*.rs", tmp.path().display());
	let result = expand_glob_patterns_filtered(&[pattern], None).expect("expansion must succeed");
	assert_eq!(
		file_names(&result),
		vec!["f2.rs"],
		"base dir must be extracted from the absolute `**` pattern: {result:?}"
	);
}

#[test]
fn test_expand_absolute_single_star_pattern_extracts_base_dir() {
	let tmp = tempfile::TempDir::new().expect("create temp dir");
	std::fs::write(tmp.path().join("f1.rs"), "").expect("write");
	std::fs::create_dir_all(tmp.path().join("sub")).expect("mkdir");
	std::fs::write(tmp.path().join("sub/f2.rs"), "").expect("write");

	let pattern = format!("{}/sub/*.rs", tmp.path().display());
	let result = expand_glob_patterns_filtered(&[pattern], None).expect("expansion must succeed");
	assert_eq!(
		file_names(&result),
		vec!["f2.rs"],
		"base dir must be extracted from the absolute `*` pattern: {result:?}"
	);
}

#[test]
fn test_expand_plain_file_path_returns_itself() {
	let (_guard, dir) = scratch("plainfile");
	write_file(&dir, "f1.rs", "content");

	let pattern = format!("{dir}/f1.rs");
	let result = expand_glob_patterns_filtered(std::slice::from_ref(&pattern), Some(&dir))
		.expect("expansion must succeed");
	assert_eq!(result, vec![pattern], "direct file path must pass through");
}

#[test]
fn test_expand_plain_dotfile_path_is_filtered() {
	let (_guard, dir) = scratch("plaindot");
	write_file(&dir, ".hidden.rs", "");

	let pattern = format!("{dir}/.hidden.rs");
	let result =
		expand_glob_patterns_filtered(&[pattern], Some(&dir)).expect("expansion must succeed");
	assert!(
		result.is_empty(),
		"dotfile direct path must be dropped: {result:?}"
	);
}

#[test]
fn test_expand_plain_directory_path_adds_contained_files() {
	let (_guard, dir) = scratch("plaindir");
	write_file(&dir, "f1.rs", "");
	write_file(&dir, "note.txt", "");
	write_file(&dir, "sub/f2.rs", "");

	let all = expand_glob_patterns_filtered(std::slice::from_ref(&dir), Some(&dir))
		.expect("expansion must succeed");
	assert_eq!(all.len(), 3, "dir pattern must add every file: {all:?}");

	let sub = expand_glob_patterns_filtered(&[format!("{dir}/sub")], Some(&dir))
		.expect("expansion must succeed");
	assert_eq!(
		file_names(&sub),
		vec!["f2.rs"],
		"subdir pattern must add only its files: {sub:?}"
	);
}

#[test]
fn test_expand_missing_plain_path_is_ignored() {
	let (_guard, dir) = scratch("missing");
	let result =
		expand_glob_patterns_filtered(&["no-such-file-anywhere.rs".to_string()], Some(&dir))
			.expect("expansion must succeed");
	assert!(
		result.is_empty(),
		"missing path must be skipped: {result:?}"
	);
}

#[test]
fn test_expand_invalid_glob_pattern_returns_error() {
	let (_guard, dir) = scratch("invalid");
	let err = expand_glob_patterns_filtered(&["[".to_string()], Some(&dir))
		.expect_err("unclosed character class must fail");
	assert!(
		err.to_string().contains("Invalid glob pattern"),
		"unexpected error: {err}"
	);
}

#[test]
fn test_expand_dedupes_overlapping_patterns() {
	let (_guard, dir) = scratch("dedup");
	write_file(&dir, "f1.rs", "");

	let patterns = vec!["*.rs".to_string(), format!("{dir}/f1.rs")];
	let result =
		expand_glob_patterns_filtered(&patterns, Some(&dir)).expect("expansion must succeed");
	assert_eq!(
		result.len(),
		1,
		"overlapping patterns must dedup: {result:?}"
	);
}

#[test]
fn test_expand_over_file_limit_returns_error() {
	let (_guard, dir) = scratch("limit");
	for i in 0..=1000 {
		write_file(&dir, &format!("f{i}.txt"), "");
	}
	let err = expand_glob_patterns_filtered(&["*".to_string()], Some(&dir))
		.expect_err("1001 files must exceed the expansion limit");
	assert!(
		err.to_string().contains("Too many files"),
		"unexpected error: {err}"
	);
}
