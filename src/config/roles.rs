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

use serde::{Deserialize, Serialize};

use super::{mcp::RoleMcpConfig, ModelProfileOverride};

// Role configuration - contains all behavior settings but NOT API keys
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoleConfig {
	// Complete model-profile override. Every omitted field inherits main.
	#[serde(default)]
	pub model: ModelProfileOverride,
	// Versionless tap manifests may still use the historical flat sampling
	// fields. They are accepted as input and folded into `model`; new config is
	// always written with `[roles.model]`.
	#[serde(default, skip_serializing)]
	pub temperature: Option<f32>,
	#[serde(default, skip_serializing)]
	pub top_p: Option<f32>,
	#[serde(default, skip_serializing)]
	pub top_k: Option<u32>,
	// Custom system prompt (REQUIRED - defined in config template)
	pub system: String,
	// Custom welcome message with variable support
	pub welcome: String,
}

impl RoleConfig {
	pub fn model_override(&self) -> ModelProfileOverride {
		ModelProfileOverride {
			temperature: self.temperature,
			top_p: self.top_p,
			top_k: self.top_k,
			..Default::default()
		}
		.overlay(&self.model)
	}
}

// REMOVED: Default implementations - all config must be explicit

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

#[cfg(test)]
#[path = "roles_tests.rs"]
mod tests;
