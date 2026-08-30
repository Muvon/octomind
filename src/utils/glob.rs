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

// Gitignore-aware glob pattern expansion utilities

use anyhow::{anyhow, Result};
use ignore::WalkBuilder;
use std::path::Path;

/// Maximum number of files allowed after glob expansion to prevent command line overflow
const MAX_EXPANDED_FILES: usize = 1000;

/// Expand glob patterns to actual file paths with gitignore and dotfile filtering
///
/// This function provides intelligent file expansion that:
/// - Respects .gitignore rules using the ignore crate
/// - Automatically excludes dotfiles (files/directories starting with '.')
/// - Applies glob pattern matching to filtered results
/// - Enforces file count limits to prevent system overload
///
/// # Arguments
/// * `patterns` - Array of glob patterns to expand
/// * `base_dir` - Base directory to search from (defaults to current directory)
///
/// # Returns
/// * `Ok(Vec<String>)` - List of expanded file paths
/// * `Err(anyhow::Error)` - If expansion fails or too many files found
pub fn expand_glob_patterns_filtered(
	patterns: &[String],
	base_dir: Option<&str>,
) -> Result<Vec<String>> {
	let mut expanded_paths = Vec::new();

	// Determine the search directory
	// If base_dir is provided, use it
	// Otherwise, try to extract base directory from the first glob pattern with absolute path
	let search_dir = if let Some(dir) = base_dir {
		dir.to_string()
	} else {
		// Try to find a base directory from patterns with absolute paths
		let mut extracted_base = None;
		for pattern in patterns {
			// Check if this is an absolute path (Unix: starts with '/', Windows: starts with drive letter or UNC)
			let is_absolute = pattern.starts_with('/')
				|| (cfg!(windows)
					&& (
						// Windows drive letter: C:\, D:\, etc.
						(pattern.len() >= 3 && pattern.chars().nth(1) == Some(':') && (pattern.chars().nth(2) == Some('\\') || pattern.chars().nth(2) == Some('/')))
					// Windows UNC path: \\server\share
					|| pattern.starts_with("\\\\")
					));

			if is_absolute {
				// Extract the base directory from absolute path pattern
				// For patterns like "/path/to/dir/**/*.rs" or "C:\path\to\dir\**\*.rs", extract the base directory
				if let Some(glob_start) = pattern.find("**") {
					// Get everything before the **
					let base = &pattern[..glob_start];
					// Remove trailing slash/backslash if present
					let base = base.trim_end_matches('/').trim_end_matches('\\');
					if !base.is_empty() {
						extracted_base = Some(base.to_string());
						break;
					}
				} else if let Some(glob_start) = pattern.find('*') {
					// For patterns like "/path/to/*.rs" or "C:\path\to\*.rs", extract the directory
					let base = &pattern[..glob_start];
					// Get the directory part (handle both / and \ separators)
					let last_separator = base.rfind('/').or_else(|| base.rfind('\\'));
					if let Some(last_sep) = last_separator {
						let base = &base[..last_sep];
						if !base.is_empty() {
							extracted_base = Some(base.to_string());
							break;
						}
					}
				}
			}
		}
		extracted_base.unwrap_or_else(|| ".".to_string())
	};

	crate::log_debug!(
		"Expanding {} glob patterns from directory '{}': {:?}",
		patterns.len(),
		search_dir,
		patterns
	);

	// Build ignore walker that respects .gitignore and excludes dotfiles
	let mut builder = WalkBuilder::new(&search_dir);
	builder
		.hidden(false) // Don't automatically skip hidden files (we'll filter manually)
		.git_ignore(true) // Respect .gitignore files
		.git_global(true) // Respect global git ignore
		.git_exclude(true) // Respect .git/info/exclude
		.require_git(false) // Don't require git repository
		.follow_links(false) // Don't follow symlinks
		.max_depth(None); // No depth limit

	// Determine if we should apply dotfile filtering
	// Skip dotfile filtering if the search directory itself contains dot components
	// (e.g., when searching in temp directories like /var/folders/.../T/.tmpXXX/)
	let should_filter_dotfiles = !is_dotfile_or_in_dot_directory(&search_dir);

	// Collect all files first, then apply glob filtering
	let walker = builder.build();
	let mut all_files = Vec::new();

	for result in walker {
		match result {
			Ok(entry) => {
				let path = entry.path();

				// Skip directories
				if !path.is_file() {
					continue;
				}

				let path_str = path.to_string_lossy();

				// Skip dotfiles and files in dot directories only if we're not already in a dot directory
				if should_filter_dotfiles {
					// Get the relative path from search_dir to check for dot components
					let relative_path = if let Ok(rel) = path.strip_prefix(&search_dir) {
						rel.to_string_lossy().to_string()
					} else {
						path_str.to_string()
					};

					if is_dotfile_or_in_dot_directory(&relative_path) {
						continue;
					}
				}

				all_files.push(path_str.to_string());
			}
			Err(err) => {
				crate::log_debug!("Walker error: {}", err);
				// Continue walking even if some paths fail
			}
		}
	}

	crate::log_debug!(
		"Found {} files after gitignore and dotfile filtering",
		all_files.len()
	);

	// Now apply glob pattern matching
	for pattern in patterns {
		let mut pattern_matches = 0;

		// Check if this looks like a glob pattern
		if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
			// Compile glob pattern
			let glob_pattern = match glob::Pattern::new(pattern) {
				Ok(p) => p,
				Err(e) => return Err(anyhow!("Invalid glob pattern '{}': {}", pattern, e)),
			};

			// Apply pattern to all files
			for file_path in &all_files {
				// Normalize path for matching: strip leading "./" if present
				let normalized_path = file_path.strip_prefix("./").unwrap_or(file_path);

				if glob_pattern.matches(normalized_path) {
					expanded_paths.push(file_path.clone());
					pattern_matches += 1;
				}
			}
		} else {
			// Not a glob pattern, handle as direct path (file or directory)
			let path_obj = Path::new(pattern);
			if path_obj.exists() {
				if path_obj.is_file() {
					// It's a file, add it directly
					let path_str = pattern;
					if !is_dotfile_or_in_dot_directory(path_str) {
						expanded_paths.push(pattern.clone());
						pattern_matches += 1;
					}
				} else if path_obj.is_dir() {
					// It's a directory, add all files from it recursively
					// Normalize the directory path for matching
					let normalized_dir = pattern.trim_end_matches('/').trim_end_matches('\\');
					for file_path in &all_files {
						// Check if file is under this directory
						let file_path_normalized =
							file_path.strip_prefix("./").unwrap_or(file_path);
						if file_path_normalized.starts_with(&format!("{}/", normalized_dir))
							|| file_path_normalized.starts_with(&format!("{}\\", normalized_dir))
							|| file_path_normalized == normalized_dir
								&& !is_dotfile_or_in_dot_directory(file_path)
						{
							expanded_paths.push(file_path.clone());
							pattern_matches += 1;
						}
					}
				}
			}
		}

		crate::log_debug!(
			"Glob pattern '{}' matched {} files",
			pattern,
			pattern_matches
		);
	}

	// Deduplicate files in case multiple patterns match the same file
	expanded_paths.sort();
	expanded_paths.dedup();

	crate::log_debug!(
		"Total expanded files after deduplication: {}",
		expanded_paths.len()
	);

	// Check if we have too many files
	if expanded_paths.len() > MAX_EXPANDED_FILES {
		return Err(anyhow!(
            "Too many files expanded from glob patterns: {} files (max allowed: {}). Consider using more specific patterns to reduce the file count.",
            expanded_paths.len(),
            MAX_EXPANDED_FILES
        ));
	}

	Ok(expanded_paths)
}

/// Check if a file path is a dotfile or is inside a dot directory
///
/// This function identifies files that should be excluded:
/// - Files starting with '.' (e.g., .env, .gitignore)
/// - Files inside directories starting with '.' (e.g., .git/config, .vscode/settings.json)
///
/// # Arguments
/// * `path` - File path to check
///
/// # Returns
/// * `true` if the file should be excluded, `false` otherwise
fn is_dotfile_or_in_dot_directory(path: &str) -> bool {
	// Split path into components and check each one
	for component in Path::new(path).components() {
		if let Some(name) = component.as_os_str().to_str() {
			if name.starts_with('.') && name != "." && name != ".." {
				return true;
			}
		}
	}
	false
}

#[cfg(test)]
mod tests {
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
			.map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
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
		let result =
			expand_glob_patterns_filtered(&[pattern], None).expect("expansion must succeed");
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
		let result =
			expand_glob_patterns_filtered(&[pattern], None).expect("expansion must succeed");
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
}
