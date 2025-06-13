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

use super::mcp::RoleMcpConfig;

// Role configuration - contains all behavior settings but NOT API keys or model (uses system-wide model)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoleConfig {
	// Layer configurations
	#[serde(default)]
	pub enable_layers: bool,
	// Custom system prompt
	pub system: Option<String>,
	// Temperature for AI responses (0.0 to 1.0) - STRICT: must be in config
	pub temperature: f32,
}

// REMOVED: Default implementations - all config must be explicit
// REMOVED: Model-related methods - roles now use system-wide model only

// Unified role configuration for all roles (developer, assistant, custom)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Role {
	// Role name (e.g., "developer", "assistant", "tester")
	pub name: String,

	// Flattened role configuration
	#[serde(flatten)]
	pub config: RoleConfig,

	// MCP configuration for this role
	#[serde(default)]
	pub mcp: RoleMcpConfig,

	// Layer references - list of layer names to use for this role
	#[serde(default)]
	pub layer_refs: Vec<String>,
}

// REMOVED: Default implementations - all config must be explicit

impl RoleMcpConfig {
	/// Create a new RoleMcpConfig with server references
	pub fn with_server_refs(server_refs: Vec<String>) -> Self {
		Self {
			server_refs,
			allowed_tools: Vec::new(),
		}
	}

	/// Create a new RoleMcpConfig with server references and allowed tools
	pub fn with_server_refs_and_tools(
		server_refs: Vec<String>,
		allowed_tools: Vec<String>,
	) -> Self {
		Self {
			server_refs,
			allowed_tools,
		}
	}
}
