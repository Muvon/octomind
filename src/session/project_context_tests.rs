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
fn tree_nests_directories_and_marks_them_with_a_slash() {
	let tree = ProjectContext::build_tree_structure("src/main.rs\nsrc/lib.rs\nREADME.md\n");
	assert_eq!(tree, "├─ README.md\n└─ src/\n   ├─ lib.rs\n   └─ main.rs\n");
}

#[test]
fn tree_is_sorted_and_deduplicates_shared_directories() {
	let tree = ProjectContext::build_tree_structure("b/2.rs\na/1.rs\nb/1.rs\n");
	let lines: Vec<&str> = tree.lines().collect();
	assert_eq!(
		lines,
		vec!["├─ a/", "│  └─ 1.rs", "└─ b/", "   ├─ 1.rs", "   └─ 2.rs",]
	);
}

#[test]
fn tree_skips_blank_lines_and_trims() {
	let tree = ProjectContext::build_tree_structure("\n  a.rs  \n\n   \n");
	assert_eq!(tree, "└─ a.rs\n");
}

#[test]
fn tree_handles_deep_nesting() {
	let tree = ProjectContext::build_tree_structure("a/b/c/d.rs\n");
	assert_eq!(tree, "└─ a/\n   └─ b/\n      └─ c/\n         └─ d.rs\n");
}

#[test]
fn tree_of_nothing_is_empty() {
	assert_eq!(ProjectContext::build_tree_structure(""), "");
}

#[test]
fn format_for_prompt_omits_absent_sections() {
	let mut ctx = ProjectContext::new();
	assert_eq!(ctx.format_for_prompt(), "");

	ctx.git_branch = Some("master".to_string());
	let out = ctx.format_for_prompt();
	assert!(out.contains("# Git Branch"));
	assert!(out.contains("master"));
	assert!(!out.contains("# Git Status"));
	assert!(!out.contains("# Project File Structure"));
}
