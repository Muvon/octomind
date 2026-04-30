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
use anyhow::Result;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock, RwLock};

// ---------------------------------------------------------------------------
// Active capabilities registry (process-global; mirrors dynamic.rs pattern)
// ---------------------------------------------------------------------------

/// Capabilities activated at runtime by this tool. Capabilities pre-loaded from
/// the tap manifest at boot are NOT tracked here — they are already merged into
/// the agent's effective config and represented as regular MCP servers.
static ACTIVE_CAPABILITIES: OnceLock<Arc<RwLock<HashSet<String>>>> = OnceLock::new();

fn registry() -> &'static Arc<RwLock<HashSet<String>>> {
	ACTIVE_CAPABILITIES.get_or_init(|| Arc::new(RwLock::new(HashSet::new())))
}

fn is_active(name: &str) -> bool {
	registry().read().unwrap().contains(name)
}

fn mark_active(name: &str) {
	registry().write().unwrap().insert(name.to_string());
}

fn mark_inactive(name: &str) {
	registry().write().unwrap().remove(name);
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

	if resolved.mcp_servers.is_empty() {
		return Ok(McpToolResult::error(
			call.tool_name.clone(),
			call.tool_id.clone(),
			format!(
				"Capability '{name}' has no MCP servers configured (no [[mcp.servers]] blocks)."
			),
		));
	}

	let filter_tools: Option<Vec<String>> = if resolved.allowed_tools.is_empty() {
		None
	} else {
		Some(resolved.allowed_tools.clone())
	};

	let mut activated_tools: Vec<String> = Vec::new();
	let mut activated_servers: Vec<String> = Vec::new();

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

		match crate::mcp::core::dynamic::enable_server(&server_name, filter_tools.clone()).await {
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

	mark_active(&name);

	let msg = format!(
		"Capability '{name}' enabled. Activated {} server(s): {}\nTools available: {}",
		activated_servers.len(),
		activated_servers.join(", "),
		if activated_tools.is_empty() {
			"none".to_string()
		} else {
			activated_tools.join(", ")
		}
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

/// Register + enable a capability's MCP servers and mark the capability
/// active. Mirrors `handle_enable`'s logic minus the `McpToolResult`
/// wrapping — errors propagate as `anyhow::Error` for the caller to log
/// or surface. Idempotent: returns `Ok(empty)` when already active.
async fn activate_capability_inline(name: &str, config: &Config) -> Result<Vec<String>> {
	if is_active(name) {
		return Ok(Vec::new());
	}
	let resolved = crate::agent::registry::parse_capability_toml(name, &config.capabilities)?;
	if resolved.mcp_servers.is_empty() {
		anyhow::bail!("capability '{}' has no MCP servers configured", name);
	}
	let filter_tools: Option<Vec<String>> = if resolved.allowed_tools.is_empty() {
		None
	} else {
		Some(resolved.allowed_tools.clone())
	};
	let mut activated_servers: Vec<String> = Vec::new();
	for server in &resolved.mcp_servers {
		let server_name = server.name().to_string();
		if !crate::mcp::core::dynamic::is_dynamic(&server_name) {
			crate::mcp::core::dynamic::register_server(server.clone())?;
		}
		crate::mcp::core::dynamic::enable_server(&server_name, filter_tools.clone()).await?;
		activated_servers.push(server_name);
	}
	mark_active(name);
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
		mark_active(cap);
		assert!(is_active(cap));
		mark_inactive(cap);
		assert!(!is_active(cap));
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
}
