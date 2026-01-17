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

use super::oauth_config::OAuthConfig;
use serde::{Deserialize, Serialize};

// Type-specific MCP server configuration using tagged enums
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum McpServerConfig {
	#[serde(rename = "builtin")]
	Builtin {
		name: String,
		timeout_seconds: u64,
		tools: Vec<String>,
	},
	#[serde(rename = "http")]
	Http {
		name: String,
		url: String,
		#[serde(skip_serializing_if = "Option::is_none")]
		auth_token: Option<String>,
		/// OAuth 2.1 + PKCE configuration for this server.
		/// When set, authentication will use OAuth instead of static auth_token.
		#[serde(skip_serializing_if = "Option::is_none")]
		oauth: Option<OAuthConfig>,
		timeout_seconds: u64,
		tools: Vec<String>,
	},
	#[serde(rename = "stdin")]
	Stdin {
		name: String,
		command: String,
		args: Vec<String>,
		timeout_seconds: u64,
		tools: Vec<String>,
	},
}

// Legacy connection type enum for backward compatibility in some functions
#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub enum McpConnectionType {
	Builtin,
	Stdin,
	Http,
}

impl McpServerConfig {
	/// Get the server name regardless of variant
	pub fn name(&self) -> &str {
		match self {
			McpServerConfig::Builtin { name, .. } => name,
			McpServerConfig::Http { name, .. } => name,
			McpServerConfig::Stdin { name, .. } => name,
		}
	}

	/// Get the connection type for compatibility
	pub fn connection_type(&self) -> McpConnectionType {
		match self {
			McpServerConfig::Builtin { .. } => McpConnectionType::Builtin,
			McpServerConfig::Http { .. } => McpConnectionType::Http,
			McpServerConfig::Stdin { .. } => McpConnectionType::Stdin,
		}
	}

	/// Get timeout seconds regardless of variant
	pub fn timeout_seconds(&self) -> u64 {
		match self {
			McpServerConfig::Builtin {
				timeout_seconds, ..
			} => *timeout_seconds,
			McpServerConfig::Http {
				timeout_seconds, ..
			} => *timeout_seconds,
			McpServerConfig::Stdin {
				timeout_seconds, ..
			} => *timeout_seconds,
		}
	}

	/// Get tools list regardless of variant
	pub fn tools(&self) -> &[String] {
		match self {
			McpServerConfig::Builtin { tools, .. } => tools,
			McpServerConfig::Http { tools, .. } => tools,
			McpServerConfig::Stdin { tools, .. } => tools,
		}
	}

	/// Get URL for HTTP servers (if available)
	pub fn url(&self) -> Option<&str> {
		match self {
			McpServerConfig::Http { url, .. } => Some(url),
			_ => None,
		}
	}

	/// Get auth token for HTTP servers (if available)
	///
	/// Returns the static auth_token if set, regardless of OAuth configuration.
	/// For OAuth-based authentication, use `get_oauth_token()` instead.
	pub fn auth_token(&self) -> Option<&str> {
		match self {
			McpServerConfig::Http { auth_token, .. } => auth_token.as_deref(),
			_ => None,
		}
	}

	/// Get OAuth configuration for HTTP servers (if available)
	///
	/// Returns `Some(OAuthConfig)` if OAuth is configured, `None` otherwise.
	pub fn oauth_config(&self) -> Option<&OAuthConfig> {
		match self {
			McpServerConfig::Http { oauth, .. } => oauth.as_ref(),
			_ => None,
		}
	}

	/// Check if OAuth is configured for this server
	///
	/// Returns `true` if OAuth configuration exists (regardless of enabled status).
	pub fn has_oauth_config(&self) -> bool {
		self.oauth_config().is_some()
	}

	/// Check if OAuth is enabled for this server
	///
	/// Returns `true` if OAuth configuration exists.
	/// The presence of an oauth section in config means OAuth is enabled.
	pub fn is_oauth_enabled(&self) -> bool {
		self.oauth_config().is_some()
	}

	/// Check if this server requires authentication
	///
	/// Returns `true` if either static auth_token or OAuth is configured.
	pub fn requires_auth(&self) -> bool {
		self.auth_token().is_some() || self.is_oauth_enabled()
	}

	/// Get command for command-based servers (if available)
	pub fn command(&self) -> Option<&str> {
		match self {
			McpServerConfig::Stdin { command, .. } => Some(command),
			_ => None,
		}
	}

	/// Get args for command-based servers (if available)
	pub fn args(&self) -> &[String] {
		match self {
			McpServerConfig::Stdin { args, .. } => args,
			_ => &[],
		}
	}

	/// Create a builtin server configuration
	pub fn builtin(name: &str, timeout_seconds: u64, tools: Vec<String>) -> Self {
		Self::Builtin {
			name: name.to_string(),
			timeout_seconds,
			tools,
		}
	}

	/// Create an HTTP server configuration
	///
	/// # Arguments
	///
	/// * `name` - Unique name for this server
	/// * `url` - The MCP server URL (can be localhost or remote)
	/// * `timeout_seconds` - Request timeout in seconds
	/// * `tools` - List of allowed tools (empty = all tools)
	/// * `auth_token` - Static Bearer token (optional, used if OAuth not configured)
	/// * `oauth` - OAuth 2.1 + PKCE configuration (optional)
	pub fn http(
		name: &str,
		url: &str,
		timeout_seconds: u64,
		tools: Vec<String>,
		auth_token: Option<String>,
		oauth: Option<OAuthConfig>,
	) -> Self {
		Self::Http {
			name: name.to_string(),
			url: url.to_string(),
			auth_token,
			oauth,
			timeout_seconds,
			tools,
		}
	}

	/// Create a stdin server configuration
	pub fn stdin(
		name: &str,
		command: &str,
		args: Vec<String>,
		timeout_seconds: u64,
		tools: Vec<String>,
	) -> Self {
		Self::Stdin {
			name: name.to_string(),
			command: command.to_string(),
			args,
			timeout_seconds,
			tools,
		}
	}

	/// Validate the server configuration
	///
	/// Returns `Ok(())` if valid, or `Err(String)` with error message.
	///
	/// For HTTP servers with OAuth configuration:
	/// - Validates OAuth config if present
	pub fn validate(&self) -> Result<(), String> {
		match self {
			McpServerConfig::Builtin { name, .. } => {
				if name.is_empty() {
					return Err("Builtin server name cannot be empty".to_string());
				}
			}
			McpServerConfig::Http {
				name, url, oauth, ..
			} => {
				if name.is_empty() {
					return Err("HTTP server name cannot be empty".to_string());
				}
				if url.is_empty() {
					return Err("HTTP server URL cannot be empty".to_string());
				}
				// Validate OAuth config if present
				if let Some(oauth_config) = oauth {
					oauth_config
						.validate()
						.map_err(|e| format!("OAuth configuration validation failed: {}", e))?;
				}
			}
			McpServerConfig::Stdin { name, command, .. } => {
				if name.is_empty() {
					return Err("Stdin server name cannot be empty".to_string());
				}
				if command.is_empty() {
					return Err("Stdin server command cannot be empty".to_string());
				}
			}
		}
		Ok(())
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
			if let Some(server_config) = global_servers.iter().find(|s| s.name() == *server_name) {
				let mut server = server_config.clone();
				// Apply role-specific tool filtering if specified
				if !self.allowed_tools.is_empty() {
					// Convert patterns to actual tool names for this server
					let filtered_tools = self.expand_patterns_for_server(server_name);
					// Update tools based on server type
					server = match server {
						McpServerConfig::Builtin {
							name,
							timeout_seconds,
							..
						} => McpServerConfig::Builtin {
							name,
							timeout_seconds,
							tools: filtered_tools,
						},
						McpServerConfig::Http {
							name,
							url,
							auth_token,
							oauth,
							timeout_seconds,
							tools: _,
						} => McpServerConfig::Http {
							name,
							url,
							auth_token,
							oauth,
							timeout_seconds,
							tools: filtered_tools,
						},
						McpServerConfig::Stdin {
							name,
							command,
							args,
							timeout_seconds,
							..
						} => McpServerConfig::Stdin {
							name,
							command,
							args,
							timeout_seconds,
							tools: filtered_tools,
						},
					};
				}
				result.push(server);
			} else {
				// Note: Using println instead of log_debug since we're in a module
				// The log_debug macro would need to be imported
				println!(
					"DEBUG: Server '{server_name}' referenced by role but not found in global registry"
				);
			}
		}

		result
	}

	/// Expand allowed_tools patterns into actual tool names for a specific server
	/// This converts patterns like "filesystem:*" or "filesystem:text_*" into concrete tool lists
	fn expand_patterns_for_server(&self, server_name: &str) -> Vec<String> {
		let mut expanded_tools = Vec::new();

		for pattern in &self.allowed_tools {
			// Check for server group pattern (e.g., "filesystem:*" or "filesystem:text_*")
			if let Some((server_prefix, tool_pattern)) = pattern.split_once(':') {
				// Check if server matches
				if server_prefix == server_name {
					if tool_pattern == "*" {
						// All tools from this server - return empty to indicate "all tools"
						return Vec::new();
					} else if tool_pattern.ends_with('*') {
						// Prefix matching (e.g., "text_*") - we'll need to get actual tools and filter
						// For now, store the pattern and let the existing filtering handle it
						expanded_tools.push(tool_pattern.to_string());
					} else {
						// Exact tool name within server namespace
						expanded_tools.push(tool_pattern.to_string());
					}
				}
			} else {
				// Exact tool name match (backward compatibility) - include for all servers
				expanded_tools.push(pattern.clone());
			}
		}

		expanded_tools
	}
}

// Note: Core server configurations are now defined in the config file
// The get_core_server_config function is removed as we rely entirely on config
