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
//! those are merged into the effective config at boot. This tool exposes
//! the *runtime* lever: agents can list, discover, enable, and disable
//! capabilities on demand without knowing the underlying MCP server names.
//!
//! Actions:
//! - `list`     — show all installed capabilities (active marked).
//! - `enable`   — activate a capability by name (registers + enables its MCP servers).
//! - `disable`  — deactivate a previously-enabled capability.
//! - `discover` — keyword-match an intent string against capability names + descriptions.

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
		description: r#"Discover and activate capabilities mid-session. Capabilities are domain abstractions (e.g., "database.postgres", "web.search") that resolve to MCP servers and tools. Use this when the agent needs functionality outside its starting kit.

Actions:
- `list`     — show all installed capabilities. Active ones are marked. Returns one line per capability: name + brief description.
- `enable`   — activate a capability by name. Registers and enables its MCP servers, exposing the capability's tools in subsequent turns.
- `disable`  — deactivate a previously-enabled capability.
- `discover` — find capabilities matching an intent string (substring match against name and description).

Workflow: call `list` or `discover` to find the right capability, then `enable` to activate it. The agent's tool surface grows on demand; nothing loaded that wasn't asked for."#.to_string(),
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
		output.push_str(&format!("- {}{} — {}\n", marker, cap.name, cap.description));
	}
	output.push_str("\nUse capability(action=\"enable\", name=\"<name>\") to activate.");
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
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
// Auto-discovery — embed each fresh user message, suggest top-matching
// non-active capability when cosine clears the threshold.
// ---------------------------------------------------------------------------

/// Marker used to recognize and skip our own hint messages so we don't
/// suggest on a message that is itself a suggestion.
const AUTO_HINT_PREFIX: &str = "💡 Capability suggestion:";

/// Cosine threshold below which a capability is too weakly matched to
/// surface as a suggestion. Tuned for `BAAI/bge-small-en-v1.5` — values
/// in the 0.4–0.6 range are typical for relevant matches; >0.7 is strong.
const AUTO_SUGGEST_THRESHOLD: f32 = 0.55;

/// Pick the highest-scoring entry strictly above `threshold`. Returns
/// `None` if no entry clears the bar, or the input is empty. On exact
/// ties, the first occurrence wins (stable, deterministic).
///
/// Pure helper — separated from the embedding-driven path so the
/// threshold/top-pick logic can be unit-tested without the model.
fn select_top_match_above_threshold<'a, I, T>(scored: I, threshold: f32) -> Option<(f32, T)>
where
	I: IntoIterator<Item = (f32, T)>,
	T: 'a,
{
	let mut best: Option<(f32, T)> = None;
	for (score, item) in scored {
		if score <= threshold {
			continue;
		}
		match &best {
			None => best = Some((score, item)),
			Some((current, _)) if score > *current => best = Some((score, item)),
			_ => {}
		}
	}
	best
}

/// Inspect the most recent user message and inject a one-line hint when
/// a non-active capability is a strong embedding match. Silent no-op when
/// the model isn't ready yet, no capabilities are installed, no match
/// clears the threshold, or the last message is not a user message.
///
/// Designed to be called from `prepare_for_api_call` so it runs before
/// every API request; the `AUTO_HINT_PREFIX` check ensures the same user
/// message doesn't accumulate multiple hints across follow-up turns.
pub async fn auto_suggest_capabilities(
	session: &mut crate::session::chat::session::ChatSession,
	config: &Config,
) {
	// Only fire when the most recent message is a fresh user input.
	let intent = match session.session.messages.last() {
		Some(m) if m.role == "user" && !m.content.contains(AUTO_HINT_PREFIX) => m.content.clone(),
		_ => return,
	};

	// If the embedding model hasn't finished warmup, skip silently.
	// The caller (initialize_servers_for_role) already kicked off warmup;
	// we don't want to block the hot API path on a 50MB download.
	if !crate::embeddings::is_ready() {
		crate::log_debug!(
			"capability auto-suggest: embedding model not ready yet, skipping this turn"
		);
		return;
	}

	let caps = match crate::agent::registry::list_all_capabilities(&config.capabilities) {
		Ok(c) => c,
		Err(e) => {
			crate::log_debug!("capability auto-suggest: enumeration failed ({})", e);
			return;
		}
	};

	// Only consider capabilities that are not already active in this session.
	let inactive: Vec<&crate::agent::registry::ResolvedCapability> =
		caps.iter().filter(|c| !is_active(&c.name)).collect();
	if inactive.is_empty() {
		return;
	}

	let intent_vec = match crate::embeddings::embed(&intent).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("capability auto-suggest: intent embed failed ({})", e);
			return;
		}
	};
	let cap_texts: Vec<String> = inactive
		.iter()
		.map(|c| format!("{} {}", c.name, c.description))
		.collect();
	let cap_vecs = match crate::embeddings::embed_many(&cap_texts).await {
		Ok(v) => v,
		Err(e) => {
			crate::log_debug!("capability auto-suggest: capability embed failed ({})", e);
			return;
		}
	};

	let scored = inactive
		.iter()
		.zip(cap_vecs.iter())
		.map(|(cap, vec)| (crate::embeddings::cosine(&intent_vec, vec), *cap));
	let top = select_top_match_above_threshold(scored, AUTO_SUGGEST_THRESHOLD);

	if let Some((score, cap)) = top {
		let hint = format!(
			"{prefix} `{name}` ({desc}) looks relevant to this request (score {score:.2}). Call `capability(action=\"enable\", name=\"{name}\")` to activate it.",
			prefix = AUTO_HINT_PREFIX,
			name = cap.name,
			desc = cap.description,
			score = score,
		);
		session.session.messages.push(crate::session::Message {
			role: "user".to_string(),
			content: hint,
			..Default::default()
		});
		crate::log_debug!(
			"capability auto-suggest: injected hint for `{}` (score {:.2})",
			cap.name,
			score
		);
	}
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

	// Embedding-based scoring is preferred; on any failure (model not yet
	// downloaded, no network, runtime init error) fall back to keyword
	// scoring so discover keeps working in restricted environments.
	let scored = match score_with_embeddings(&intent, &caps).await {
		Ok(s) => s,
		Err(e) => {
			crate::log_debug!(
				"capability discover: embedding scoring unavailable ({}); using keyword fallback",
				e
			);
			score_with_keywords(&intent, &caps)
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
			marker, cap.name, score, cap.description
		));
	}
	output.push_str("\nUse capability(action=\"enable\", name=\"<name>\") to activate.");
	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
}

/// Embedding-based scoring: cosine similarity between the intent and each
/// capability's `name + description`. Filters out scores below 0.2 to avoid
/// noise. Sorted by similarity descending. Errors propagate so the caller
/// can fall back to keyword scoring on model-init failure.
async fn score_with_embeddings<'a>(
	intent: &str,
	caps: &'a [crate::agent::registry::ResolvedCapability],
) -> Result<Vec<(f32, &'a crate::agent::registry::ResolvedCapability)>> {
	let intent_vec = crate::embeddings::embed(intent).await?;
	let cap_texts: Vec<String> = caps
		.iter()
		.map(|c| format!("{} {}", c.name, c.description))
		.collect();
	let cap_vecs = crate::embeddings::embed_many(&cap_texts).await?;

	let mut scored: Vec<(f32, &crate::agent::registry::ResolvedCapability)> = caps
		.iter()
		.zip(cap_vecs.iter())
		.map(|(cap, vec)| (crate::embeddings::cosine(&intent_vec, vec), cap))
		.filter(|(score, _)| *score > 0.2)
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	Ok(scored)
}

/// Keyword scoring fallback: count whitespace-separated tokens from `intent`
/// that occur as substrings of the lowercased `name + description`.
fn score_with_keywords<'a>(
	intent: &str,
	caps: &'a [crate::agent::registry::ResolvedCapability],
) -> Vec<(f32, &'a crate::agent::registry::ResolvedCapability)> {
	let intent_lower = intent.to_lowercase();
	let intent_words: Vec<&str> = intent_lower.split_whitespace().collect();
	let mut scored: Vec<(f32, &crate::agent::registry::ResolvedCapability)> = caps
		.iter()
		.map(|cap| {
			let haystack = format!("{} {}", cap.name, cap.description).to_lowercase();
			let count = intent_words
				.iter()
				.filter(|word| haystack.contains(*word))
				.count();
			(count as f32, cap)
		})
		.filter(|(score, _)| *score > 0.0)
		.collect();
	scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
	scored
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use crate::agent::registry::ResolvedCapability;

	fn make_cap(name: &str, description: &str) -> ResolvedCapability {
		ResolvedCapability {
			name: name.to_string(),
			description: description.to_string(),
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

	#[test]
	fn keyword_scoring_ranks_relevant_first() {
		let caps = vec![
			make_cap("database.postgres", "Query and inspect Postgres databases"),
			make_cap("web.search", "Search the web for information"),
			make_cap("filesystem.local", "Read and write local files"),
		];
		let scored = score_with_keywords("I need to query a database", &caps);
		assert!(!scored.is_empty(), "expected at least one match");
		assert_eq!(scored[0].1.name, "database.postgres");
	}

	#[test]
	fn keyword_scoring_filters_zero_matches() {
		let caps = vec![
			make_cap("web.search", "Search the web for information"),
			make_cap("filesystem.local", "Read and write local files"),
		];
		let scored = score_with_keywords("zzz nothing matches xyzzy", &caps);
		assert!(
			scored.is_empty(),
			"expected no matches for nonsense intent, got {scored:?}"
		);
	}

	// -----------------------------------------------------------------------
	// Pure-logic tests for the auto-suggest threshold + top-pick selection.
	// These cover the decision boundary that determines whether a hint is
	// injected into the session — independent of the embedding model.
	// -----------------------------------------------------------------------

	#[test]
	fn select_top_returns_none_for_empty_input() {
		let empty: Vec<(f32, &str)> = Vec::new();
		assert!(select_top_match_above_threshold(empty, 0.5).is_none());
	}

	#[test]
	fn select_top_returns_none_when_all_below_threshold() {
		let scored = vec![(0.30_f32, "a"), (0.40_f32, "b"), (0.10_f32, "c")];
		assert!(select_top_match_above_threshold(scored, 0.5).is_none());
	}

	#[test]
	fn select_top_excludes_score_at_exact_threshold() {
		// Threshold is strictly greater-than (not >=) so a score equal to
		// the threshold should NOT be selected.
		let scored = vec![(0.55_f32, "a"), (0.40_f32, "b")];
		assert!(select_top_match_above_threshold(scored, 0.55).is_none());
	}

	#[test]
	fn select_top_picks_highest_above_threshold() {
		let scored = vec![
			(0.30_f32, "low"),
			(0.62_f32, "mid"),
			(0.81_f32, "high"),
			(0.40_f32, "below"),
		];
		let top = select_top_match_above_threshold(scored, 0.55).unwrap();
		assert_eq!(top.1, "high");
		assert!((top.0 - 0.81).abs() < 1e-6);
	}

	#[test]
	fn select_top_first_wins_on_exact_tie() {
		// Stable selection: when two entries share the top score, the
		// first one encountered in iteration order is kept.
		let scored = vec![(0.70_f32, "first"), (0.70_f32, "second")];
		let top = select_top_match_above_threshold(scored, 0.5).unwrap();
		assert_eq!(top.1, "first");
	}

	#[test]
	fn select_top_works_with_zero_threshold() {
		// Threshold 0.0 admits any positive score (still strictly greater).
		let scored = vec![(0.01_f32, "tiny"), (0.0_f32, "zero")];
		let top = select_top_match_above_threshold(scored, 0.0).unwrap();
		assert_eq!(top.1, "tiny");
	}
}
