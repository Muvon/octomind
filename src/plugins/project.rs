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

//! The name of the project a session works in.

use std::path::Path;

/// Name used when the repository root or workdir has no readable base name.
const UNKNOWN_PROJECT: &str = "unknown";

/// Base name of the repository root above `workdir`, or of `workdir` outside
/// a repository. The walk stops below the home directory: a dotfiles
/// repository at `~` must not name every unversioned project after itself.
pub fn project_name(workdir: &Path) -> String {
	project_root(workdir)
		.file_name()
		.and_then(|name| name.to_str())
		.map(String::from)
		.unwrap_or_else(|| UNKNOWN_PROJECT.to_string())
}

/// Nearest ancestor holding `.git`, a directory or the file a worktree or
/// submodule uses; `dir` itself when there is none.
fn project_root(dir: &Path) -> &Path {
	let home = dirs::home_dir();
	dir.ancestors()
		.take_while(|ancestor| home.as_deref() != Some(*ancestor))
		.find(|ancestor| ancestor.join(".git").exists())
		.unwrap_or(dir)
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
