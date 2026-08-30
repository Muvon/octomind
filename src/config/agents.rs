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

//! Agent configuration - ACP-based agent definitions.
//!
//! Each agent is defined by:
//! - name: Unique identifier (exposed as MCP tool `agent_<name>`)
//! - description: MCP tool description shown to the AI
//! - command: Shell command that runs an ACP server (e.g. "octomind acp --role context_gatherer")
//! - workdir: Working directory for agent execution (optional, default ".")

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Agent configuration - runs any ACP-compatible command as a sub-agent.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AgentConfig {
	/// Unique agent name (used in tool name: agent_<name>)
	pub name: String,

	/// Description shown in MCP tool definition
	pub description: String,

	/// Shell command that starts an ACP server over stdio.
	/// Example: "octomind acp --role context_gatherer"
	pub command: String,

	/// Working directory for agent execution (optional, default: ".")
	/// Relative paths are resolved from the session's working directory.
	#[serde(default = "default_workdir")]
	pub workdir: String,
}

fn default_workdir() -> String {
	".".to_string()
}

impl AgentConfig {
	/// Get the resolved working directory as an absolute path.
	///
	/// If workdir is relative, it's resolved relative to the session's working directory.
	pub fn get_resolved_workdir(&self, session_workdir: &Path) -> PathBuf {
		let workdir_path = PathBuf::from(&self.workdir);
		if workdir_path.is_absolute() {
			workdir_path
		} else {
			session_workdir.join(&self.workdir)
		}
	}
}

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
