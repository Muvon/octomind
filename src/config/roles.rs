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

// Mode configuration - contains all behavior settings but NOT API keys or model (uses system-wide model)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModeConfig {
	// Layer configurations
	pub enable_layers: bool,
	// Custom system prompt
	pub system: Option<String>,
}

// REMOVED: Default implementations - all config must be explicit
// REMOVED: Model-related methods - roles now use system-wide model only

// Updated role configurations using the new ModeConfig structure
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeveloperRoleConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	pub mcp: RoleMcpConfig,
	// Layer references - list of layer names to use for this role
	pub layer_refs: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssistantRoleConfig {
	#[serde(flatten)]
	pub config: ModeConfig,
	pub mcp: RoleMcpConfig,
	// Layer references - list of layer names to use for this role
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
