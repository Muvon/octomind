// Copyright 2025 Muvon Un Limited
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
			// Not a glob pattern, add as-is if it exists and passes filters
			let path_obj = Path::new(pattern);
			if path_obj.exists() && path_obj.is_file() {
				let path_str = pattern;
				if !is_dotfile_or_in_dot_directory(path_str) {
					expanded_paths.push(pattern.clone());
					pattern_matches += 1;
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
}
