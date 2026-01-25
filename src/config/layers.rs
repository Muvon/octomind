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

use crate::session::layers::LayerConfig;
use serde::{Deserialize, Serialize};

/// Global layers configuration - registry of available layers
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LayersConfig {
	/// Layer registry - layer configurations (only custom/external layers need to be defined here)
	/// Core layers are automatically available and don't need configuration
	pub layers: std::collections::HashMap<String, LayerConfig>,
}

impl LayersConfig {
	/// Check if this config should be skipped during serialization
	/// This helps avoid writing empty [layers] sections when only core layers exist
	pub fn is_default_for_serialization(&self) -> bool {
		self.layers.is_empty()
	}

	/// Get all layers from the registry (for populating role configs)
	/// Now relies entirely on config - no more runtime injection
	pub fn get_all_layers(&self) -> Vec<LayerConfig> {
		let mut result = Vec::new();

		// Add layers from loaded registry
		for (layer_name, layer_config) in &self.layers {
			let mut layer = layer_config.clone();
			// Auto-set the name from the registry key
			layer.name = layer_name.clone();
			result.push(layer);
		}

		result
	}
}

// Note: Core layer configurations are now defined in the config file
// The get_core_layer_config function is removed as we rely entirely on config
