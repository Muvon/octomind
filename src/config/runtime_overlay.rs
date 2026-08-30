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

//! Runtime tool-allowlist overlay for dynamic capability activation.
//!
//! When a capability is activated at runtime via `capability enable <name>`,
//! its `[roles.mcp] allowed_tools` patterns must extend the role's effective
//! per-server tool filter for *that session*, without mutating the role's
//! authored config. This module owns that overlay.
//!
//! The overlay is consulted by [`crate::config::RoleMcpConfig::get_enabled_servers`]
//! during config merge: when the role's filter is restrictive (non-empty
//! `allowed_tools`), runtime extras for each affected server are unioned into
//! the per-server `tools` field. Capabilities declared in the role manifest
//! (`capabilities = [...]`) bypass this — they were already merged at boot
//! by `agent::registry::resolve_capabilities`.
//!
//! Lifecycle:
//! - `set_capability_extras(cap_name, per_server)` — install on activation.
//! - `clear_capability_extras(cap_name)` — remove on deactivation/eviction.
//! - `extras_for_server(server_name)` — union of bare tool names contributed
//!   by every currently-active capability for that server. Stable order, no
//!   duplicates.
//!
//! Storage is process-global to mirror `ACTIVE_CAPABILITIES` in
//! `src/mcp/core/capability.rs`. Both are runtime registries with the same
//! lifetime; tying them to per-session scope would only matter for multi-
//! session daemon mode, which is out of scope here.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// `cap_name -> server_name -> bare tool names contributed for that server`.
type Registry = HashMap<String, HashMap<String, Vec<String>>>;

static OVERLAY: OnceLock<RwLock<Registry>> = OnceLock::new();

fn registry() -> &'static RwLock<Registry> {
	OVERLAY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Install or replace this capability's per-server tool contributions.
/// Subsequent merges of any role that enables one of these servers will
/// expose the union of static + runtime extras for it.
pub fn set_capability_extras(cap_name: &str, per_server: HashMap<String, Vec<String>>) {
	let mut reg = match registry().write() {
		Ok(r) => r,
		Err(_) => return,
	};
	if per_server.is_empty() {
		reg.remove(cap_name);
	} else {
		reg.insert(cap_name.to_string(), per_server);
	}
}

/// Drop every server contribution for this capability. Called on
/// `capability disable` and on LRU eviction.
pub fn clear_capability_extras(cap_name: &str) {
	if let Ok(mut reg) = registry().write() {
		reg.remove(cap_name);
	}
}

/// Snapshot of the overlay: `cap_name -> server_name -> tool names`.
/// Used by the guardrail capability resolver to find which dynamically
/// activated capability owns a `(server, tool)` pair when the static tap
/// map doesn't already cover it.
pub fn snapshot() -> HashMap<String, HashMap<String, Vec<String>>> {
	let reg = match registry().read() {
		Ok(r) => r,
		Err(_) => return HashMap::new(),
	};
	reg.clone()
}

/// Union of bare tool names contributed by every active capability for
/// `server_name`. Order is insertion order across capabilities; duplicates
/// are deduplicated. Empty when no capability has registered an extra for
/// this server.
pub fn extras_for_server(server_name: &str) -> Vec<String> {
	let reg = match registry().read() {
		Ok(r) => r,
		Err(_) => return Vec::new(),
	};
	let mut out: Vec<String> = Vec::new();
	for per_server in reg.values() {
		if let Some(tools) = per_server.get(server_name) {
			for t in tools {
				if !out.iter().any(|x| x == t) {
					out.push(t.clone());
				}
			}
		}
	}
	out
}

#[cfg(test)]
#[path = "runtime_overlay_tests.rs"]
mod tests;
