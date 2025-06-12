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
	#[serde(rename = "agent")]
	Agent, // Built-in agent tool
}

// Keep Default for runtime usage only (not config defaults)
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

// Keep Default for runtime usage only (not config defaults)
impl Default for McpServerMode {
	fn default() -> Self {
		Self::Http
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
	// Name field is now explicit in config (like layers)
	pub name: String,

	// Server type - now part of config to distinguish builtin vs external
	pub server_type: McpServerType,

	// External server configuration
	pub url: Option<String>,
	pub auth_token: Option<String>,
	pub command: Option<String>,
	pub args: Vec<String>,

	// Communication mode - http or stdin (for external servers)
	pub mode: McpServerMode,

	// Timeout in seconds for tool execution
	pub timeout_seconds: u64,

	// Tool filtering - empty means all tools are enabled
	pub tools: Vec<String>,

	// Mark if this is a builtin server (affects maintenance and availability)
	#[serde(default)]
	pub builtin: bool,
}

// REMOVED: Default implementations - all config must be explicit

impl McpServerConfig {
	/// Create a server config from just the key name, auto-detecting type
	pub fn from_name(name: &str) -> Self {
		let server_type = match name {
			"developer" => McpServerType::Developer,
			"filesystem" => McpServerType::Filesystem,
			"agent" => McpServerType::Agent,
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
			builtin: false, // Default to false, set explicitly when needed
		}
	}

	/// Create a developer server configuration
	pub fn developer(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Developer,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools,
			builtin: true, // Developer servers are builtin
		}
	}

	/// Create a filesystem server configuration
	pub fn filesystem(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Filesystem,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools,
			builtin: true, // Filesystem servers are builtin
		}
	}

	/// Create an agent server configuration
	pub fn agent(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::Agent,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools,
			builtin: true, // Agent servers are builtin
		}
	}

	/// Create an external HTTP server configuration
	pub fn external_http(name: &str, url: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			server_type: McpServerType::External,
			url: Some(url.to_string()),
			auth_token: None,
			command: None,
			args: Vec::new(),
			mode: McpServerMode::Http,
			timeout_seconds: 30,
			tools,
			builtin: false, // External servers are not builtin
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
			url: None,
			auth_token: None,
			command: Some(command.to_string()),
			args,
			mode: McpServerMode::Stdin,
			timeout_seconds: 30,
			tools,
			builtin: false, // External servers are not builtin
		}
	}

	/// Create an octocode server configuration (builtin but external command)
	pub fn octocode(available: bool) -> Self {
		Self {
			name: "octocode".to_string(),
			server_type: McpServerType::External,
			command: Some("octocode".to_string()),
			args: vec!["mcp".to_string(), "--path=.".to_string()],
			mode: McpServerMode::Stdin,
			timeout_seconds: 30,
			tools: if available {
				vec![]
			} else {
				vec!["unavailable".to_string()]
			},
			url: None,
			auth_token: None,
			builtin: true, // Octocode is builtin even though it's external command
		}
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpConfig {
	// Server registry - array of server configurations (consistent with layers)
	pub servers: Vec<McpServerConfig>,

	// Tool filtering - allows limiting tools across all enabled servers
	pub allowed_tools: Vec<String>,
}

// Role-specific MCP configuration with server_refs
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct RoleMcpConfig {
	// Server references - list of server names from the global registry to use for this role
	// Empty list means MCP is disabled for this role
	pub server_refs: Vec<String>,

	// Tool filtering - allows limiting tools across all enabled servers for this role
	pub allowed_tools: Vec<String>,
}

// REMOVED: Default implementations - all config must be explicit

impl RoleMcpConfig {
	/// Check if MCP is enabled for this role (has any server references)
	pub fn is_enabled(&self) -> bool {
		!self.server_refs.is_empty()
	}

	/// Get enabled servers from the global registry for this role
	/// Now works with array format (consistent with layers)
	pub fn get_enabled_servers(&self, global_servers: &[McpServerConfig]) -> Vec<McpServerConfig> {
		if self.server_refs.is_empty() {
			return Vec::new();
		}

		let mut result = Vec::new();
		for server_name in &self.server_refs {
			// Find server by name in the array
			if let Some(server_config) = global_servers.iter().find(|s| s.name == *server_name) {
				let mut server = server_config.clone();
				// Apply role-specific tool filtering if specified
				if !self.allowed_tools.is_empty() {
					server.tools = self.allowed_tools.clone();
				}
				result.push(server);
			} else {
				// Note: Using println instead of log_debug since we're in a module
				// The log_debug macro would need to be imported
				println!(
					"DEBUG: Server '{}' referenced by role but not found in global registry",
					server_name
				);
			}
		}

		result
	}
}

// Note: Core server configurations are now defined in the config file
// The get_core_server_config function is removed as we rely entirely on config

// is_octocode_available function moved to config/loading.rs
