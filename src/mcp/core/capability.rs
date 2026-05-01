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

/// State for one active capability. `servers` is the list of MCP server
/// names this capability registered when it was activated — stored here
/// so eviction can disable them without re-parsing the TOML. `last_used`
/// updates on every successful tool call from any of these servers; LRU
/// eviction picks the entry with the smallest `last_used`.
#[derive(Debug, Clone)]
struct CapState {
	servers: Vec<String>,
	last_used: Instant,
}

/// Soft cap on simultaneously-active capabilities. When a new activation
/// would exceed this, the LRU entry is disabled first to make room.
/// Sized loose enough that normal sessions never hit it; only flapping
/// or runaway accumulation does.
const MAX_ACTIVE_CAPS: usize = 8;

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

fn mark_active(name: &str, servers: Vec<String>) {
	registry().write().unwrap().insert(
		name.to_string(),
		CapState {
			servers,
			last_used: Instant::now(),
		},
	);
}

fn mark_inactive(name: &str) {
	registry().write().unwrap().remove(name);
}

/// Find which active capability owns the given MCP server name and bump
/// its `last_used` to now. Called from the tool-call dispatch path so
/// LRU eviction tracks real usage, not just activation order.
pub(crate) fn touch_capability_for_server(server_name: &str) {
	let mut reg = registry().write().unwrap();
	for state in reg.values_mut() {
		if state.servers.iter().any(|s| s == server_name) {
			state.last_used = Instant::now();
			return;
		}
	}
}

/// Pure helper: find the entry with the smallest `last_used` and remove it.
/// Returns `(name, servers)` so the caller can disable the underlying
/// servers; doesn't touch the dynamic-server registry itself. Separated
/// from `evict_lru_if_full` so the selection logic is unit-testable
/// without touching global state or needing a `Config`.
fn select_lru_in(map: &mut HashMap<String, CapState>) -> Option<(String, Vec<String>)> {
	let lru_name = map
		.iter()
		.min_by_key(|(_, st)| st.last_used)
		.map(|(n, _)| n.clone())?;
	let st = map.remove(&lru_name)?;
	Some((lru_name, st.servers))
}

/// If the active set is at or above the soft cap, evict the LRU entry
/// (lowest `last_used`) and disable its underlying MCP servers. Logged
/// at info level so users see what flipped off.
///
/// Called before activating a new capability; idempotent when the active
/// set is below the cap. Errors disabling individual servers are logged
/// but don't block: we'd rather have the eviction happen with one stale
/// server than fail the new activation.
fn evict_lru_if_full(config: &Config) {
	if active_count() < MAX_ACTIVE_CAPS {
		return;
	}
	let evicted = {
		let mut reg = registry().write().unwrap();
		select_lru_in(&mut reg)
	};
	if let Some((name, servers)) = evicted {
		for server_name in &servers {
			if let Err(e) = crate::mcp::core::dynamic::disable_server(server_name, Some(config)) {
				crate::log_debug!(
					"capability LRU evict: failed to disable server '{}' ({})",
					server_name,
					e
				);
			}
		}
		crate::log_info!(
			"capability LRU evicted: '{}' ({} server(s) disabled to make room)",
			name,
			servers.len()
		);
	}
}

// ---------------------------------------------------------------------------
// McpFunction definition
// ---------------------------------------------------------------------------

pub fn get_capability_function() -> McpFunction {
	McpFunction {
		name: "capability".to_string(),
		description: r#"Discover and activate capabilities mid-session. Capabilities are domain bundles (e.g., "database-postgres", "filesystem", "kubernetes") that resolve to MCP servers and tools. Use this when the agent needs functionality outside its starting kit.

Actions:
- `list`     — show all installed capabilities. Active ones are marked. Returns one line per capability: name + brief description.
- `enable`   — activate a capability by name. Registers and enables its MCP servers, exposing the capability's tools in subsequent turns.
- `disable`  — deactivate a previously-enabled capability.
- `discover` — find capabilities matching an intent string (semantic match via embeddings, falls back to keyword match).

Workflow: call `list` or `discover` to find the right capability, then `enable` to activate it. The agent's tool surface grows on demand; nothing loaded that wasn't asked for. When the user's intent is generic (e.g. "I need a database") and multiple capabilities could fit, prefer `list` or `discover` to surface options rather than guessing."#.to_string(),
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
					"description": "Capability name (required for enable and disable)"
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
	if caps.is_empty() {
		return Ok(McpToolResult::success(
			call.tool_name.clone(),
			call.tool_id.clone(),
			"No capabilities installed in any tap.".to_string(),
		));
	}
	let mut output = format!("Installed capabilities ({}):\n", caps.len());
	for cap in &caps {
		let marker = if is_active(&cap.name) {
			"[active] "
		} else {
			""
		};
		output.push_str(&format!(
			"- {}{} — {}\n",
			marker,
			cap.name,
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

	if is_active(&name) {
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
		mark_active(&name, Vec::new());
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
	// Track whether *any* server activation passed a non-empty filter, so
	// the success message can distinguish "all-tools" from "filter-applied".
	let mut any_filter_applied = false;

	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();

		// Register if not already in the dynamic registry (idempotent on conflicts).
		if !crate::mcp::core::dynamic::is_dynamic(&server_name) {
			if let Err(e) = crate::mcp::core::dynamic::register_server(server.clone()) {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!(
						"Failed to register server '{server_name}' for capability '{name}': {e}"
					),
				));
			}
		}

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

		match crate::mcp::core::dynamic::enable_server(&server_name, filter_for_this).await {
			Ok(functions) => {
				activated_tools.extend(functions.iter().map(|f| f.name.clone()));
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

	mark_active(&name, activated_servers.clone());

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
		Some(n) if !n.trim().is_empty() => n.trim().to_string(),
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

	let resolved = match crate::agent::registry::parse_capability_toml(&name, &config.capabilities)
	{
		Ok(r) => r,
		Err(e) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Capability '{name}' not found (cannot determine servers to disable): {e}"),
			));
		}
	};

	let mut disabled_servers: Vec<String> = Vec::new();
	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();
		if crate::mcp::core::dynamic::is_dynamic(&server_name) {
			if let Err(e) = crate::mcp::core::dynamic::disable_server(&server_name, Some(config)) {
				return Ok(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					format!("Failed to disable server '{server_name}': {e}"),
				));
			}
			disabled_servers.push(server_name);
		}
	}

	mark_inactive(&name);

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		format!(
			"Capability '{name}' disabled. Deactivated {} server(s): {}",
			disabled_servers.len(),
			disabled_servers.join(", ")
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
/// Tuned for BGE-small-en-v1.5 over short hand-authored triggers — lower
/// than the 0.55 used previously against descriptions because triggers
/// match the user's natural-language distribution and produce tighter
/// clusters around real intents.
const AUTO_ACTIVATE_THRESHOLD: f32 = 0.42;

/// Required gap between top-1 and top-2 capability scores. Prevents
/// activating one of two near-tied capabilities (e.g. `database-postgres`
/// vs `database-mysql`) when the user's intent doesn't disambiguate.
/// Ambiguous matches abstain — the user (or the agent later via
/// `capability(action="discover")`) clarifies.
const AUTO_ACTIVATE_MARGIN: f32 = 0.05;

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
/// no match clears the gate, or the last message is not user input.
///
/// Designed to run before every API request from `prepare_for_api_call`.
/// Does not block the hot path on model warmup — `is_ready` is consulted
/// first and skips silently while the model is still downloading.
pub async fn auto_activate_capabilities(
	session: &mut crate::session::chat::session::ChatSession,
	config: &Config,
) {
	// Fire only on a fresh user message. Tool-loop iterations are skipped.
	let intent = match session.session.messages.last() {
		Some(m) if m.role == "user" => m.content.clone(),
		_ => return,
	};

	if !crate::embeddings::is_ready() {
		crate::log_debug!(
			"capability auto-activate: embedding model not ready yet, skipping this turn"
		);
		return;
	}

	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			crate::log_debug!("capability auto-activate: enumeration failed ({})", e);
			return;
		}
	};

	let inactive: Vec<&crate::agent::registry::ResolvedCapability> =
		caps.iter().filter(|c| !is_active(&c.name)).collect();
	if inactive.is_empty() {
		return;
	}

	let intent_vec = match crate::embeddings::embed(&intent).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("capability auto-activate: intent embed failed ({})", e);
			return;
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
		return;
	}

	let trigger_vecs = match crate::embeddings::embed_many(&flat).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("capability auto-activate: trigger embed failed ({})", e);
			return;
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

	let top = select_with_margin(scored, AUTO_ACTIVATE_THRESHOLD, AUTO_ACTIVATE_MARGIN);

	if let Some((score, cap)) = top {
		match activate_capability_inline(&cap.name, config).await {
			Ok(servers) => {
				crate::log_info!(
					"capability auto-activated: '{}' (score {:.2}) — servers: [{}]",
					cap.name,
					score,
					servers.join(", ")
				);
			}
			Err(e) => {
				crate::log_debug!(
					"capability auto-activate: failed to enable '{}' ({})",
					cap.name,
					e
				);
			}
		}
	}
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
/// or surface. Idempotent: returns `Ok(empty)` when already active.
async fn activate_capability_inline(name: &str, config: &Config) -> Result<Vec<String>> {
	if is_active(name) {
		return Ok(Vec::new());
	}
	let resolved = crate::agent::registry::parse_capability_toml(name, &config.capabilities)?;
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
		mark_active(name, Vec::new());
		return Ok(Vec::new());
	}
	// Make room before activating — drops the LRU active capability if
	// we'd exceed `MAX_ACTIVE_CAPS`. No-op when below the cap.
	evict_lru_if_full(config);

	let mut activated_servers: Vec<String> = Vec::new();
	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();
		if !crate::mcp::core::dynamic::is_dynamic(&server_name) {
			crate::mcp::core::dynamic::register_server(server.clone())?;
		}
		// Per-server bare-name filter (strips `<server>:` namespace from
		// capability `allowed_tools` patterns; see `filter_for_server`).
		let filter = filter_for_server(&resolved.allowed_tools, &server_name);
		crate::mcp::core::dynamic::enable_server(&server_name, filter).await?;
		activated_servers.push(server_name);
	}
	mark_active(name, activated_servers.clone());
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
mod tests {
	use super::*;
	use crate::agent::registry::ResolvedCapability;

	fn make_cap_with_triggers(name: &str, triggers: &[&str]) -> ResolvedCapability {
		ResolvedCapability {
			name: name.to_string(),
			triggers: triggers.iter().map(|s| s.to_string()).collect(),
			deps: Vec::new(),
			server_refs: Vec::new(),
			allowed_tools: Vec::new(),
			mcp_servers: Vec::new(),
			tap_root: std::path::PathBuf::new(),
		}
	}

	#[test]
	fn schema_has_required_action() {
		let f = get_capability_function();
		assert_eq!(f.name, "capability");
		let required = f
			.parameters
			.get("required")
			.and_then(|v| v.as_array())
			.expect("required array");
		assert!(required.iter().any(|v| v.as_str() == Some("action")));
	}

	#[test]
	fn active_registry_marks_and_clears() {
		let cap = "test.cap.alpha";
		assert!(!is_active(cap));
		mark_active(cap, vec!["test-server".to_string()]);
		assert!(is_active(cap));
		mark_inactive(cap);
		assert!(!is_active(cap));
	}

	// --------------------------------------------------------------------
	// filter_for_server — translates capability `allowed_tools` patterns
	// (namespaced) into the bare-name patterns enable_server expects.
	// --------------------------------------------------------------------

	#[test]
	fn filter_for_server_empty_input_returns_none() {
		assert!(filter_for_server(&[], "playwright").is_none());
	}

	#[test]
	fn filter_for_server_strips_matching_namespace_prefix() {
		// `playwright:*` → `*` for the playwright server.
		let patterns = vec!["playwright:*".to_string()];
		let f = filter_for_server(&patterns, "playwright").expect("should produce a filter");
		assert_eq!(f, vec!["*".to_string()]);
	}

	#[test]
	fn filter_for_server_strips_specific_tool_namespace() {
		let patterns = vec![
			"playwright:browser_navigate".to_string(),
			"playwright:browser_click".to_string(),
		];
		let f = filter_for_server(&patterns, "playwright").expect("should produce a filter");
		assert_eq!(
			f,
			vec!["browser_navigate".to_string(), "browser_click".to_string()]
		);
	}

	#[test]
	fn filter_for_server_drops_other_servers_namespaced_patterns() {
		// Patterns scoped to `octoweb:*` shouldn't apply when enabling
		// `playwright`. With nothing scoped to playwright, no filter.
		let patterns = vec!["octoweb:*".to_string()];
		assert!(filter_for_server(&patterns, "playwright").is_none());
	}

	#[test]
	fn filter_for_server_keeps_unnamespaced_patterns_for_all_servers() {
		// A bare pattern (no `:`) applies to every server in the cap.
		let patterns = vec!["browser_*".to_string()];
		let f = filter_for_server(&patterns, "playwright").expect("bare pattern applies");
		assert_eq!(f, vec!["browser_*".to_string()]);
	}

	#[test]
	fn filter_for_server_mixed_patterns_only_keeps_relevant_ones() {
		// Mixed: own namespace + foreign namespace + bare. Result for
		// `playwright`: own (stripped) + bare; foreign dropped.
		let patterns = vec![
			"playwright:browser_navigate".to_string(),
			"octoweb:fetch".to_string(),
			"shared_tool".to_string(),
		];
		let f = filter_for_server(&patterns, "playwright").expect("filter applies");
		assert_eq!(
			f,
			vec!["browser_navigate".to_string(), "shared_tool".to_string()]
		);
	}

	#[test]
	fn select_lru_picks_oldest_timestamp() {
		use std::time::Duration;
		let now = Instant::now();
		let mut map: HashMap<String, CapState> = HashMap::new();
		map.insert(
			"alpha".to_string(),
			CapState {
				servers: vec!["s1".to_string()],
				last_used: now - Duration::from_secs(100),
			},
		);
		map.insert(
			"beta".to_string(),
			CapState {
				servers: vec!["s2".to_string()],
				last_used: now - Duration::from_secs(50),
			},
		);
		map.insert(
			"gamma".to_string(),
			CapState {
				servers: vec!["s3".to_string()],
				last_used: now,
			},
		);
		let evicted = select_lru_in(&mut map).expect("should evict the oldest");
		assert_eq!(evicted.0, "alpha");
		assert_eq!(evicted.1, vec!["s1".to_string()]);
		assert_eq!(map.len(), 2);
		assert!(!map.contains_key("alpha"));
	}

	#[test]
	fn select_lru_returns_none_for_empty_map() {
		let mut map: HashMap<String, CapState> = HashMap::new();
		assert!(select_lru_in(&mut map).is_none());
	}

	#[test]
	fn select_lru_handles_single_entry() {
		let mut map: HashMap<String, CapState> = HashMap::new();
		map.insert(
			"only".to_string(),
			CapState {
				servers: vec!["s1".to_string()],
				last_used: Instant::now(),
			},
		);
		let evicted = select_lru_in(&mut map).expect("should evict the only entry");
		assert_eq!(evicted.0, "only");
		assert!(map.is_empty());
	}

	#[test]
	fn touch_capability_updates_timestamp_for_owning_cap() {
		// Use unique cap name so we don't interfere with other tests.
		let cap = "test.touch.alpha";
		let server = "test.touch.server";
		mark_active(cap, vec![server.to_string()]);
		let before = registry().read().unwrap().get(cap).unwrap().last_used;
		std::thread::sleep(std::time::Duration::from_millis(2));
		touch_capability_for_server(server);
		let after = registry().read().unwrap().get(cap).unwrap().last_used;
		assert!(
			after > before,
			"touch_capability_for_server should bump last_used"
		);
		mark_inactive(cap);
	}

	// -----------------------------------------------------------------------
	// Pure-logic tests for the deterministic auto-activation gate. These
	// cover the (threshold, margin) decision boundary that controls whether
	// a capability is flipped on — independent of any embedding model.
	// -----------------------------------------------------------------------

	#[test]
	fn select_with_margin_returns_none_for_empty_input() {
		let empty: Vec<(f32, &str)> = Vec::new();
		assert!(select_with_margin(empty, 0.4, 0.05).is_none());
	}

	#[test]
	fn select_with_margin_returns_none_when_top_below_threshold() {
		let scored = vec![(0.30_f32, "a"), (0.10_f32, "b")];
		assert!(select_with_margin(scored, 0.4, 0.05).is_none());
	}

	#[test]
	fn select_with_margin_admits_score_at_threshold() {
		// Threshold is `>=` (inclusive). A score equal to the threshold
		// IS selected provided the margin gate is also satisfied.
		let scored = vec![(0.42_f32, "a"), (0.10_f32, "b")];
		let top = select_with_margin(scored, 0.42, 0.05).unwrap();
		assert_eq!(top.1, "a");
	}

	#[test]
	fn select_with_margin_rejects_when_top1_top2_too_close() {
		// Both entries clear the threshold but are within the margin —
		// ambiguous, so the gate abstains rather than picking one.
		let scored = vec![(0.50_f32, "a"), (0.48_f32, "b")];
		assert!(select_with_margin(scored, 0.4, 0.05).is_none());
	}

	#[test]
	fn select_with_margin_admits_when_margin_satisfied() {
		let scored = vec![(0.50_f32, "a"), (0.40_f32, "b")];
		let top = select_with_margin(scored, 0.4, 0.05).unwrap();
		assert_eq!(top.1, "a");
	}

	#[test]
	fn select_with_margin_handles_single_candidate() {
		// With only one candidate, top2 is treated as 0.0 — so the margin
		// gate reduces to "top1 >= max(threshold, margin)".
		let scored = vec![(0.45_f32, "only")];
		let top = select_with_margin(scored, 0.4, 0.05).unwrap();
		assert_eq!(top.1, "only");
	}

	#[test]
	fn select_with_margin_zero_margin_returns_first_on_tie() {
		// With margin=0.0, exact ties pass the gate; the stable sort keeps
		// the first occurrence.
		let scored = vec![(0.70_f32, "first"), (0.70_f32, "second")];
		let top = select_with_margin(scored, 0.4, 0.0).unwrap();
		assert_eq!(top.1, "first");
	}

	#[test]
	fn select_with_margin_picks_top_when_scores_well_separated() {
		let scored = vec![
			(0.30_f32, "low"),
			(0.62_f32, "mid"),
			(0.81_f32, "high"),
			(0.40_f32, "below"),
		];
		let top = select_with_margin(scored, 0.55, 0.05).unwrap();
		assert_eq!(top.1, "high");
		assert!((top.0 - 0.81).abs() < 1e-6);
	}

	#[test]
	fn score_capability_empty_triggers_returns_zero() {
		let intent = vec![1.0_f32, 0.0, 0.0];
		let empty: Vec<Vec<f32>> = Vec::new();
		assert_eq!(score_capability(&intent, &empty), 0.0);
	}

	#[test]
	fn score_capability_takes_mean_of_top_k() {
		// Trigger vectors aligned with intent at varying degrees so the
		// computed cosines are 1.0, 0.5, 0.0, 0.0 — top-3 mean is 0.5.
		let intent = vec![1.0_f32, 0.0];
		let triggers = vec![
			vec![1.0_f32, 0.0],   // cos = 1.0
			vec![0.5_f32, 0.866], // cos ≈ 0.5
			vec![0.0_f32, 1.0],   // cos = 0.0
			vec![0.0_f32, 1.0],   // cos = 0.0 — excluded by top-3
		];
		let score = score_capability(&intent, &triggers);
		// Mean of (1.0, 0.5, 0.0) = 0.5. Allow small float slack.
		assert!((score - 0.5).abs() < 0.01, "expected ~0.5 got {score}");
	}

	/// End-to-end smoke test: with the real BGE-small model loaded, a
	/// natural-language intent should pick the semantically closest
	/// synthetic capability over plausible distractors when ranked by
	/// the same `score_capability` + `select_with_margin` pipeline used
	/// by `auto_activate_capabilities`.
	///
	/// Uses synthetic capabilities with hand-authored triggers so the
	/// test doesn't depend on any real tap being installed. The model
	/// itself is downloaded on first run to fastembed's cache (~30MB)
	/// and reused thereafter.
	#[tokio::test]
	async fn auto_activate_picks_semantically_closest_capability() {
		let postgres = make_cap_with_triggers(
			"database.postgres",
			&[
				"query a postgres database",
				"EXPLAIN ANALYZE a slow postgres query",
				"look at the postgres schema",
				"investigate a Postgres query plan",
			],
		);
		let web_search = make_cap_with_triggers(
			"web.search",
			&[
				"search the web for an article",
				"find recent news online",
				"look something up on the internet",
			],
		);
		let filesystem = make_cap_with_triggers(
			"filesystem.local",
			&[
				"read a file from disk",
				"list the contents of a directory",
				"write to a local file",
			],
		);
		let candidates = vec![postgres.clone(), web_search.clone(), filesystem.clone()];

		let intent = "I need to look at a slow Postgres query plan";
		let intent_vec = crate::embeddings::embed(intent)
			.await
			.expect("embed intent should succeed");

		let mut flat: Vec<String> = Vec::new();
		let mut offsets: Vec<(usize, usize)> = Vec::new();
		for cap in &candidates {
			let start = flat.len();
			flat.extend(cap.triggers.iter().cloned());
			offsets.push((start, flat.len()));
		}
		let trigger_vecs = crate::embeddings::embed_many(&flat)
			.await
			.expect("embed_many should succeed");

		let scored: Vec<(f32, &ResolvedCapability)> = candidates
			.iter()
			.zip(offsets.iter())
			.map(|(cap, (start, end))| {
				let score = score_capability(&intent_vec, &trigger_vecs[*start..*end]);
				(score, cap)
			})
			.collect();

		// Use threshold 0.0 / margin 0.0 so the test checks *ranking*, not
		// absolute cosine values which depend on the specific model.
		let top = select_with_margin(scored, 0.0, 0.0)
			.expect("at least one capability should outscore the rest for a clear intent");
		assert_eq!(
			top.1.name, "database.postgres",
			"expected database.postgres to win for a postgres intent (got {} score {:.3})",
			top.1.name, top.0
		);
	}

	/// Fixture-based regression test for the deterministic auto-activation
	/// gate. Each fixture is a `(user_message, expected_capability_or_None)`
	/// pair authored by hand. We run the *production* gate (same scoring
	/// pipeline + `AUTO_ACTIVATE_THRESHOLD` + `AUTO_ACTIVATE_MARGIN`) and
	/// assert ≥85% top-1 accuracy on positive cases plus ≥80% abstain rate
	/// on negative cases.
	///
	/// Substitute for a labeled corpus we don't have. Catches threshold/
	/// margin drift, BGE-small ranking regressions, and trigger-set quality
	/// across 12 representative capabilities. Triggers are copied verbatim
	/// from `../octomind-tap/capabilities/<cap>/config.toml`; if those
	/// change, update both places (intentional duplication — the test is a
	/// regression net for the data we ship).
	#[tokio::test]
	async fn capability_routing_fixtures_match_expected_caps() {
		let caps = vec![
			make_cap_with_triggers(
				"database-postgres",
				&[
					"query a postgres database",
					"EXPLAIN ANALYZE a slow postgres query",
					"look at the postgres schema",
					"investigate a Postgres query plan",
					"check rows in a postgres table",
					"run SQL against postgres",
				],
			),
			make_cap_with_triggers(
				"database-sqlite",
				&[
					"query a sqlite database",
					"inspect a SQLite file",
					"run SQL against a sqlite db",
					"look at the schema of a sqlite database",
					"open a .db file and read tables",
				],
			),
			make_cap_with_triggers(
				"filesystem",
				&[
					"read a local file",
					"edit a file on disk",
					"list directory contents",
					"search files for a pattern",
					"execute a shell command",
					"find files by name",
				],
			),
			make_cap_with_triggers(
				"codesearch",
				&[
					"find where this function is used",
					"search the codebase for an implementation",
					"look up symbol definitions",
					"find code matching a pattern",
					"semantic search across the repo",
					"view function signatures in this file",
				],
			),
			make_cap_with_triggers(
				"websearch",
				&[
					"search the web for information",
					"find recent news online",
					"google something",
					"look up an article on the web",
					"find a tutorial online",
				],
			),
			make_cap_with_triggers(
				"webfetch",
				&[
					"fetch a URL's content",
					"download a webpage",
					"get the contents of a web page",
					"retrieve a web resource",
				],
			),
			make_cap_with_triggers(
				"kubernetes",
				&[
					"list pods in a kubernetes cluster",
					"check kubectl logs",
					"describe a kubernetes deployment",
					"look at a helm chart",
					"troubleshoot a failing pod",
					"scale a kubernetes deployment",
				],
			),
			make_cap_with_triggers(
				"docker",
				&[
					"list running docker containers",
					"build a docker image",
					"inspect a container's logs",
					"run a docker compose service",
					"stop a docker container",
					"check docker container status",
				],
			),
			make_cap_with_triggers(
				"messaging-slack",
				&[
					"send a slack message",
					"post to a slack channel",
					"search slack history",
					"look up a slack thread",
					"list slack channels",
				],
			),
			make_cap_with_triggers(
				"messaging-discord",
				&[
					"send a message to a discord channel",
					"post to discord",
					"list discord servers",
					"read recent discord messages",
				],
			),
			make_cap_with_triggers(
				"versioning",
				&[
					"check git status",
					"look at the version history",
					"view git log",
					"see what changed between commits",
					"track changes in version control",
				],
			),
			make_cap_with_triggers(
				"payments",
				&[
					"look up a stripe payment",
					"check payment status",
					"refund a stripe charge",
					"manage stripe customers",
					"create a stripe invoice",
				],
			),
		];

		// Embed all triggers once.
		let mut flat: Vec<String> = Vec::new();
		let mut offsets: Vec<(usize, usize)> = Vec::with_capacity(caps.len());
		for cap in &caps {
			let start = flat.len();
			flat.extend(cap.triggers.iter().cloned());
			offsets.push((start, flat.len()));
		}
		let trigger_vecs = crate::embeddings::embed_many(&flat)
			.await
			.expect("embed all triggers should succeed");

		// Positive fixtures: clear intent → expected capability.
		let positives: &[(&str, &str)] = &[
			(
				"EXPLAIN ANALYZE this slow postgres query",
				"database-postgres",
			),
			(
				"look at the postgres users table schema",
				"database-postgres",
			),
			(
				"I have a sqlite database I need to query",
				"database-sqlite",
			),
			("open a .db file and check the tables", "database-sqlite"),
			("read the contents of this file", "filesystem"),
			("list everything in the current directory", "filesystem"),
			("find where this function is defined", "codesearch"),
			("search the codebase for the user model", "codesearch"),
			("search the web for recent AI news", "websearch"),
			("google how to do X", "websearch"),
			("fetch the contents of this URL", "webfetch"),
			("download this webpage", "webfetch"),
			("list the pods in my k8s cluster", "kubernetes"),
			("describe this kubernetes deployment", "kubernetes"),
			("show me running docker containers", "docker"),
			("build a docker image", "docker"),
			("send a slack message to the team", "messaging-slack"),
			(
				"post in a slack channel about the deploy",
				"messaging-slack",
			),
			("send a discord message", "messaging-discord"),
			("post to discord", "messaging-discord"),
			("show me git log", "versioning"),
			("what changed in the last commit", "versioning"),
			("look up a stripe payment", "payments"),
			("refund this customer's stripe charge", "payments"),
		];

		// Negative fixtures: chitchat / generic with no clear capability fit.
		// The gate should abstain (return None) for these.
		let negatives: &[&str] = &[
			"hello how are you doing today",
			"what's the weather like",
			"explain the concept of recursion",
			"good morning, can you help me?",
			"thanks, that was helpful",
		];

		let mut positive_correct = 0usize;
		let mut positive_misses: Vec<String> = Vec::new();
		for (intent, expected) in positives {
			let intent_vec = crate::embeddings::embed(intent)
				.await
				.expect("embed intent should succeed");
			let scored: Vec<(f32, &ResolvedCapability)> = caps
				.iter()
				.zip(offsets.iter())
				.map(|(cap, (start, end))| {
					let s = score_capability(&intent_vec, &trigger_vecs[*start..*end]);
					(s, cap)
				})
				.collect();
			let result = select_with_margin(scored, AUTO_ACTIVATE_THRESHOLD, AUTO_ACTIVATE_MARGIN);
			match &result {
				Some((_, c)) if c.name == *expected => positive_correct += 1,
				other => positive_misses.push(format!(
					"{intent:?} → expected {expected}, got {:?}",
					other
						.as_ref()
						.map(|(s, c)| format!("{} (score {:.2})", c.name, s))
				)),
			}
		}

		let mut negative_abstained = 0usize;
		let mut negative_misses: Vec<String> = Vec::new();
		for intent in negatives {
			let intent_vec = crate::embeddings::embed(intent)
				.await
				.expect("embed intent should succeed");
			let scored: Vec<(f32, &ResolvedCapability)> = caps
				.iter()
				.zip(offsets.iter())
				.map(|(cap, (start, end))| {
					let s = score_capability(&intent_vec, &trigger_vecs[*start..*end]);
					(s, cap)
				})
				.collect();
			let result = select_with_margin(scored, AUTO_ACTIVATE_THRESHOLD, AUTO_ACTIVATE_MARGIN);
			match &result {
				None => negative_abstained += 1,
				Some((s, c)) => negative_misses.push(format!(
					"{intent:?} → expected None, got {} (score {:.2})",
					c.name, s
				)),
			}
		}

		let pos_total = positives.len();
		let neg_total = negatives.len();
		let pos_acc = positive_correct as f32 / pos_total as f32;
		let neg_acc = negative_abstained as f32 / neg_total as f32;

		assert!(
			pos_acc >= 0.85,
			"Positive top-1 accuracy {pos_acc:.2} below 0.85 threshold ({}/{} correct).\nMisses:\n{}",
			positive_correct,
			pos_total,
			positive_misses.join("\n")
		);
		assert!(
			neg_acc >= 0.80,
			"Negative abstain rate {neg_acc:.2} below 0.80 threshold ({}/{} abstained).\nMisses:\n{}",
			negative_abstained,
			neg_total,
			negative_misses.join("\n")
		);
	}
}
