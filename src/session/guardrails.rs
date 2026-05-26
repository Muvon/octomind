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

//! Session-scoped guardrail state.
//!
//! Two registries, both keyed by `SessionId`:
//!
//!   * `RULES`    — compiled `Guardrails` loaded from
//!     `<workdir>/.agents/guardrails.toml` at session start.
//!   * `CALL_LOG` — list of `(capability, params)` for every successful tool
//!     call this session, used to evaluate `when = ["+/-..."]`.

use crate::config::guardrails::{CallRecord, Guardrails};
use crate::config::Config;
use crate::session::context::SessionId;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

static RULES: RwLock<Option<HashMap<SessionId, Arc<Guardrails>>>> = RwLock::new(None);

type CallLog = Vec<CallRecord>;

static CALL_LOG: RwLock<Option<HashMap<SessionId, CallLog>>> = RwLock::new(None);

/// `(server_name, tool_name) -> capability_name`. Built once per session
/// from installed tap manifests (see `agent::registry::list_all_capabilities`)
/// and consulted on every tool call to resolve the call's owning capability.
/// `*` patterns in `allowed_tools` (e.g. `octofs:*`) map server-wide.
type CapMap = HashMap<(String, String), String>;

/// Map of server-name -> capability-name for `<server>:*` wildcard entries.
type WildcardMap = HashMap<String, String>;

#[derive(Default)]
struct CapLookup {
	exact: CapMap,
	wildcard: WildcardMap,
}

static CAP_LOOKUP: RwLock<Option<HashMap<SessionId, Arc<CapLookup>>>> = RwLock::new(None);

/// Per-session, per-validator cursor into the call log. Validators evaluate
/// their `when` conditions against `call_log[cursor..]` — i.e. calls made
/// since this validator last ran. When the validator runs, the cursor is
/// advanced to `call_log.len()` so subsequent runs see a fresh slice.
static VALIDATOR_CURSORS: RwLock<Option<HashMap<SessionId, HashMap<String, usize>>>> =
	RwLock::new(None);

/// Per-session, per-pipe run counters. Keyed by (SessionId, pipe_name).
static PIPE_RUN_COUNTS: RwLock<Option<HashMap<SessionId, HashMap<String, u64>>>> =
	RwLock::new(None);

/// Per-session message counter (total user messages including current).
static MESSAGE_COUNTS: RwLock<Option<HashMap<SessionId, u64>>> = RwLock::new(None);

/// Load `.agents/guardrails.toml` from the session's working directory and
/// install it for the current session. No-op when not in a session context.
pub fn init_for_session() {
	let Some(sid) = crate::session::context::current_session_id() else {
		return;
	};
	let workdir = crate::session::context::get_current_workdir(&sid)
		.or_else(|| std::env::current_dir().ok())
		.unwrap_or_default();
	let rules = Guardrails::load_from_workdir(&workdir);
	let mut guard = RULES.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(sid, Arc::new(rules));
}

pub fn get_rules(session_id: &SessionId) -> Option<Arc<Guardrails>> {
	let guard = RULES.read().ok()?;
	guard.as_ref()?.get(session_id).cloned()
}

pub fn clear_for_session(session_id: &SessionId) {
	if let Ok(mut guard) = RULES.write() {
		if let Some(r) = guard.as_mut() {
			r.remove(session_id);
		}
	}
	if let Ok(mut guard) = CALL_LOG.write() {
		if let Some(r) = guard.as_mut() {
			r.remove(session_id);
		}
	}
	if let Ok(mut guard) = CAP_LOOKUP.write() {
		if let Some(r) = guard.as_mut() {
			r.remove(session_id);
		}
	}
	if let Ok(mut guard) = VALIDATOR_CURSORS.write() {
		if let Some(r) = guard.as_mut() {
			r.remove(session_id);
		}
	}
	if let Ok(mut guard) = PIPE_RUN_COUNTS.write() {
		if let Some(r) = guard.as_mut() {
			r.remove(session_id);
		}
	}
	if let Ok(mut guard) = MESSAGE_COUNTS.write() {
		if let Some(r) = guard.as_mut() {
			r.remove(session_id);
		}
	}
}

/// Read the cursor for a validator (default 0 = "since session start").
pub fn validator_cursor(session_id: &SessionId, name: &str) -> usize {
	let guard = match VALIDATOR_CURSORS.read() {
		Ok(g) => g,
		Err(_) => return 0,
	};
	guard
		.as_ref()
		.and_then(|r| r.get(session_id))
		.and_then(|m| m.get(name))
		.copied()
		.unwrap_or(0)
}

/// Advance the cursor for a validator to the given position.
pub fn set_validator_cursor(session_id: &SessionId, name: &str, pos: usize) {
	let mut guard = VALIDATOR_CURSORS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let per_session = registry.entry(session_id.clone()).or_default();
	per_session.insert(name.to_string(), pos);
}

/// Build the (server, tool) -> capability map from installed tap manifests.
/// Called lazily on first guardrail check per session.
fn build_cap_lookup(config: &Config) -> CapLookup {
	let mut out = CapLookup::default();
	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(v) => v,
		Err(_) => return out,
	};
	for cap in caps {
		for entry in &cap.allowed_tools {
			// Entries are "server:tool" or "server:*".
			let Some((server, tool)) = entry.split_once(':') else {
				continue;
			};
			if tool == "*" {
				out.wildcard
					.entry(server.to_string())
					.or_insert_with(|| cap.name.clone());
			} else {
				out.exact
					.entry((server.to_string(), tool.to_string()))
					.or_insert_with(|| cap.name.clone());
			}
		}
	}
	out
}

fn get_or_build_lookup(session_id: &SessionId, config: &Config) -> Arc<CapLookup> {
	{
		let guard = CAP_LOOKUP.read().unwrap();
		if let Some(registry) = guard.as_ref() {
			if let Some(l) = registry.get(session_id) {
				return l.clone();
			}
		}
	}
	let lookup = Arc::new(build_cap_lookup(config));
	let mut guard = CAP_LOOKUP.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry.insert(session_id.clone(), lookup.clone());
	lookup
}

/// Resolve a tool call to its capability name. Tries static taps first
/// (exact `server:tool`, then `server:*`); falls back to the dynamic
/// `runtime_overlay` for capabilities activated mid-session.
pub fn resolve_capability(
	session_id: &SessionId,
	config: &Config,
	server: Option<&str>,
	tool: &str,
) -> Option<String> {
	let server = server?;
	let lookup = get_or_build_lookup(session_id, config);
	if let Some(name) = lookup.exact.get(&(server.to_string(), tool.to_string())) {
		return Some(name.clone());
	}
	if let Some(name) = lookup.wildcard.get(server) {
		return Some(name.clone());
	}
	// Dynamic activation: scan runtime overlay for this (server, tool).
	for (cap_name, per_server) in crate::config::runtime_overlay::snapshot() {
		if let Some(tools) = per_server.get(server) {
			if tools.iter().any(|t| t == tool) {
				return Some(cap_name);
			}
		}
	}
	None
}

/// Append a successful tool call to the session log.
pub fn record_call(session_id: &SessionId, capability: Option<String>, params: Value) {
	let mut guard = CALL_LOG.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	registry
		.entry(session_id.clone())
		.or_default()
		.push((capability, params));
}

/// Sequentially evaluate guardrails for an ordered batch of tool calls,
/// returning per-call deny messages (or `None` to allow). Allowed calls are
/// recorded immediately so later calls in the same batch can see them via
/// `+/-` history conditions. Blocked calls are NOT recorded — they don't
/// run, so they shouldn't satisfy history requirements on retry.
pub fn check_batch(
	session_id: &SessionId,
	config: &crate::config::Config,
	calls: &[crate::mcp::McpToolCall],
) -> Vec<Option<String>> {
	// Two reasons to traverse the batch:
	//   1. Evaluate [[guard]] rules and produce per-call deny messages.
	//   2. Append allowed calls to the session call log so later phases
	//      (hooks via inspection, validators via `when`) see them.
	// The log MUST be recorded even when no guards exist — validators read
	// from the same log, and skipping recording would make `+used` checks
	// fail on perfectly valid runs.
	let rules_opt = get_rules(session_id);
	let has_guards = rules_opt
		.as_ref()
		.map(|r| !r.guards.is_empty())
		.unwrap_or(false);
	let loaded: std::collections::HashSet<String> = config
		.mcp
		.servers
		.iter()
		.map(|s| s.name().to_string())
		.collect();
	let mut out = Vec::with_capacity(calls.len());
	for call in calls {
		let server = crate::mcp::tool_map::get_server_for_tool(&call.tool_name)
			.map(|s| s.name().to_string());
		let cap = resolve_capability(session_id, config, server.as_deref(), &call.tool_name);
		let msg = if has_guards {
			let log = get_call_log(session_id);
			crate::config::guardrails::check(
				rules_opt.as_ref().unwrap(),
				cap.as_deref(),
				&call.parameters,
				&log,
				&loaded,
			)
		} else {
			None
		};
		if msg.is_none() {
			record_call(session_id, cap, call.parameters.clone());
		}
		out.push(msg);
	}
	out
}

pub fn get_call_log(session_id: &SessionId) -> CallLog {
	let guard = match CALL_LOG.read() {
		Ok(g) => g,
		Err(_) => return Vec::new(),
	};
	guard
		.as_ref()
		.and_then(|r| r.get(session_id))
		.cloned()
		.unwrap_or_default()
}

/// Increment the message counter for this session and return the new count.
pub fn increment_message_count(session_id: &SessionId) -> u64 {
	let mut guard = MESSAGE_COUNTS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let count = registry.entry(session_id.clone()).or_insert(0);
	*count += 1;
	*count
}

/// Increment the run counter for a specific pipe and return the new count.
pub fn increment_pipe_run_count(session_id: &SessionId, pipe_name: &str) -> u64 {
	let mut guard = PIPE_RUN_COUNTS.write().unwrap();
	let registry = guard.get_or_insert_with(HashMap::new);
	let per_session = registry.entry(session_id.clone()).or_default();
	let count = per_session.entry(pipe_name.to_string()).or_insert(0);
	*count += 1;
	*count
}
