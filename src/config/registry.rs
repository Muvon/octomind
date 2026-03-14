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

fn default_sources() -> Vec<String> {
	vec!["https://raw.githubusercontent.com/muvon/octomind-agents/main".to_string()]
}

fn default_cache_ttl_hours() -> u64 {
	24
}

/// Registry configuration for fetching agent manifests.
/// Sources are checked in order — first hit wins.
/// Supports https:// URLs and file:// local paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
	/// Ordered list of registry sources (https:// or file://)
	#[serde(default = "default_sources")]
	pub sources: Vec<String>,

	/// How long to cache fetched manifests before re-checking (hours)
	#[serde(default = "default_cache_ttl_hours")]
	pub cache_ttl_hours: u64,
}

impl Default for RegistryConfig {
	fn default() -> Self {
		Self {
			sources: default_sources(),
			cache_ttl_hours: default_cache_ttl_hours(),
		}
	}
}
