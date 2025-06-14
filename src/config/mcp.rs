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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum McpConnectionType {
	#[serde(rename = "builtin")]
	Builtin, // Built-in server (developer, filesystem, agent)
	#[serde(rename = "stdin")]
	Stdin, // External server via stdin/command
	#[serde(rename = "http")]
	Http, // External server via HTTP
}

// Keep Default for runtime usage only (not config defaults)
impl Default for McpConnectionType {
	fn default() -> Self {
		Self::Http
	}
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
	// Name field is now explicit in config (like layers)
	pub name: String,

	// Connection type - determines how to connect to the server
	#[serde(rename = "type")]
	pub connection_type: McpConnectionType,

	// External server configuration
	pub url: Option<String>,
	pub auth_token: Option<String>,
	pub command: Option<String>,
	pub args: Vec<String>,

	// Timeout in seconds for tool execution
	pub timeout_seconds: u64,

	// Tool filtering - empty means all tools are enabled
	pub tools: Vec<String>,
}

// REMOVED: Default implementations - all config must be explicit

impl McpServerConfig {
	/// Create a server config from just the key name, auto-detecting type
	pub fn from_name(name: &str) -> Self {
		let connection_type = match name {
			"developer" | "filesystem" | "agent" => McpConnectionType::Builtin,
			_ => McpConnectionType::Http,
		};

		Self {
			name: name.to_string(),
			connection_type,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			timeout_seconds: 30,
			tools: Vec::new(),
		}
	}

	/// Create a developer server configuration
	pub fn developer(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			connection_type: McpConnectionType::Builtin,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			timeout_seconds: 30,
			tools,
		}
	}

	/// Create a filesystem server configuration
	pub fn filesystem(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			connection_type: McpConnectionType::Builtin,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			timeout_seconds: 30,
			tools,
		}
	}

	/// Create an agent server configuration
	pub fn agent(name: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			connection_type: McpConnectionType::Builtin,
			url: None,
			auth_token: None,
			command: None,
			args: Vec::new(),
			timeout_seconds: 30,
			tools,
		}
	}

	/// Create an external HTTP server configuration
	pub fn external_http(name: &str, url: &str, tools: Vec<String>) -> Self {
		Self {
			name: name.to_string(),
			connection_type: McpConnectionType::Http,
			url: Some(url.to_string()),
			auth_token: None,
			command: None,
			args: Vec::new(),
			timeout_seconds: 30,
			tools,
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
			connection_type: McpConnectionType::Stdin,
			url: None,
			auth_token: None,
			command: Some(command.to_string()),
			args,
			timeout_seconds: 30,
			tools,
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
