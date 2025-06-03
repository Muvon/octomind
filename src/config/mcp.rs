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

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerType {
	#[serde(rename = "external")]
	External, // External server (URL or command)
	#[serde(rename = "developer")]
	Developer, // Built-in developer tools
	#[serde(rename = "filesystem")]
	Filesystem, // Built-in filesystem tools
}

impl Default for McpServerType {
	fn default() -> Self {
		Self::External
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerMode {
	#[serde(rename = "http")]
	Http,
	#[serde(rename = "stdin")]
	Stdin,
}

impl Default for McpServerMode {
	fn default() -> Self {
		Self::Http
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
	// Name is auto-set from registry key (runtime field)
	#[serde(skip)]
	pub name: String,

	// Server type is auto-detected from name (runtime field)
	#[serde(skip)]
	pub server_type: McpServerType,

	// External server configuration
	pub url: Option<String>,
	pub auth_token: Option<String>,
	pub command: Option<String>,
	#[serde(default)]
	pub args: Vec<String>,

	// Communication mode - http or stdin (for external servers)
	#[serde(default)]
	pub mode: McpServerMode,

	// Timeout in seconds for tool execution
	#[serde(default = "default_timeout")]
	pub timeout_seconds: u64,

	// Tool filtering - empty means all tools are enabled
	#[serde(default)]
	pub tools: Vec<String>,
}

fn default_timeout() -> u64 {
	30 // Default timeout of 30 seconds
}

impl Default for McpServerConfig {
	fn default() -> Self {
		Self {
			name: "".to_string(),
			server_type: McpServerType::External, // Will be auto-detected
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: default_timeout(),
			tools: Vec::new(),
		}
	}
}

impl McpServerConfig {
	/// Create a server config from just the key name, auto-detecting type
	pub fn from_name(name: &str) -> Self {
		let server_type = match name {
			"developer" => McpServerType::Developer,
			"filesystem" => McpServerType::Filesystem,
			_ => McpServerType::External,
		};

		Self {
			name: name.to_string(),
			server_type,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools: Vec::new(),
		}
	}

	/// Create a developer server configuration
	pub fn developer(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Developer,
			tools,
			..Default::default()
		}
	}

	/// Create a filesystem server configuration
	pub fn filesystem(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Filesystem,
			tools,
			..Default::default()
		}
	}

	/// Create an external HTTP server configuration
	pub fn external_http(name: &str, url: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::External,
			url: Some(url.to_string()),
			mode: McpServerMode::Http,
			tools,
			..Default::default()
		}
	}

	/// Create an external command-based server configuration
	pub fn external_command(
		name: &str,
		command: &str,
		args: Vec<String>,
		tools: Vec<String>,
	) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::External,
			command: Some(command.to_string()),
			args,
			mode: McpServerMode::Stdin,
			tools,
			..Default::default()
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct McpConfig {
	// Server registry - server configurations
	#[serde(default)]
	pub servers: std::collections::HashMap<String, McpServerConfig>,

	// Tool filtering - allows limiting tools across all enabled servers
	#[serde(default)]
	pub allowed_tools: Vec<String>,
}

// Role-specific MCP configuration with server_refs
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct RoleMcpConfig {
	// Server references - list of server names from the global registry to use for this role
	// Empty list means MCP is disabled for this role
	#[serde(default)]
	pub server_refs: Vec<String>,

	// Tool filtering - allows limiting tools across all enabled servers for this role
	#[serde(default)]
	pub allowed_tools: Vec<String>,
}

impl RoleMcpConfig {
	/// Check if MCP is enabled for this role (has any server references)
	pub fn is_enabled(&self) -> bool {
		!self.server_refs.is_empty()
	}

	/// Get enabled servers from the global registry for this role
	/// UPDATED: Now uses runtime injection for core servers
	pub fn get_enabled_servers(
		&self,
		global_servers: &std::collections::HashMap<String, McpServerConfig>,
	) -> Vec<McpServerConfig> {
		if self.server_refs.is_empty() {
			return Vec::new();
		}

		let mut result = Vec::new();
		for server_name in &self.server_refs {
			// Try to get from loaded registry first, then fallback to core servers
			let server_config = global_servers
				.get(server_name)
				.cloned()
				.or_else(|| get_core_server_config(server_name));

			if let Some(mut server) = server_config {
				// Auto-set the name from the registry key
				server.name = server_name.clone();
				// Auto-detect server type from name
				server.server_type = match server_name.as_str() {
					"developer" => McpServerType::Developer,
					"filesystem" => McpServerType::Filesystem,
					_ => McpServerType::External,
				};
				// Apply role-specific tool filtering if specified
				if !self.allowed_tools.is_empty() {
					server.tools = self.allowed_tools.clone();
				}
				result.push(server);
			} else {
				// Note: Using println instead of log_debug since we're in a module
				// The log_debug macro would need to be imported
				println!("DEBUG: Server '{}' referenced by role but not found in global registry or core servers", server_name);
			}
		}

		result
	}
}

/// Get core server configuration - these are always available
/// This is separated from the config loading to ensure consistency
pub fn get_core_server_config(server_name: &str) -> Option<McpServerConfig> {
	match server_name {
		"developer" => Some(McpServerConfig {
			name: "developer".to_string(),
			server_type: McpServerType::Developer,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools: Vec::new(),
		}),
		"filesystem" => Some(McpServerConfig {
			name: "filesystem".to_string(),
			server_type: McpServerType::Filesystem,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools: Vec::new(),
		}),
		"octocode" => {
			let octocode_available = is_octocode_available();
			if octocode_available {
				Some(McpServerConfig {
					name: "octocode".to_string(),
					server_type: McpServerType::External,
					command: Some("octocode".to_string()),
					args: vec!["mcp".to_string(), "--path=.".to_string()],
					mode: McpServerMode::Stdin,
					timeout_seconds: 30,
					tools: vec![], // Empty means all tools are enabled
					url: None,
					auth_token: None,
				})
			} else {
				Some(McpServerConfig {
					name: "octocode".to_string(),
					server_type: McpServerType::External,
					command: Some("octocode".to_string()),
					args: vec!["mcp".to_string(), "--path=.".to_string()],
					mode: McpServerMode::Stdin,
					timeout_seconds: 30,
					tools: vec!["unavailable".to_string()], // Mark as unavailable if binary not found
					url: None,
					auth_token: None,
				})
			}
		}
		_ => None,
	}
}

/// Check if the octocode binary is available in PATH
fn is_octocode_available() -> bool {
	use std::process::Command;

	// Try to run `octocode --version` to check if it's available
	match Command::new("octocode").arg("--version").output() {
		Ok(output) => output.status.success(),
		Err(_) => false,
	}
}