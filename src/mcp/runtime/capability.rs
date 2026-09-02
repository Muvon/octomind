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

//! Capability tool — runtime discovery and activation of capabilities.
//!
//! A capability is a domain abstraction (e.g., `database.postgres`,
//! `web.search`) that resolves to one or more MCP servers and a set of
//! allowed tools. Taps declare must-have capabilities in agent manifests;
//! those are merged into the effective config at boot.
//!
//! Two activation paths exist:
//!
//! - **Deterministic auto-activation** (preferred). On every fresh user
//!   message, `auto_activate_capabilities` embeds the intent and matches
//!   it against each inactive capability's hand-authored `triggers`
//!   (mean-of-top-K cosine + margin gate). On a hit, the capability's
//!   MCP servers are registered and enabled directly — no LLM in the
//!   routing loop, no extra tool-call turn.
//!
//! - **Manual via this tool** (fallback). The `capability` tool exposes
//!   `list`, `enable`, `disable`, `discover` for cases where auto-
//!   activation didn't fire (offline, model still warming up, intent
//!   too ambiguous to clear the margin gate).
//!
//! Actions:
//! - `list`     — show all installed capabilities (active marked).
//! - `enable`   — activate a capability by name (registers + enables its MCP servers).
//! - `disable`  — deactivate a previously-enabled capability.
//! - `discover` — find capabilities matching an intent string (semantic match via embeddings, falls back to keyword match).

use crate::config::Config;
use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Active capabilities registry (process-global; mirrors dynamic.rs pattern)
//
// We track per-capability state so we can do LRU eviction when the active
// set hits a soft cap. Eviction is the only auto-disable mechanism; we
// deliberately don't time-decay or domain-shift evict because production
// agent UX is hurt more by false-disable than by carrying an idle cap.
// ---------------------------------------------------------------------------

/// State for one active capability. `server_tools` is the list of MCP
/// servers + the bare tool names this capability registered when it was
/// activated. Per-server tool granularity is required because multiple
/// capabilities can share one MCP server (e.g. `codesearch` exposes
/// `semantic_search`+`view_signatures` while `codesearch-graph` exposes
/// `graphrag`, both backed by the same `octocode` server). On eviction
/// we strip only THIS cap's tools and only kill the server when no other
/// active cap still references it (refcount → 0). `last_used` updates on
/// every successful tool call from any of these servers; LRU eviction
/// picks the entry with the smallest `last_used`.
#[derive(Debug, Clone)]
struct CapState {
	server_tools: Vec<(String, Vec<String>)>,
	last_used: Instant,
}

/// Soft cap on simultaneously-active capabilities. When a new activation
/// would exceed this, the LRU entry is disabled first to make room.
///
/// Sized to balance two pressures:
/// - **Tool overload research** (Microsoft, AWS, Boundary, Chroma) shows
///   sharp accuracy degradation past ~20-25 tools exposed to the model.
///   With baseline always-on tools (~15-20) plus ~4-5 tools per capability,
///   4 active caps keeps total tool surface in the safe zone (~35-40).
/// - **Real task concurrency** rarely needs more than 2-3 capabilities at
///   once; 4 leaves headroom for cross-domain tasks without churning.
///
/// Eviction is purely demand-driven: caps stay active indefinitely until
/// a new activation hits the cap. No background timers or idle cleanup.
const MAX_ACTIVE_CAPS: usize = 4;

/// Capabilities activated at runtime by this tool. Capabilities pre-loaded from
/// the tap manifest at boot are NOT tracked here — they are already merged into
/// the agent's effective config and represented as regular MCP servers.
static ACTIVE_CAPABILITIES: OnceLock<Arc<RwLock<HashMap<String, CapState>>>> = OnceLock::new();

fn registry() -> &'static Arc<RwLock<HashMap<String, CapState>>> {
	ACTIVE_CAPABILITIES.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

fn is_active(name: &str) -> bool {
	registry().read().unwrap().contains_key(name)
}

fn active_count() -> usize {
	registry().read().unwrap().len()
}

fn mark_active(name: &str, server_tools: Vec<(String, Vec<String>)>) {
	registry().write().unwrap().insert(
		name.to_string(),
		CapState {
			server_tools,
			last_used: Instant::now(),
		},
	);
}

// `mark_inactive` removed — handle_disable / evict_lru_if_full now
// remove cap entries directly under the same write lock that builds
// the per-server disable plan, so a separate helper is dead weight.

/// Find which active capability owns the given MCP server name and bump
/// its `last_used` to now. Called from the tool-call dispatch path so
/// LRU eviction tracks real usage, not just activation order.
pub(crate) fn touch_capability_for_server(server_name: &str) {
	let mut reg = registry().write().unwrap();
	for state in reg.values_mut() {
		if state.server_tools.iter().any(|(s, _)| s == server_name) {
			state.last_used = Instant::now();
			return;
		}
	}
}

/// Count how many active capabilities (other than `excluding`) still
/// reference `server_name`. Used by eviction to decide whether the
/// underlying MCP server should be fully shut down or only have its
/// caller's tools stripped from the global tool_map.
fn server_refcount(reg: &HashMap<String, CapState>, server_name: &str, excluding: &str) -> usize {
	reg.iter()
		.filter(|(name, _)| name.as_str() != excluding)
		.filter(|(_, st)| st.server_tools.iter().any(|(s, _)| s == server_name))
		.count()
}

/// Pure helper: find the entry with the smallest `last_used` and remove it.
/// Returns `(name, server_tools)` so the caller can disable the underlying
/// servers selectively; doesn't touch the dynamic-server registry itself.
/// Separated from `evict_lru_if_full` so the selection logic is unit-
/// testable without touching global state or needing a `Config`.
/// Per-capability tool ownership: (server_name, bare tool names this
/// cap registered on that server). Multiple caps can list the same
/// `server_name` with disjoint tool sets — refcount logic uses this.
pub(crate) type ServerToolGroups = Vec<(String, Vec<String>)>;

/// Disable plan entry: server name, the specific tools to strip from
/// the global tool_map, and whether to fully kill the server (true =
/// no other active cap references it).
type DisablePlanEntry = (String, Vec<String>, bool);

fn select_lru_in(map: &mut HashMap<String, CapState>) -> Option<(String, ServerToolGroups)> {
	let lru_name = map
		.iter()
		.min_by_key(|(_, st)| st.last_used)
		.map(|(n, _)| n.clone())?;
	let st = map.remove(&lru_name)?;
	Some((lru_name, st.server_tools))
}

/// If the active set is at or above the soft cap, evict the LRU entry
/// (lowest `last_used`) and disable its MCP-server tools. Logged at info
/// level so users see what flipped off.
///
/// Refcount-aware: for each (server, tools) the evicted cap registered,
/// the underlying server is fully shut down ONLY when no other active
/// cap still references that server name. Otherwise just THIS cap's
/// tools are stripped from the global tool_map and the server keeps
/// running for its other consumers.
///
/// Called before activating a new capability; idempotent when the active
/// set is below the cap. Errors disabling individual servers are logged
/// but don't block: we'd rather have the eviction happen with one stale
/// server than fail the new activation.
fn evict_lru_if_full(config: &Config) {
	if active_count() < MAX_ACTIVE_CAPS {
		return;
	}

	// Compute the disable plan under one write lock so refcounts are
	// consistent: read the LRU's server list, remove it from the
	// registry, then count remaining references for each server.
	//
	// `kill` here means "tear down the underlying MCP server process", not
	// "strip this cap's tools". Two reasons to keep the server alive:
	//   1. Another active capability still references it (refcount > 0).
	//   2. The role's static config declares it — the role still owns it
	//      regardless of dynamic-cap activity.
	let plan: Option<(String, Vec<DisablePlanEntry>)> = {
		let mut reg = registry().write().unwrap();
		select_lru_in(&mut reg).map(|(lru_name, server_tools)| {
			let entries = server_tools
				.into_iter()
				.map(|(srv, tools)| {
					let static_owned = config.mcp.servers.iter().any(|s| s.name() == srv);
					let kill = !static_owned && server_refcount(&reg, &srv, &lru_name) == 0;
					(srv, tools, kill)
				})
				.collect();
			(lru_name, entries)
		})
	};

	if let Some((name, entries)) = plan {
		// Drop overlay contributions before stripping tools so the next
		// merge sees the reduced filter.
		crate::config::runtime_overlay::clear_capability_extras(&name);

		let server_count = entries.len();
		for (srv, tools, kill) in &entries {
			if let Err(e) =
				crate::mcp::runtime::dynamic::disable_server_tools(srv, tools, *kill, Some(config))
			{
				crate::log_debug!(
					"capability LRU evict: failed to disable tools for server '{}' (kill={}, {} tools): {}",
					srv,
					kill,
					tools.len(),
					e
				);
			}
		}
		crate::log_info!(
			"capability LRU evicted: '{}' ({} server-tool-group(s) processed)",
			name,
			server_count
		);
	}
}

// ---------------------------------------------------------------------------
// McpFunction definition
// ---------------------------------------------------------------------------

pub fn get_capability_function() -> McpFunction {
	McpFunction {
		name: "capability".to_string(),
		description: r#"Discover and activate capabilities mid-session. Capabilities are domain bundles (e.g., database-postgres, filesystem, kubernetes) that resolve to MCP servers and tools. Use when the agent needs functionality outside its starting kit.

Actions:
- list: show all installed capabilities. Active ones are marked. One line per capability: name + brief description.
- enable: activate a capability by name. Registers and enables its MCP servers, exposing tools in subsequent turns.
- disable: deactivate a previously-enabled capability.
- discover: find capabilities matching an intent string (semantic match via embeddings, falls back to keyword match).

Workflow: call list or discover to find the right capability, then enable to activate it. Tool surface grows on demand. When intent is generic (e.g. 'I need a database') and multiple capabilities could fit, prefer list or discover over guessing."#.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"action": {
					"type": "string",
					"enum": ["list", "enable", "disable", "discover"],
					"description": "Action to perform"
				},
				"name": {
					"type": "string",
					"description": "Capability name (required for enable and disable). Bare name searches every tap; octomind/<name> pins the baseline tap, <org>/<name> a connected tap."
				},
				"intent": {
					"type": "string",
					"description": "Free-text intent for discover action (e.g., 'I need to query a database')"
				}
			},
			"required": ["action"]
		}),
	}
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Filter a capability list down to those available in the current session's
/// domain. Wraps `agent::registry::cap_available_in_domain` against the
/// session-scoped domain string from `session::context::current_session_domain`.
///
/// When no domain is set (early init / out-of-session tool calls), only
/// universal (empty-`domains`) caps survive — strict interpretation of the
/// hard-bound rule: a domain-restricted cap requires a known domain context.
fn filter_caps_by_domain(
	caps: Vec<crate::agent::registry::ResolvedCapability>,
) -> Vec<crate::agent::registry::ResolvedCapability> {
	let domain = crate::session::context::current_session_domain();
	let domain_ref: &str = domain.as_deref().unwrap_or("");
	caps.into_iter()
		.filter(|c| crate::agent::registry::cap_available_in_domain(&c.domains, domain_ref))
		.collect()
}

/// Domain check for a single capability's domains list. Same rule as
/// `filter_caps_by_domain` but for the single-cap activation path
/// (`handle_enable`, `activate_capability_inline`).
fn cap_in_current_domain(domains: &[String]) -> bool {
	let cur = crate::session::context::current_session_domain();
	let cur_ref: &str = cur.as_deref().unwrap_or("");
	crate::agent::registry::cap_available_in_domain(domains, cur_ref)
}

pub async fn execute_capability_command(
	call: &McpToolCall,
	config: &Config,
) -> Result<McpToolResult> {
	let action = match call.parameters.get("action").and_then(|v| v.as_str()) {
		Some(a) if !a.trim().is_empty() => a.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'action'".to_string(),
			));
		}
	};
	match action.as_str() {
		"list" => handle_list(call, config).await,
		"enable" => handle_enable(call, config).await,
		"disable" => handle_disable(call, config).await,
		"discover" => handle_discover(call, config).await,
		other => Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Unknown action '{other}'. Use list, enable, disable, or discover."),
		)),
	}
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_list(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to enumerate capabilities: {e}"),
			));
		}
	};
	let caps = filter_caps_by_domain(caps);
	if caps.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No capabilities installed in any tap.".to_string(),
		));
	}
	let mut output = format!("Installed capabilities ({}):\n", caps.len());
	for cap in &caps {
		let env_missing = if cap.required_env_keys.is_empty() {
			None
		} else {
			check_env_readiness(&cap.required_env_keys).err()
		};
		if let Some(missing) = &env_missing {
			crate::log_debug!(
				"capability list: '{}' not env-ready — missing: {}",
				cap.name,
				missing.join(", ")
			);
		}
		let marker = if is_active(&cap.name) {
			"[active] "
		} else if env_missing.is_some() {
			"[missing env] "
		} else {
			""
		};
		let env_note = match &env_missing {
			Some(missing) => format!(" (missing env: {})", missing.join(", ")),
			None => String::new(),
		};
		output.push_str(&format!(
			"- {}{} — {}{}\n",
			marker,
			cap.name,
			triggers_preview(&cap.triggers),
			env_note
		));
	}
	output.push_str("\nUse capability(action=\"enable\", name=\"<name>\") to activate.");
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
}

/// Render the first few triggers of a capability as a comma-separated
/// preview so users see *what they'd say* to invoke it. More useful than
/// a hand-written description.
fn triggers_preview(triggers: &[String]) -> String {
	let take = triggers.iter().take(3).cloned().collect::<Vec<_>>();
	let suffix = if triggers.len() > 3 { ", …" } else { "" };
	format!(
		"{}{}",
		take.iter()
			.map(|t| format!("\"{t}\""))
			.collect::<Vec<_>>()
			.join(", "),
		suffix
	)
}

async fn handle_enable(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let name = match call.parameters.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if is_active(crate::agent::registry::capability_bare_name(&name)) {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Capability '{name}' is already active."),
		));
	}

	let resolved = match crate::agent::registry::parse_capability_toml(&name, &config.capabilities)
	{
		Ok(r) => r,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Capability '{name}' not found: {e}"),
			));
		}
	};

	// Env readiness gate: refuse to activate a capability whose required
	// env vars are not set. Prevents activating a server that will fail
	// at first use because its API key is missing.
	if !resolved.required_env_keys.is_empty() {
		if let Err(missing) = check_env_readiness(&resolved.required_env_keys) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Capability '{name}' requires env vars: {} — set them before activating.",
					missing.join(", ")
				),
			));
		}
	}

	// Domain gate. Refuses to enable a capability that's bound to other
	// domains, regardless of whether the user invoked `capability enable`
	// directly, the auto-activator routed here, or `OCTOMIND_CAPABILITIES`
	// env-loaded it at boot. Hard-bound — no bypass.
	if !cap_in_current_domain(&resolved.domains) {
		let current = crate::session::context::current_session_domain()
			.unwrap_or_else(|| "<unknown>".to_string());
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Capability '{name}' is bound to domains {:?}; current domain is '{current}'. \
				 Run the matching role (e.g. `octomind run {}:general`) to access it.",
				resolved.domains,
				resolved
					.domains
					.first()
					.map(String::as_str)
					.unwrap_or("<domain>"),
			),
		));
	}

	// Deps-only capabilities (no MCP servers): activation runs the dep
	// installers — that IS the activation. Toolchain caps like
	// `programming-nodejs` use this path to install node/npm/npx so the
	// agent's shell can use them. Genuinely empty caps (no servers AND no
	// deps) remain an error.
	if resolved.mcp_servers.is_empty() {
		if resolved.deps.is_empty() {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Capability '{name}' has no [[mcp.servers]] and no [deps] — nothing to activate."),
			));
		}
		evict_lru_if_full(config);
		if let Err(e) =
			crate::agent::deps::run_dep_entries(&resolved.deps, &resolved.tap_root, None).await
		{
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to install deps for capability '{name}': {e:#}"),
			));
		}
		mark_active(&resolved.name, Vec::new());
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Capability '{name}' enabled. Installed deps: {}",
				resolved.deps.join(", ")
			),
		));
	}

	// Make room before activating — drops the LRU active capability if
	// we'd exceed `MAX_ACTIVE_CAPS`. No-op when below the cap.
	evict_lru_if_full(config);

	let mut activated_tools: Vec<String> = Vec::new();
	let mut activated_servers: Vec<String> = Vec::new();
	// Per-(server, bare-tool-names) record we hand to `mark_active` so
	// LRU eviction can strip only THIS cap's tools when servers are
	// shared with other active caps. See CapState docs.
	let mut activated_server_tools: Vec<(String, Vec<String>)> = Vec::new();
	// Track whether *any* server activation passed a non-empty filter, so
	// the success message can distinguish "all-tools" from "filter-applied".
	let mut any_filter_applied = false;

	// Per-server tool contributions for the runtime overlay. Only servers
	// that are already in the role's static config get an overlay entry —
	// fully-dynamic servers are surfaced through `dynamic::get_all_functions`
	// and don't need overlay extras to be visible.
	let mut overlay_per_server: std::collections::HashMap<String, Vec<String>> =
		std::collections::HashMap::new();

	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();

		// Compute the filter for *this* server. `allowed_tools` patterns in
		// capability TOMLs are namespace-prefixed (e.g., `playwright:*`,
		// `playwright:browser_navigate`) so a single capability config can
		// scope tools across multiple MCP servers. But `enable_server`
		// matches against the *bare* tool names returned by the server
		// (e.g., `browser_navigate`, not `playwright:browser_navigate`),
		// so we strip the `<server>:` prefix here. Patterns for *other*
		// servers in the same cap are dropped for this server. Patterns
		// without any namespace apply to every server.
		let filter_for_this = filter_for_server(&resolved.allowed_tools, &server_name);
		if filter_for_this.is_some() {
			any_filter_applied = true;
		}

		// Two activation paths share the same registry/overlay/tool-map
		// shape so disable/eviction is uniform regardless of where the
		// server originated.
		//
		// 1. Server already in the role's static config (declared by the
		//    role's `capabilities = [...]` at boot). The MCP init already
		//    exposes its tools via the static path, but this capability's
		//    `allowed_tools` for that server may include names the role's
		//    own filter rejects. We extend the role's effective filter
		//    via the runtime overlay (consulted by
		//    `RoleMcpConfig::get_enabled_servers`) AND register the bare
		//    tool names in the global tool_map so dispatch can route them.
		// 2. Server is fully dynamic (capability brought it in at runtime).
		//    Register + enable through the dynamic registry as before; the
		//    dynamic `get_all_functions` path surfaces its tools, no
		//    overlay needed.
		let already_in_static = config.mcp.servers.iter().any(|s| s.name() == server_name);

		if already_in_static {
			let bare_names: Vec<String> = filter_for_this.clone().unwrap_or_default();

			// Register THIS cap's named tools in the global tool_map so the
			// dispatcher can route a call like `octofs:shell` even though
			// the role's static filter never listed it. Empty `bare_names`
			// (capability allows all tools from this server) is a no-op
			// here — the static path already mapped them.
			if !bare_names.is_empty() {
				if let Some(server_config) =
					config.mcp.servers.iter().find(|s| s.name() == server_name)
				{
					crate::mcp::tool_map::register_dynamic_server_tools(
						&server_name,
						server_config,
						&bare_names,
					);
					crate::mcp::server::clear_function_cache_for_server(&server_name);
				}
				overlay_per_server.insert(server_name.clone(), bare_names.clone());
			}

			activated_tools.extend(bare_names.iter().cloned());
			activated_server_tools.push((server_name.clone(), bare_names));
			activated_servers.push(server_name);
			continue;
		}

		// Fully dynamic — register + enable.
		if !crate::mcp::runtime::dynamic::is_dynamic(&server_name) {
			if let Err(e) = crate::mcp::runtime::dynamic::register_server(server.clone()) {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Failed to register server '{server_name}' for capability '{name}': {e}"
					),
				));
			}
		}

		match crate::mcp::runtime::dynamic::enable_server(&server_name, filter_for_this).await {
			Ok(functions) => {
				let bare_names: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();
				activated_tools.extend(bare_names.iter().cloned());
				activated_server_tools.push((server_name.clone(), bare_names));
				activated_servers.push(server_name);
			}
			Err(e) => {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Failed to enable server '{server_name}' for capability '{name}': {e}"),
				));
			}
		}
	}

	// Publish overlay entries so the next config merge picks up this
	// capability's contributions to static servers' filters.
	crate::config::runtime_overlay::set_capability_extras(&resolved.name, overlay_per_server);

	mark_active(&resolved.name, activated_server_tools);

	// Don't mislead the LLM with "Tools available: none" when no filter
	// was applied — that path means "expose all server tools", and an
	// empty function list at activation time can simply mean the server
	// hasn't completed its tool-list handshake yet (e.g., Playwright MCP
	// initializes lazily). Saying "none" makes the agent disable the
	// server it just activated. Distinguish the three cases explicitly.
	let tools_summary = if !any_filter_applied {
		"all tools the server exposes (list populates on first use if empty now)".to_string()
	} else if activated_tools.is_empty() {
		"none — the configured allowed_tools filter excluded every tool the server reported"
			.to_string()
	} else {
		activated_tools.join(", ")
	};

	let msg = format!(
		"Capability '{name}' enabled. Activated {} server(s): {}\nTools available: {}",
		activated_servers.len(),
		activated_servers.join(", "),
		tools_summary
	);
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		msg,
	))
}

async fn handle_disable(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let name = match call.parameters.get("name").and_then(|v| v.as_str()) {
		Some(n) if !n.trim().is_empty() => {
			crate::agent::registry::capability_bare_name(n.trim()).to_string()
		}
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'name'".to_string(),
			));
		}
	};

	if !is_active(&name) {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!("Capability '{name}' is not active."),
		));
	}

	// Compute the disable plan under one write lock so refcounts are
	// consistent: pull THIS cap's (server, tools) record, remove the
	// cap from the registry, then count remaining references for each
	// server. Mirrors `evict_lru_if_full`.
	//
	// `kill` only flips true when no other active capability references
	// the server AND the server is not in the role's static config. The
	// static-config check stops `disable` from tearing down servers the
	// role still relies on (the LRU eviction path uses the same rule).
	let plan: Option<(CapState, Vec<DisablePlanEntry>)> = {
		let mut reg = registry().write().unwrap();
		reg.remove(&name).map(|state| {
			// Build the plan from a clone so the original state can be
			// re-inserted verbatim if any disable step fails mid-loop.
			let entries = state
				.server_tools
				.clone()
				.into_iter()
				.map(|(srv, tools)| {
					let static_owned = config.mcp.servers.iter().any(|s| s.name() == srv);
					let kill = !static_owned && server_refcount(&reg, &srv, &name) == 0;
					(srv, tools, kill)
				})
				.collect();
			(state, entries)
		})
	};

	let (original_state, plan) = match plan {
		Some(p) => p,
		None => {
			// Race: someone else evicted between is_active check and the
			// write-lock above. Treat as no-op.
			return Ok(McpToolResult::success(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Capability '{name}' is not active."),
			));
		}
	};

	// Drop the overlay entry so the next merge sees the reduced per-server
	// filter for static servers this cap was contributing to. Order matters:
	// clear before tool_map updates so the two stay in sync if a concurrent
	// merge reads them.
	crate::config::runtime_overlay::clear_capability_extras(&name);

	let mut disabled_servers: Vec<String> = Vec::new();
	for (srv, tools, kill) in &plan {
		// Always strip THIS cap's tool entries from the global tool_map,
		// even on static servers — the cap brought them in via the runtime
		// overlay, so they need to leave the map when it's disabled.
		// `kill=false` selects the strip-only path inside
		// `disable_server_tools`; static servers reach this branch via the
		// `static_owned` rule above.
		if let Err(e) =
			crate::mcp::runtime::dynamic::disable_server_tools(srv, tools, *kill, Some(config))
		{
			// Re-insert the cap so the user can retry. Fail closed — partial
			// disable is worse than reporting the error. Partially stripped
			// servers are restored by the user retrying enable (enable
			// re-applies overlay + tools).
			registry()
				.write()
				.unwrap()
				.insert(name.clone(), original_state);
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to disable server '{srv}' for capability '{name}': {e}"),
			));
		}
		if *kill {
			disabled_servers.push(srv.clone());
		}
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		format!(
			"Capability '{name}' disabled. Fully shut down {} server(s): {}",
			disabled_servers.len(),
			if disabled_servers.is_empty() {
				"(none — all servers still in use by other active capabilities)".to_string()
			} else {
				disabled_servers.join(", ")
			}
		),
	))
}

// ---------------------------------------------------------------------------
// Deterministic auto-activation — embed each fresh user message and flip
// the matching capability on without a tool-call round-trip through the LLM.
//
// Why deterministic: agents are unreliable as routers and every extra
// tool-call turn costs money. The embedding layer is fast (≈30ms cold,
// cached thereafter), local (BGE-small-en-v1.5), and cheap. We trade a
// small false-positive risk — bounded by the margin gate — for not
// burning a turn on every capability decision.
//
// Algorithm:
//   1. Embed the user's message once.
//   2. Embed each inactive capability's `triggers` (cached, so this is
//      free after the first turn — triggers don't change mid-session).
//   3. Per capability: cosine vs each trigger, take the mean of the
//      top-K (K = 3). Aurelio Labs Semantic Router pattern; triggers
//      drag the centroid into the query distribution where one-line
//      descriptions don't reach.
//   4. Margin gate: activate iff `top1 >= THRESHOLD && top1 - top2 >= MARGIN`.
//      Single most important precision lever — ambiguous matches abstain
//      rather than activating the wrong capability.
//   5. On a hit, register + enable the underlying MCP servers directly.
//      The agent never sees the routing decision; it just gets a wider
//      tool surface next turn.
// ---------------------------------------------------------------------------

/// Mean-of-top-K cosine threshold a capability must clear to be auto-activated.
/// Tuned for `muvon/octomind-embed` (BGE-small-en-v1.5 fine-tune) over short
/// hand-authored triggers.
///
/// 0.45 is the post-fine-tune calibration. After fine-tuning, the FT model
/// places every matched-intent positive well above 0.55 (mean top1 cosine
/// on `eval_real` is ~0.7+), so the threshold is no longer the load-bearing
/// constraint — `AUTO_ACTIVATE_MARGIN` is. The floor is kept at 0.45 only
/// as a safety net for the bottom-tail of legitimately-matched intents that
/// score lower than typical; tightening it further trades recall for no
/// false-positive reduction, since the FT model already separates chitchat
/// / OOD inputs into a distinct cluster (see `_oos` sink label training in
/// octomind-tap/model/scripts/build_dataset.py).
///
/// Re-calibrate after every model retrain with
/// `octomind-tap/model/scripts/calibrate_thresholds.py`.
///
/// History: 0.42 (base BGE, recall-tuned) → 0.55 (base BGE, FP-tuned for
/// chitchat aversion) → 0.45 (FT model, margin is now the binding gate).
const AUTO_ACTIVATE_THRESHOLD: f32 = 0.45;

/// Required gap between top-1 and top-2 capability scores. Prevents
/// activating one of two near-tied capabilities (e.g. `database-postgres`
/// vs `database-mysql`) when the user's intent doesn't disambiguate.
/// Ambiguous matches abstain — the user (or the agent later via
/// `capability(action="discover")`) clarifies. Tightened from 0.05 because
/// the previous gap let near-ties through on generic chitchat where the
/// embedding produces low-but-similar cosines across multiple caps.
const AUTO_ACTIVATE_MARGIN: f32 = 0.08;

/// How many triggers per capability contribute to the per-cap score.
/// Mean-of-top-K smooths a single noisy trigger while still rewarding
/// capabilities whose authored examples align with the user's wording.
const AUTO_ACTIVATE_TOP_K: usize = 3;

/// Sort `(score, T)` pairs descending and return the top entry only if
/// `top1 >= threshold` and `top1 - top2 >= margin`. With a single
/// candidate, top2 is treated as 0.0. Sort is stable (Timsort) so ties
/// preserve insertion order. Pure helper, separated from the embedding-
/// driven path so threshold/margin behavior is unit-testable.
fn select_with_margin<T>(
	mut scored: Vec<(f32, T)>,
	threshold: f32,
	margin: f32,
) -> Option<(f32, T)> {
	if scored.is_empty() {
		return None;
	}
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	let top1_score = scored[0].0;
	if top1_score < threshold {
		return None;
	}
	let top2_score = scored.get(1).map(|x| x.0).unwrap_or(0.0);
	if top1_score - top2_score < margin {
		return None;
	}
	scored.into_iter().next()
}

/// Score one capability against the user's intent: mean of the top-K
/// cosines between the intent vector and each trigger vector. Empty
/// trigger lists score 0.0.
fn score_capability(intent_vec: &[f32], trigger_vecs: &[Vec<f32>]) -> f32 {
	if trigger_vecs.is_empty() {
		return 0.0;
	}
	let mut scores: Vec<f32> = trigger_vecs
		.iter()
		.map(|v| crate::embeddings::cosine(intent_vec, v))
		.collect();
	scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
	let take = scores.len().min(AUTO_ACTIVATE_TOP_K);
	let sum: f32 = scores.iter().take(take).sum();
	sum / take as f32
}

/// Inspect the most recent user message and, if a non-active capability
/// strongly matches, activate it directly via `dynamic::enable_server`.
/// Silent no-op when the model isn't ready, no capabilities are installed,
/// Progress events emitted by `load_env_capabilities`.
///
/// Mirrors `crate::mcp::McpInitProgress` so the boot spinner can drive both
/// phases (static MCP init + env-capability load) through one UI loop.
#[derive(Debug, Clone)]
pub enum EnvCapabilityProgress {
	/// Initial event with the full list of capabilities about to load.
	Starting { capabilities: Vec<String> },
	/// One capability finished (success or failure).
	Completed { capability: String, success: bool },
}

/// Load capabilities from the `OCTOMIND_CAPABILITIES` env var (if set).
///
/// Mirrors `skill_auto::load_env_skills`: parses a comma-separated list of
/// capability names and force-activates each one before the agent's first
/// turn. Bypasses both the auto-activation embedding pipeline and the
/// `capability` tool — capabilities listed here are always loaded,
/// regardless of intent matching.
///
/// `progress` is an optional callback driven during loading so a boot
/// spinner / TUI can show per-capability status alongside the standard MCP
/// init phase. Pass `None` for headless flows (ACP, WebSocket).
///
/// Failures are logged and skipped (never abort the session). Already-active
/// capabilities are no-ops. Use this from CLI / CI / non-interactive runs
/// that need a deterministic tool surface (e.g., `OCTOMIND_CAPABILITIES=cron,docker octomind run -r ...`).
pub async fn load_env_capabilities(
	config: &Config,
	progress: Option<&(dyn Fn(EnvCapabilityProgress) + Send + Sync)>,
) {
	let env_val = match std::env::var("OCTOMIND_CAPABILITIES") {
		Ok(v) if !v.trim().is_empty() => v,
		_ => return,
	};
	let names: Vec<String> = env_val
		.split(',')
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.collect();
	if names.is_empty() {
		return;
	}

	if let Some(cb) = progress {
		cb(EnvCapabilityProgress::Starting {
			capabilities: names.clone(),
		});
	}

	let suppress = crate::config::with_thread_config(|c| c.output_mode())
		.map(|m| m.should_suppress_cli_output())
		.unwrap_or(false);

	for name in &names {
		if is_active(name) {
			if let Some(cb) = progress {
				cb(EnvCapabilityProgress::Completed {
					capability: name.clone(),
					success: true,
				});
			}
			continue;
		}
		let call = crate::mcp::McpToolCall {
			tool_name: "capability".to_string(),
			tool_id: format!("env_{name}"),
			parameters: serde_json::json!({"action": "enable", "name": name}),
		};
		let success = match handle_enable(&call, config).await {
			Ok(result) if result.is_error() => {
				let msg = result.extract_content();
				if !suppress {
					eprintln!("OCTOMIND_CAPABILITIES: capability '{name}' failed: {msg}");
				} else {
					crate::log_debug!(
						"OCTOMIND_CAPABILITIES: capability '{}' failed: {}",
						name,
						msg
					);
				}
				false
			}
			Ok(_) => {
				crate::log_debug!("OCTOMIND_CAPABILITIES: enabled capability '{}'", name);
				true
			}
			Err(e) => {
				if !suppress {
					eprintln!("OCTOMIND_CAPABILITIES: capability '{name}' failed: {e:#}");
				} else {
					crate::log_debug!(
						"OCTOMIND_CAPABILITIES: capability '{}' failed: {:#}",
						name,
						e
					);
				}
				false
			}
		};
		if let Some(cb) = progress {
			cb(EnvCapabilityProgress::Completed {
				capability: name.clone(),
				success,
			});
		}
	}
}

/// Snapshot of currently-active capability names. Used by the boot flow to
/// print "Using capability: X" summary lines after env loading completes,
/// mirroring the per-skill summary lines.
pub fn list_active_names() -> Vec<String> {
	let mut names: Vec<String> = registry().read().unwrap().keys().cloned().collect();
	names.sort();
	names
}

/// Designed to run before every API request from `prepare_for_api_call`.
/// Does not block the hot path on model warmup — `is_ready` is consulted
/// first and skips silently while the model is still downloading.
pub async fn auto_activate_capabilities(
	session: &mut crate::session::chat::session::ChatSession,
	config: &Config,
) {
	// Fire only on a fresh user message. Tool-loop iterations are skipped.
	let intent = match session.session.messages.last() {
		Some(m) if crate::session::is_real_user_task_message(m) => m.content.clone(),
		_ => return,
	};

	let _ = auto_activate_capabilities_for_intent(&intent, config).await;
}

/// Trigger capability auto-activation for explicit intent text.
///
/// This is the same scoring path as fresh user-message activation, exposed for
/// runtime prompts that ask the session to load missing tools.
pub async fn auto_activate_capabilities_for_intent(intent: &str, config: &Config) -> Vec<String> {
	// Control-plane text (supervisor steers/recalls, skill replays, continuation
	// wrappers) is not a user intent — mirrors the same gate in
	// `skill_auto::run_activation`.
	if crate::session::is_system_managed_user_content(intent) {
		crate::log_debug!("capability auto-activate: skipping — system-managed content");
		return Vec::new();
	}

	// Strip XML blocks (skill injections, <log> pastes, system tags, etc.)
	// so pasted content doesn't drive false-positive capability matches.
	let intent = crate::mcp::runtime::skill_auto::strip_xml_blocks(intent);

	// Skip embedding + scoring entirely for short/empty inputs. Short
	// acknowledgments ("try", "ok", "do it") produce noisy embeddings that
	// can clear the threshold against an unrelated trigger by coincidence;
	// they also waste the embed call on no real intent. Mirrors the same
	// gate applied in `skill_auto::run_activation`.
	if !crate::mcp::runtime::skill_auto::intent_has_enough_signal(&intent) {
		crate::log_debug!(
			"capability auto-activate: skipping — intent below {} non-ws chars: {:?}",
			crate::mcp::runtime::skill_auto::MIN_INTENT_NON_WS_CHARS,
			intent
		);
		return Vec::new();
	}

	if !crate::embeddings::is_ready() {
		crate::log_debug!(
			"capability auto-activate: embedding model not ready yet, skipping this turn"
		);
		return Vec::new();
	}

	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			crate::log_debug!("capability auto-activate: enumeration failed ({})", e);
			return Vec::new();
		}
	};
	// Domain gate: skip out-of-domain caps before embedding their triggers.
	// Saves embed work AND prevents the gate from picking, say, medical-
	// reference for a `developer:general` user message that happens to score
	// well against medical-domain triggers.
	let caps = filter_caps_by_domain(caps);

	// Env readiness gate: filter out caps whose required env vars are not
	// set before scoring. Saves embedding compute on caps that can't
	// activate, and prevents the auto-activator from picking a cap that
	// would fail at activation time.
	let inactive: Vec<&crate::agent::registry::ResolvedCapability> = caps
		.iter()
		.filter(|c| {
			if is_active(&c.name) {
				return false;
			}
			if let Err(missing) = check_env_readiness(&c.required_env_keys) {
				crate::log_debug!(
					"capability auto-activate: filtering out '{}' — missing env vars: {}",
					c.name,
					missing.join(", ")
				);
				return false;
			}
			true
		})
		.collect();
	if inactive.is_empty() {
		return Vec::new();
	}

	let intent_vec = match crate::embeddings::embed(&intent).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("capability auto-activate: intent embed failed ({})", e);
			return Vec::new();
		}
	};

	// Flatten all triggers into one batch to amortize the embed call.
	// `embed_many` caches by content hash, so subsequent turns are free.
	let mut flat: Vec<String> = Vec::new();
	let mut offsets: Vec<(usize, usize)> = Vec::with_capacity(inactive.len());
	for cap in &inactive {
		let start = flat.len();
		flat.extend(cap.triggers.iter().cloned());
		offsets.push((start, flat.len()));
	}
	if flat.is_empty() {
		return Vec::new();
	}

	let trigger_vecs = match crate::embeddings::embed_many(&flat).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("capability auto-activate: trigger embed failed ({})", e);
			return Vec::new();
		}
	};

	let scored: Vec<(f32, &crate::agent::registry::ResolvedCapability)> = inactive
		.iter()
		.zip(offsets.iter())
		.map(|(cap, (start, end))| {
			let score = score_capability(&intent_vec, &trigger_vecs[*start..*end]);
			(score, *cap)
		})
		.collect();

	let mut ranked: Vec<(f32, String)> = scored.iter().map(|(s, c)| (*s, c.name.clone())).collect();
	ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	let preview: Vec<String> = ranked
		.iter()
		.take(5)
		.map(|(s, n)| format!("{n}={s:.3}"))
		.collect();
	crate::log_debug!(
		"capability auto-activate: intent={:?} candidates={} threshold={} margin={} top5=[{}]",
		intent,
		ranked.len(),
		AUTO_ACTIVATE_THRESHOLD,
		AUTO_ACTIVATE_MARGIN,
		preview.join(", ")
	);

	let top = select_with_margin(scored, AUTO_ACTIVATE_THRESHOLD, AUTO_ACTIVATE_MARGIN);

	if let Some((score, cap)) = top {
		match activate_capability_inline(&cap.name, config).await {
			Ok(servers) => {
				crate::log_info!(
					"· capability auto-activated: '{}' (score {:.2}) — servers: [{}]",
					cap.name,
					score,
					servers.join(", ")
				);
				return vec![cap.name.clone()];
			}
			Err(e) => {
				crate::log_debug!(
					"capability auto-activate: failed to enable '{}' ({})",
					cap.name,
					e
				);
			}
		}
	} else {
		let top1 = ranked.first().map(|x| x.0).unwrap_or(0.0);
		let top2 = ranked.get(1).map(|x| x.0).unwrap_or(0.0);
		let top1_name = ranked.first().map(|x| x.1.as_str()).unwrap_or("<none>");
		let reason = if top1 < AUTO_ACTIVATE_THRESHOLD {
			format!(
				"top1 {top1:.3} below threshold {:.3}",
				AUTO_ACTIVATE_THRESHOLD
			)
		} else {
			format!(
				"margin {:.3} below required {:.3} (top1={top1:.3} top2={top2:.3})",
				top1 - top2,
				AUTO_ACTIVATE_MARGIN
			)
		};
		crate::log_debug!(
			"capability auto-activate: no winner — {} (top1 was '{}')",
			reason,
			top1_name
		);
	}

	Vec::new()
}

/// Translate capability `allowed_tools` patterns into the bare-name
/// patterns `enable_server` expects, for one server.
///
/// Capability TOMLs use a namespaced convention (`<server>:<tool>` or
/// `<server>:*`) so a single capability config can scope tools across
/// multiple MCP servers. The actual tool names returned by an MCP
/// server are bare (`browser_navigate`, not `playwright:browser_navigate`),
/// so we strip the prefix here. Rules:
///
/// - `<server_name>:<rest>` → `<rest>` (applies to this server)
/// - `<other>:<...>` → dropped (pattern is for a different server)
/// - `<bare_name_or_glob>` → unchanged (applies to all servers in cap)
///
/// Returns `None` when the input list is empty (no filter ⇒ all tools)
/// or all patterns are scoped to other servers (also "no filter for me",
/// expose all). Returns `Some(...)` only when at least one pattern
/// genuinely scopes this server.
fn filter_for_server(allowed_tools: &[String], server_name: &str) -> Option<Vec<String>> {
	if allowed_tools.is_empty() {
		return None;
	}
	let prefix = format!("{server_name}:");
	let kept: Vec<String> = allowed_tools
		.iter()
		.filter_map(|p| {
			if let Some(rest) = p.strip_prefix(&prefix) {
				Some(rest.to_string())
			} else if p.contains(':') {
				None
			} else {
				Some(p.clone())
			}
		})
		.collect();
	if kept.is_empty() {
		None
	} else {
		Some(kept)
	}
}

/// Register + enable a capability's MCP servers and mark the capability
/// active. Mirrors `handle_enable`'s logic minus the `McpToolResult`
/// wrapping — errors propagate as `anyhow::Error` for the caller to log
/// Check that all required env keys are set and non-empty.
/// Returns Ok(()) if all present, Err(missing_keys) listing the unset ones.
pub(crate) fn check_env_readiness(required: &[String]) -> Result<(), Vec<String>> {
	let missing: Vec<String> = required
		.iter()
		.filter(|key| std::env::var(key).map(|v| v.is_empty()).unwrap_or(true))
		.cloned()
		.collect();
	if missing.is_empty() {
		Ok(())
	} else {
		Err(missing)
	}
}

/// or surface. Idempotent: returns `Ok(empty)` when already active.
async fn activate_capability_inline(name: &str, config: &Config) -> Result<Vec<String>> {
	if is_active(crate::agent::registry::capability_bare_name(name)) {
		return Ok(Vec::new());
	}
	let resolved = crate::agent::registry::parse_capability_toml(name, &config.capabilities)?;
	if let Err(missing) = check_env_readiness(&resolved.required_env_keys) {
		crate::log_debug!(
			"capability activation: skipping '{}' — missing env vars: {}",
			name,
			missing.join(", ")
		);
		anyhow::bail!(
			"capability '{}' requires env vars: {} — set them before activating",
			name,
			missing.join(", ")
		);
	}
	// Deps-only capability: activation installs its toolchain. Mirrors
	// `handle_enable` so auto-activation and manual `enable` behave the same.
	if resolved.mcp_servers.is_empty() {
		if resolved.deps.is_empty() {
			anyhow::bail!("capability '{}' has no [[mcp.servers]] and no [deps]", name);
		}
		evict_lru_if_full(config);
		crate::agent::deps::run_dep_entries(&resolved.deps, &resolved.tap_root, None)
			.await
			.with_context(|| format!("dep install failed for capability '{name}'"))?;
		mark_active(&resolved.name, Vec::new());
		return Ok(Vec::new());
	}
	// Make room before activating — drops the LRU active capability if
	// we'd exceed `MAX_ACTIVE_CAPS`. No-op when below the cap.
	evict_lru_if_full(config);

	let mut activated_servers: Vec<String> = Vec::new();
	let mut activated_server_tools: Vec<(String, Vec<String>)> = Vec::new();
	let mut overlay_per_server: std::collections::HashMap<String, Vec<String>> =
		std::collections::HashMap::new();
	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();
		let filter = filter_for_server(&resolved.allowed_tools, &server_name);

		// Server already provided by the role's static config — extend
		// rather than re-register. Mirrors the `already_in_static` branch
		// in `handle_enable`. The overlay extends the role's per-server
		// filter at next merge; tool_map registration makes named tools
		// dispatchable now.
		if config.mcp.servers.iter().any(|s| s.name() == server_name) {
			let bare_names: Vec<String> = filter.clone().unwrap_or_default();
			if !bare_names.is_empty() {
				if let Some(server_config) =
					config.mcp.servers.iter().find(|s| s.name() == server_name)
				{
					crate::mcp::tool_map::register_dynamic_server_tools(
						&server_name,
						server_config,
						&bare_names,
					);
					crate::mcp::server::clear_function_cache_for_server(&server_name);
				}
				overlay_per_server.insert(server_name.clone(), bare_names.clone());
			}
			activated_server_tools.push((server_name.clone(), bare_names));
			activated_servers.push(server_name);
			continue;
		}

		if !crate::mcp::runtime::dynamic::is_dynamic(&server_name) {
			crate::mcp::runtime::dynamic::register_server(server.clone())?;
		}
		let functions = crate::mcp::runtime::dynamic::enable_server(&server_name, filter).await?;
		let bare_names: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();
		activated_server_tools.push((server_name.clone(), bare_names));
		activated_servers.push(server_name);
	}

	crate::config::runtime_overlay::set_capability_extras(&resolved.name, overlay_per_server);
	mark_active(&resolved.name, activated_server_tools);
	Ok(activated_servers)
}

async fn handle_discover(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
	let intent = match call.parameters.get("intent").and_then(|v| v.as_str()) {
		Some(i) if !i.trim().is_empty() => i.trim().to_string(),
		_ => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter 'intent'".to_string(),
			));
		}
	};

	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Failed to enumerate capabilities: {e}"),
			));
		}
	};
	let caps = filter_caps_by_domain(caps);

	if caps.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No capabilities installed in any tap.".to_string(),
		));
	}

	// Embedding-only — same scoring pipeline as auto-activation, just with
	// the threshold/margin gate replaced by "return top 5". No keyword
	// fallback: capability authors give us hand-authored triggers, the
	// SOTA path runs always.
	let scored = match score_caps_by_triggers(&intent, &caps).await {
		Ok(s) => s,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!(
					"Capability discover requires the embedding model. Init failed: {e}. \
					 If the model is still downloading, retry in a moment."
				),
			));
		}
	};

	let top: Vec<_> = scored.into_iter().take(5).collect();
	if top.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"No capabilities matched intent '{intent}'. Try `capability list` to see all installed capabilities."
			),
		));
	}

	let mut output = format!("Capabilities matching '{intent}':\n");
	for (score, cap) in top {
		let marker = if is_active(&cap.name) {
			"[active] "
		} else {
			""
		};
		output.push_str(&format!(
			"- {}{} (score {:.2}) — {}\n",
			marker,
			cap.name,
			score,
			triggers_preview(&cap.triggers)
		));
	}
	output.push_str("\nUse capability(action=\"enable\", name=\"<name>\") to activate.");
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
}

/// Score every capability by mean-of-top-K cosine over its triggers —
/// the same pipeline `auto_activate_capabilities` uses, just without the
/// threshold/margin gate. Returns capabilities sorted by score descending,
/// filtered to scores above a low noise floor (0.2) so empty intents
/// don't pull every capability into the result.
async fn score_caps_by_triggers<'a>(
	intent: &str,
	caps: &'a [crate::agent::registry::ResolvedCapability],
) -> Result<Vec<(f32, &'a crate::agent::registry::ResolvedCapability)>> {
	let intent_vec = crate::embeddings::embed(intent).await?;

	let mut flat: Vec<String> = Vec::new();
	let mut offsets: Vec<(usize, usize)> = Vec::with_capacity(caps.len());
	for cap in caps {
		let start = flat.len();
		flat.extend(cap.triggers.iter().cloned());
		offsets.push((start, flat.len()));
	}
	let trigger_vecs = crate::embeddings::embed_many(&flat).await?;

	let mut scored: Vec<(f32, &crate::agent::registry::ResolvedCapability)> = caps
		.iter()
		.zip(offsets.iter())
		.map(|(cap, (start, end))| {
			let score = score_capability(&intent_vec, &trigger_vecs[*start..*end]);
			(score, cap)
		})
		.filter(|(score, _)| *score > 0.2)
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	Ok(scored)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "capability_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "capability_command_tests.rs"]
mod command_tests;
