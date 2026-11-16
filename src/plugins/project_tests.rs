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

fn assert_project_name(dir: &Path, expected: &str) {
	assert_eq!(project_name(dir), expected);
}

#[test]
fn a_plain_directory_names_itself() {
	let root = tempfile::tempdir().unwrap();
	let dir = root.path().join("notes");
	std::fs::create_dir(&dir).unwrap();
	assert_project_name(&dir, "notes");
}

#[test]
fn a_repository_root_names_itself() {
	let root = tempfile::tempdir().unwrap();
	let repo = root.path().join("shop");
	std::fs::create_dir_all(repo.join(".git")).unwrap();
	assert_project_name(&repo, "shop");
}

#[test]
fn a_subdirectory_names_its_repository() {
	let root = tempfile::tempdir().unwrap();
	let repo = root.path().join("shop");
	let sub = repo.join("src").join("cart");
	std::fs::create_dir_all(repo.join(".git")).unwrap();
	std::fs::create_dir_all(&sub).unwrap();
	assert_project_name(&sub, "shop");
}

#[test]
fn a_git_file_marks_a_worktree_root() {
	let root = tempfile::tempdir().unwrap();
	let worktree = root.path().join("shop-fix");
	std::fs::create_dir_all(&worktree).unwrap();
	std::fs::write(worktree.join(".git"), "gitdir: /elsewhere\n").unwrap();
	assert_project_name(&worktree, "shop-fix");
}

#[test]
fn a_path_without_a_base_name_is_unknown() {
	assert_eq!(project_name(Path::new("/")), UNKNOWN_PROJECT);
}
