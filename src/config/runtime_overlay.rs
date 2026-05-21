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
mod tests {
	use super::*;
	use std::sync::Mutex;

	// Serialize all tests in this module — they share the process-global OVERLAY static.
	static TEST_LOCK: Mutex<()> = Mutex::new(());

	fn fresh_registry() {
		if let Ok(mut r) = registry().write() {
			r.clear();
		}
	}

	#[test]
	fn extras_unions_across_capabilities() {
		let _guard = TEST_LOCK.lock().unwrap();
		fresh_registry();
		let mut shell_map = HashMap::new();
		shell_map.insert("octofs".to_string(), vec!["shell".to_string()]);
		set_capability_extras("shell", shell_map);

		let mut fs_map = HashMap::new();
		fs_map.insert(
			"octofs".to_string(),
			vec!["text_editor".to_string(), "view".to_string()],
		);
		set_capability_extras("filesystem-write", fs_map);

		let mut got = extras_for_server("octofs");
		got.sort();
		let mut expected = vec![
			"shell".to_string(),
			"text_editor".to_string(),
			"view".to_string(),
		];
		expected.sort();
		assert_eq!(got, expected);
	}

	#[test]
	fn clear_removes_only_named_capability() {
		let _guard = TEST_LOCK.lock().unwrap();
		fresh_registry();
		let mut a = HashMap::new();
		a.insert("svr".to_string(), vec!["one".to_string()]);
		set_capability_extras("a", a);

		let mut b = HashMap::new();
		b.insert("svr".to_string(), vec!["two".to_string()]);
		set_capability_extras("b", b);

		clear_capability_extras("a");

		let got = extras_for_server("svr");
		assert_eq!(got, vec!["two".to_string()]);
	}

	#[test]
	fn empty_per_server_is_treated_as_clear() {
		let _guard = TEST_LOCK.lock().unwrap();
		fresh_registry();
		let mut a = HashMap::new();
		a.insert("svr".to_string(), vec!["one".to_string()]);
		set_capability_extras("a", a);

		// Calling with empty map removes the capability rather than
		// inserting an empty entry — keeps the registry tight.
		set_capability_extras("a", HashMap::new());
		assert!(extras_for_server("svr").is_empty());
	}

	#[test]
	fn unknown_server_returns_empty() {
		let _guard = TEST_LOCK.lock().unwrap();
		fresh_registry();
		assert!(extras_for_server("never-seen").is_empty());
	}

	#[test]
	fn duplicates_within_one_server_are_deduped() {
		let _guard = TEST_LOCK.lock().unwrap();
		fresh_registry();
		let mut a = HashMap::new();
		a.insert(
			"svr".to_string(),
			vec!["x".to_string(), "y".to_string(), "x".to_string()],
		);
		set_capability_extras("a", a);

		let mut b = HashMap::new();
		b.insert("svr".to_string(), vec!["y".to_string(), "z".to_string()]);
		set_capability_extras("b", b);

		let got = extras_for_server("svr");
		assert_eq!(got.len(), 3);
		assert!(got.contains(&"x".to_string()));
		assert!(got.contains(&"y".to_string()));
		assert!(got.contains(&"z".to_string()));
	}
}
