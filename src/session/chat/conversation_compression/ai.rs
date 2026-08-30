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

// LLM I/O for compression decision + summary generation.
//
// `prepare_decision` picks one of two equal paths up-front from
// the provider's `enforces_response_schema(model)` capability:
//
//   - JSON path (schema enforced): builds the JSON prompt + attaches the
//     strict JSON schema; deserialises `response.structured_output` (with a
//     small lenient recovery for providers that misroute valid JSON into
//     `content` instead of `structured_output`).
//   - XML path (no schema guarantee): builds the XML prompt (embedding the
//     tag spec); parses the response with `parse_xml_summary`, which performs
//     structural validation matching the JSON schema's bounds. Used for both
//     providers with no structured-output support AND providers that only
//     offer `json_object` mode (e.g. DeepSeek) where response shape is not
//     guaranteed and typed JSON deserialization would be fragile.
//
// Both paths return `CompressionSummary`; the substantive-content gate and
// cost/knowledge side-effects are mode-agnostic.

use super::prompt::{build_compression_prompt_json, build_compression_prompt_xml};
use super::schema::{
	build_compression_schema, is_summary_substantive, parse_xml_summary, CompressionSummary,
};
use crate::config::Config;
use crate::providers::ProviderFactory;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Invoke the compression model and return the parsed summary.
///
/// `schema` decides the wire-mode:
///   - `Some(schema)` → JSON path. Schema is attached to the request,
///     `response.structured_output` is preferred and falls back to a
///     lenient text-content recovery for providers that misroute it.
///   - `None` → XML path. No schema attached, the model's textual
///     response is fed through `parse_xml_summary`.
///
/// The decision call's spend is added to the session total. The system
/// message is marked cached with 1h TTL so it's amortised across every
/// compression call in a session.
pub(super) async fn run_decision_call(
	config: &Config,
	system_content: String,
	user_content: String,
	schema: Option<serde_json::Value>,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(CompressionSummary, Option<crate::providers::TokenUsage>)> {
	let now = crate::utils::time::now_secs();
	let decision_config = &config.compression.decision;

	// Cache the system prompt only if the compression model supports caching.
	// The system content is stable across compression calls (only varies on
	// `force` and mode), so cache hits amortise the system tokens.
	let supports_caching = crate::session::model_supports_caching(&decision_config.model);

	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: system_content,
			timestamp: now,
			cached: supports_caching,
			cache_ttl: if supports_caching {
				Some("1h".to_string())
			} else {
				None
			},
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		crate::session::Message {
			role: "user".to_string(),
			content: user_content,
			timestamp: now,
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	let mode_label = if schema.is_some() { "json" } else { "xml" };
	log_debug!(
		"Using compression decision model '{}' mode={} (max_tokens={}, temp={})",
		decision_config.model,
		mode_label,
		decision_config.max_tokens,
		decision_config.temperature
	);

	let mut params = crate::session::ChatCompletionWithValidationParams::new(
		&messages,
		&decision_config.model,
		decision_config.temperature,
		decision_config.top_p,
		decision_config.top_k,
		decision_config.max_tokens,
		config,
	)
	.with_max_retries(decision_config.max_retries)
	.with_full_context_tokens(true)
	.with_cancellation_token(operation_rx)
	.with_purpose(crate::providers::ModelPurpose::Compression)
	// Text-only summarization: no tools. Sending the MCP toolset here wastes
	// input tokens and makes proxy providers (octohub) skip schema
	// enforcement, breaking the JSON wire mode.
	.without_tools();

	if let Some(s) = schema.clone() {
		params = params.with_schema(s);
	}

	let response = crate::session::chat_completion_with_validation(params).await?;
	let usage = response.exchange.usage.clone();

	let summary = if schema.is_some() {
		parse_json_response(&response, &decision_config.model)?
	} else {
		parse_xml_summary(&response.content).map_err(|e| {
			anyhow::anyhow!(
				"Compression model '{}' (XML mode) produced an unparseable response: {}",
				decision_config.model,
				e
			)
		})?
	};
	Ok((summary, usage))
}

/// The prompt/schema bundle a decision call needs — built in the foreground
/// (it reads the live session), then runnable anywhere, including a spawned
/// background task that holds no session reference.
pub(super) struct PreparedDecision {
	pub system_content: String,
	pub user_content: String,
	pub schema: Option<serde_json::Value>,
}

pub(super) fn prepare_decision(
	session: &ChatSession,
	config: &Config,
	messages_to_compress: &[crate::session::Message],
	pact: Option<&super::attention::PactContext>,
	force: bool,
	target_ratio: f64,
) -> Result<PreparedDecision> {
	let model = &config.compression.decision.model;
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(model)?;
	let use_json = provider.enforces_response_schema(&actual_model);

	let (system_content, user_content) = if use_json {
		build_compression_prompt_json(session, messages_to_compress, pact, force, target_ratio)
	} else {
		build_compression_prompt_xml(session, messages_to_compress, pact, force, target_ratio)
	};
	let schema = use_json.then(|| build_compression_schema(force, pact.is_some()));
	log_debug!(
		"Compression wire mode: {} (provider='{}', model='{}')",
		if use_json { "json" } else { "xml" },
		provider.name(),
		actual_model
	);
	Ok(PreparedDecision {
		system_content,
		user_content,
		schema,
	})
}

/// Per-component spend for `/info` plus the session total — recorded even when
/// the decision ends up "don't compress" (the call happened).
pub(super) fn record_decision_usage(
	session: &mut ChatSession,
	usage: Option<&crate::providers::TokenUsage>,
) {
	let Some(usage) = usage else { return };
	let stats = &mut session.session.info.compression_stats;
	stats.input_tokens += usage.input_tokens;
	stats.output_tokens += usage.output_tokens;
	stats.reasoning_tokens += usage.reasoning_tokens;
	stats.cost += usage.cost.unwrap_or(0.0);
	stats.api_time_ms += usage.request_time_ms.unwrap_or(0);
	if let Some(cost) = usage.cost {
		session.session.info.total_cost += cost;
		session.estimated_cost = session.session.info.total_cost;
		log_debug!(
			"Compression decision cost: ${:.5} (total: ${:.5})",
			cost,
			session.session.info.total_cost
		);
	}
}

/// The veto and substantive-summary rules, shared by the inline and background
/// paths. Returns the effective should_compress.
pub(super) fn evaluate_decision(summary: &CompressionSummary, force: bool, has_pact: bool) -> bool {
	let mut should_compress = summary.should_compress;
	if !should_compress {
		if force {
			// Forced compression (ceiling breach or /done) grants the decision
			// model no veto — the schema/prompt demand should_compress=true, so a
			// false here is a protocol violation, not a decision. Override and let
			// the substantive-summary guard stay the real safety.
			log_info!(
				"Forced compression: decision model returned should_compress=false — overriding (refusal is not an option under force)"
			);
			should_compress = true;
		} else {
			log_debug!("AI compression decision: should_compress=false");
			return false;
		}
	}
	if !has_pact && !is_summary_substantive(summary) {
		log_info!(
			"Compression aborted: AI set should_compress=true but every narrative field is empty. Skipping compression to avoid context loss."
		);
		return false;
	}
	should_compress
}

/// JSON-path response parser. Prefers `response.structured_output`; falls
/// back to lenient extraction from `response.content` so providers that
/// misroute valid JSON into the text body (notably some OctoHub-routed
/// models) still succeed. The recovered value is then deserialized into
/// the typed `CompressionSummary`.
fn parse_json_response(
	response: &crate::providers::ProviderResponse,
	model: &str,
) -> Result<CompressionSummary> {
	let raw = match response.structured_output.clone() {
		Some(v) => v,
		None => {
			let recovered = extract_json_lenient(&response.content).ok_or_else(|| {
				anyhow::anyhow!(
					"Compression model '{}' returned no structured_output and no recoverable JSON in text content",
					model
				)
			})?;
			log_debug!(
				"Compression model '{}' omitted structured_output; recovered JSON from text content",
				model
			);
			recovered
		}
	};

	serde_json::from_value(raw).map_err(|e| {
		anyhow::anyhow!(
			"Failed to deserialize compression schema response: {}. The provider returned JSON that does not match the expected shape.",
			e
		)
	})
}

/// Best-effort JSON extraction from a text response when the provider didn't
/// populate `structured_output`. Handles three common provider patterns:
///
///   1. Bare JSON: `{"…": …}`
///   2. Fenced JSON: <code>```json\n{…}\n```</code> or unlabeled fences
///   3. Prose preamble: `"Here is the analysis: {…}"`
///
/// Returns `None` if no parseable JSON object/array can be located.
pub(crate) fn extract_json_lenient(content: &str) -> Option<serde_json::Value> {
	let trimmed = content.trim();
	if trimmed.is_empty() {
		return None;
	}

	// Direct parse — bare JSON or JSON-with-only-whitespace-padding.
	if matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'[')) {
		if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
			return Some(v);
		}
	}

	// Strip a single surrounding markdown fence (```json … ``` or ``` … ```)
	// and retry direct parse on the inner body.
	if let Some(inner) = strip_markdown_fence(trimmed) {
		if let Ok(v) = serde_json::from_str::<serde_json::Value>(inner.trim()) {
			return Some(v);
		}
	}

	// Last resort: scan for the first balanced JSON object or array anywhere
	// in the text, respecting string literals so brackets inside strings
	// don't fool the counter.
	find_first_balanced_json(trimmed)
}

/// Strip an outer markdown code fence if the content is wrapped in one.
/// Accepts ` ```json … ``` `, ` ```JSON … ``` `, or bare ` ``` … ``` `.
/// Returns the inner body without the fence markers, or `None` if no fence
/// envelope is present.
fn strip_markdown_fence(s: &str) -> Option<&str> {
	let s = s.trim();
	let after_open = s.strip_prefix("```")?;
	// Optional language tag on the opening fence — accept any letters then \n.
	let body = match after_open.find('\n') {
		Some(nl) => &after_open[nl + 1..],
		None => after_open,
	};
	body.strip_suffix("```").map(str::trim)
}

/// Scan `s` for the first balanced JSON object (`{…}`) or array (`[…]`).
/// Tracks bracket depth while skipping over string literals (with `\"` escape
/// handling) so punctuation inside strings doesn't unbalance the counter.
fn find_first_balanced_json(s: &str) -> Option<serde_json::Value> {
	let bytes = s.as_bytes();
	for start in 0..bytes.len() {
		let open = bytes[start];
		if open != b'{' && open != b'[' {
			continue;
		}
		let close = if open == b'{' { b'}' } else { b']' };
		let mut depth: i32 = 0;
		let mut in_string = false;
		let mut escape = false;
		for (i, &b) in bytes.iter().enumerate().skip(start) {
			if in_string {
				if escape {
					escape = false;
				} else if b == b'\\' {
					escape = true;
				} else if b == b'"' {
					in_string = false;
				}
				continue;
			}
			if b == b'"' {
				in_string = true;
			} else if b == open {
				depth += 1;
			} else if b == close {
				depth -= 1;
				if depth == 0 {
					let candidate = &s[start..=i];
					if let Ok(v) = serde_json::from_str::<serde_json::Value>(candidate) {
						return Some(v);
					}
					// Balanced but invalid — abandon this opener, outer loop continues.
					break;
				}
			}
		}
	}
	None
}

/// Orchestration entrypoint: pick the wire mode from the provider's
/// `enforces_response_schema(model)` capability, build the matching
/// prompt, invoke the model, apply the substantive-content gate.
///
/// Returns `(should_compress, summary)`:
/// - `should_compress = false` → caller skips compression entirely; the
///   returned `summary` is meaningless and must not be applied.
/// - `should_compress = true` → caller proceeds with `apply_compression`
///   using the returned typed summary.
///
/// Substantive-content gate: if the model emits `should_compress: true` but
/// every narrative field is empty, we refuse to compress. Better to skip
/// than to wipe the session with a header-only summary.
#[cfg(test)]
#[path = "ai_extract_json_lenient_tests.rs"]
mod extract_json_lenient_tests;

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
