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

/// Registry configuration for fetching agent manifests.
/// Registry sources are managed via `octomind tap` / `octomind untap`.
/// Every field is required: `config-templates/default.toml` is the only place that holds a
/// default, and the config migration copies a missing field from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
	/// How long to cache fetched manifests before re-checking (hours)
	pub cache_ttl_hours: u64,
	/// The built-in tap in `user/repo` form, always loaded after the user's taps
	pub tap: String,
	/// Branch, tag or commit of the built-in tap to serve (`ref` is a Rust keyword)
	#[serde(rename = "ref")]
	pub git_ref: String,
}

/// The values the template ships, for tests that run without a config file. A test checks
/// they still equal the template.
#[cfg(test)]
impl Default for RegistryConfig {
	fn default() -> Self {
		Self {
			cache_ttl_hours: 24,
			tap: "muvon/tap".to_string(),
			git_ref: "feature/flat-agents".to_string(),
		}
	}
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
