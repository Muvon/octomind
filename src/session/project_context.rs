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

// Project context module for gathering and managing contextual information

use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use anyhow::Result;
use colored::*;

/// Represents the contextual information about the project
#[derive(Debug, Clone)]
pub struct ProjectContext {
	pub readme_content: Option<String>,
	pub changes_content: Option<String>,
	pub file_tree: Option<String>,
	pub git_status: Option<String>,
	pub git_branch: Option<String>,
}

impl Default for ProjectContext {
	fn default() -> Self {
		Self::new()
	}
}

impl ProjectContext {
	/// Create a new empty project context
	pub fn new() -> Self {
		Self {
			readme_content: None,
			changes_content: None,
			file_tree: None,
			git_status: None,
			git_branch: None,
		}
	}

	/// Collect all contextual information for the project
	pub fn collect(project_dir: &Path) -> Self {
		let mut context = Self::new();

		// Collect README.md content
		context.readme_content = Self::read_file_if_exists(project_dir.join("README.md"));

		// Collect CHANGES.md content
		context.changes_content = Self::read_file_if_exists(project_dir.join("CHANGES.md"));

		// Get file tree (excluding .gitignore patterns)
		context.file_tree = Self::get_file_tree(project_dir);

		// Get git status and branch if available
		context.git_status = Self::get_git_status(project_dir);
		context.git_branch = Self::get_git_branch(project_dir);

		context
	}

	/// Read file content if file exists
	fn read_file_if_exists(path: PathBuf) -> Option<String> {
		if path.exists() && path.is_file() {
			match fs::read_to_string(&path) {
				Ok(content) => {
					// Debug output
					// println!("{} {}", "Loaded context from:".green(), path.display());
					Some(content)
				},
				Err(e) => {
					println!("{} {}: {}", "Error reading".red(), path.display(), e);
					None
				}
			}
		} else {
			None
		}
	}

	/// Get file tree respecting .gitignore exclusions
	fn get_file_tree(project_dir: &Path) -> Option<String> {
		// Get list of files first
		let files_list = Self::get_files_list(project_dir)?;

		// Build tree structure from file list
		Some(Self::build_tree_structure(&files_list))
	}

	/// Get list of files using git, ripgrep, or manual fallback
	fn get_files_list(project_dir: &Path) -> Option<String> {
		// Try git ls-files first (respects .gitignore)
		let git_check = Command::new("git")
			.args(["rev-parse", "--is-inside-work-tree"])
			.current_dir(project_dir)
			.output();

		if let Ok(output) = git_check {
			if output.status.success() {
				if let Ok(output) = Command::new("git")
					.args(["ls-files"])
					.current_dir(project_dir)
					.output()
				{
					if output.status.success() {
						return Some(String::from_utf8_lossy(&output.stdout).to_string());
					}
				}
			}
		}

		// Fallback to ripgrep
		if let Ok(output) = Command::new("rg")
			.args(["--files"])
			.current_dir(project_dir)
			.output()
		{
			if output.status.success() {
				return Some(String::from_utf8_lossy(&output.stdout).to_string());
			}
		}

		// Last fallback: manual listing
		Self::list_files_manually(project_dir).ok()
	}

	/// Build a tree structure from a list of file paths
	fn build_tree_structure(files_list: &str) -> String {
		use std::collections::BTreeMap;

		#[derive(Debug)]
		enum TreeNode {
			File,
			Directory(BTreeMap<String, TreeNode>),
		}

		// Build the tree structure
		let mut root: BTreeMap<String, TreeNode> = BTreeMap::new();

		for line in files_list.lines() {
			let path = line.trim();
			if path.is_empty() {
				continue;
			}

			let parts: Vec<&str> = path.split('/').collect();
			if parts.is_empty() {
				continue;
			}

			// Build path step by step
			let mut current_map = &mut root;

			for (i, part) in parts.iter().enumerate() {
				let part_owned = part.to_string();
				let is_last = i == parts.len() - 1;

				if is_last {
					// This is the final file
					current_map.insert(part_owned, TreeNode::File);
					break; // Exit the loop after inserting the file
				} else {
					// This is an intermediate directory
					// Use entry API to ensure the directory exists
					current_map.entry(part_owned.clone()).or_insert_with(|| {
						TreeNode::Directory(BTreeMap::new())
					});

					// Now get the mutable reference to continue navigation
					if let Some(TreeNode::Directory(ref mut dir_map)) = current_map.get_mut(&part_owned) {
						current_map = dir_map;
					} else {
						break; // Should not happen, but break to be safe
					}
				}
			}
		}

		// Convert tree to string representation
		fn render_tree(
			node_map: &BTreeMap<String, TreeNode>,
			prefix: &str
		) -> String {
			let mut result = String::new();
			let entries: Vec<_> = node_map.iter().collect();

			for (i, (name, node)) in entries.iter().enumerate() {
				let is_last = i == entries.len() - 1;
				let current_prefix = if is_last { "└─ " } else { "├─ " };
				let next_prefix = if is_last { "   " } else { "│  " };

				match node {
					TreeNode::File => {
						result.push_str(&format!("{}{}{}\n", prefix, current_prefix, name));
					}
					TreeNode::Directory(children) => {
						result.push_str(&format!("{}{}{}/\n", prefix, current_prefix, name));
						if !children.is_empty() {
							result.push_str(&render_tree(
								children,
								&format!("{}{}", prefix, next_prefix)
							));
						}
					}
				}
			}

			result
		}

		render_tree(&root, "")
	}

	/// Manual file listing as a fallback
	fn list_files_manually(dir: &Path) -> Result<String> {
		let mut result = String::new();

		fn visit_dir(dir: &Path, base: &Path, result: &mut String) -> Result<()> {
			if dir.join(".git").exists() || dir.join("node_modules").exists() {
				return Ok(());
			}

			for entry in fs::read_dir(dir)? {
				let entry = entry?;
				let path = entry.path();
				let relative = path.strip_prefix(base)?
					.to_string_lossy()
					.to_string();

				if path.is_file() {
					result.push_str(&relative);
					result.push('\n');
				} else if path.is_dir() {
					visit_dir(&path, base, result)?;
				}
			}
			Ok(())
		}

		visit_dir(dir, dir, &mut result)?;
		Ok(result)
	}

	/// Get git status if available
	fn get_git_status(project_dir: &Path) -> Option<String> {
		let output = Command::new("git")
			.args(["status", "--short"])
			.current_dir(project_dir)
			.output();

		if let Ok(output) = output {
			if output.status.success() {
				let status = String::from_utf8_lossy(&output.stdout).to_string();
				if !status.trim().is_empty() {
					// Debug output
					// println!("{}", "Collected git status".green());
					return Some(status);
				}
			}
		}
		None
	}

	/// Get git branch if available
	fn get_git_branch(project_dir: &Path) -> Option<String> {
		let output = Command::new("git")
			.args(["rev-parse", "--abbrev-ref", "HEAD"])
			.current_dir(project_dir)
			.output();

		if let Ok(output) = output {
			if output.status.success() {
				let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
				if !branch.is_empty() {
					// Debug output
					// println!("{} {}", "Current git branch:".green(), branch);
					return Some(branch);
				}
			}
		}
		None
	}

	/// Format the project context as a string for inclusion in system prompts
	pub fn format_for_prompt(&self) -> String {
		let mut result = String::new();

		// Add README.md content if available
		if let Some(readme) = &self.readme_content {
			result.push_str("# Project README\n\n");
			result.push_str(readme);
			result.push_str("\n\n");
		}

		// Add CHANGES.md content if available
		if let Some(changes) = &self.changes_content {
			result.push_str("# Project CHANGES\n\n");
			result.push_str(changes);
			result.push_str("\n\n");
		}

		// Add git info if available
		if let Some(branch) = &self.git_branch {
			result.push_str(&format!("# Git Branch\n\n{}", branch));
			result.push_str("\n\n");
		}

		if let Some(status) = &self.git_status {
			result.push_str("# Git Status\n\n");
			result.push_str(status);
			result.push_str("\n\n");
		}

		// Add file tree if available
		if let Some(tree) = &self.file_tree {
			result.push_str("# Project File Structure\n\n");
			result.push_str(tree);
		}

		result
	}
}
