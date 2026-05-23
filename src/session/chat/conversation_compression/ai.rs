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
// `ask_ai_decision_and_summary` builds the prompt, invokes the model with a
// strict JSON schema attached, deserialises the typed response, and applies
// the substantive-content gate. Cost accounting and critical-knowledge
// folding remain side-effects on the session.

use super::prompt::build_compression_prompt;
use super::schema::{build_compression_schema, is_summary_substantive, CompressionSummary};
use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Invoke the compression model with the JSON schema attached, return the
/// parsed structured response.
///
/// Cost tracking applies unless `decision.ignore_cost` is set.
///
/// The system message is marked cached with 1h TTL so it's amortised across
/// every compression call in a session — the schema + behaviour rules are
/// byte-identical between calls and benefit from prompt caching.
pub(super) async fn call_ai_for_decision(
	session: &mut ChatSession,
	config: &Config,
	system_content: String,
	user_content: String,
	schema: serde_json::Value,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<CompressionSummary> {
	let now = crate::utils::time::now_secs();
	let decision_config = &config.compression.decision;

	// Cache the system prompt only if the compression model supports caching.
	// The system content is stable across compression calls (only varies on
	// `force`), so cache hits amortise the (still small) system tokens.
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

	crate::log_debug!(
		"Using compression decision model '{}' (max_tokens={}, temp={}, ignore_cost={})",
		decision_config.model,
		decision_config.max_tokens,
		decision_config.temperature,
		decision_config.ignore_cost
	);

	let params = crate::session::ChatCompletionWithValidationParams::new(
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
	.with_schema(schema)
	.with_cancellation_token(operation_rx);

	let response = crate::session::chat_completion_with_validation(params).await?;

	if !decision_config.ignore_cost {
		if let Some(cost) = response.exchange.usage.as_ref().and_then(|u| u.cost) {
			session.session.info.total_cost += cost;
			session.estimated_cost = session.session.info.total_cost;
			log_debug!(
				"Compression decision cost: ${:.5} (total: ${:.5})",
				cost,
				session.session.info.total_cost
			);
		}
	} else {
		log_debug!("Compression decision cost ignored (ignore_cost=true)");
	}

	// Provider returns the validated JSON in `structured_output`. If absent
	// the provider didn't honour the schema — treat as a hard error rather
	// than silently falling back to text parsing. `completion.rs:233`
	// already pre-validates that the model supports structured output, so
	// reaching this branch means a runtime provider misbehaviour.
	let raw = response.structured_output.ok_or_else(|| {
		anyhow::anyhow!(
			"Compression model '{}' returned no structured_output despite schema being attached",
			decision_config.model
		)
	})?;

	let summary: CompressionSummary = serde_json::from_value(raw).map_err(|e| {
		anyhow::anyhow!(
			"Failed to deserialize compression schema response: {}. The provider returned JSON that does not match the expected shape.",
			e
		)
	})?;

	Ok(summary)
}

/// Orchestration entrypoint: build prompt + schema, invoke model, apply
/// substantive-content gate, return whether to compress and the typed summary.
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
pub(super) async fn ask_ai_decision_and_summary(
	session: &mut ChatSession,
	config: &Config,
	messages_to_compress: &[crate::session::Message],
	operation_rx: tokio::sync::watch::Receiver<bool>,
	force: bool,
	target_ratio: f64,
) -> Result<(bool, CompressionSummary)> {
	let (system_content, user_content) =
		build_compression_prompt(session, messages_to_compress, force, target_ratio);
	let schema = build_compression_schema(force);

	let summary = call_ai_for_decision(
		session,
		config,
		system_content,
		user_content,
		schema,
		operation_rx,
	)
	.await?;

	if !summary.should_compress {
		log_debug!("AI compression decision: should_compress=false");
		return Ok((false, summary));
	}

	if !is_summary_substantive(&summary) {
		log_info!(
			"Compression aborted: AI set should_compress=true but every narrative field is empty. Skipping compression to avoid context loss."
		);
		return Ok((false, summary));
	}

	log_debug!(
		"AI compression decision: should_compress=true (findings={}, recent={}, knowledge={}, files={})",
		summary.analysis_findings.len(),
		summary.recent_exchanges.len(),
		summary.critical_knowledge.len(),
		summary.file_context.len()
	);

	Ok((true, summary))
}
