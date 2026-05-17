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
// This submodule owns the request → response → parse cycle:
// `ask_ai_decision_and_summary` is the orchestration entrypoint that builds
// the prompt (via the `prompt` submodule), invokes the model
// (`call_ai_for_decision`), parses the body (`parse_ai_response`), and
// validates it (`is_summary_valid`). Side-effects on the session: cost
// tracking and `<knowledge>` extraction routed through the `knowledge`
// submodule.

use super::knowledge::{extract_and_store_knowledge, strip_knowledge_tags};
use super::prompt::build_compression_prompt;
use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::{log_debug, log_info};
use anyhow::Result;

/// Call the AI compression model and return the raw response content.
///
/// Tracks cost against the session unless `ignore_cost` is set in config.
pub(super) async fn call_ai_for_decision(
	session: &mut ChatSession,
	config: &Config,
	system_content: String,
	user_content: String,
	operation_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<String> {
	let now = crate::utils::time::now_secs();
	let messages = vec![
		crate::session::Message {
			role: "system".to_string(),
			content: system_content,
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

	let decision_config = &config.compression.decision;

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

	Ok(response.content)
}

/// Minimum acceptable length (after trim + knowledge-tag strip) for a compression summary.
///
/// A 200-OK response from the AI is no guarantee the body is usable. The model can
/// return:
/// - bare `"YES"` with no summary line,
/// - `"YES\n<knowledge>...</knowledge>"` (knowledge stripped → empty),
/// - `force=true` response that is ONLY knowledge tags (also strips to empty),
/// - whitespace, a stray punctuation, or other near-empty noise.
///
/// If we accept any of those, `apply_compression` would drain dozens of messages and
/// insert a header-only "## Conversation Summary" block — wiping the entire context.
/// This guard refuses to compress in that case so the caller sets a cooldown and the
/// session keeps its real history.
pub(super) const MIN_SUMMARY_LEN: usize = 20;

/// True if a candidate summary is substantive enough to replace compressed messages.
pub(super) fn is_summary_valid(summary: &str) -> bool {
	summary.trim().chars().count() >= MIN_SUMMARY_LEN
}

/// Parse the AI response into a compression decision and optional summary text.
///
/// `force=true`: entire response is the summary (no YES/NO gate).
/// `force=false`: first line must be YES to proceed; NO means skip compression.
///
/// SAFETY: A response that yields a too-short summary (after trim + knowledge-tag
/// strip) is treated as a compression failure and returns `(false, "")` — even on
/// the `force` path. Better to skip compression than to wipe the conversation with
/// an empty summary. See `MIN_SUMMARY_LEN`.
pub(super) fn parse_ai_response(
	session: &mut ChatSession,
	config: &Config,
	content: &str,
	force: bool,
) -> Result<(bool, String)> {
	let content = content.trim();
	let lines: Vec<&str> = content.lines().collect();

	if lines.is_empty() {
		if force {
			return Err(anyhow::anyhow!(
				"AI returned empty summary during forced compression"
			));
		}
		log_debug!("AI compression decision: NO (empty response)");
		return Ok((false, String::new()));
	}

	// Extract and store critical knowledge from <knowledge> tags before returning summary
	extract_and_store_knowledge(session, config, content);

	if force {
		// Entire response is the summary — no YES/NO prefix expected.
		let summary = strip_knowledge_tags(content);
		if !is_summary_valid(&summary) {
			log_info!(
				"Compression aborted: AI returned too-short summary ({} chars, force=true). Skipping compression to avoid context loss.",
				summary.trim().chars().count()
			);
			return Ok((false, String::new()));
		}
		log_debug!("AI forced compression summary ({} chars)", summary.len());
		return Ok((true, summary));
	}

	let first_line = lines[0].trim().to_uppercase();
	let decision = first_line.contains("YES");

	if decision {
		let summary = if lines.len() > 1 {
			let raw = lines[1..].join("\n").trim().to_string();
			strip_knowledge_tags(&raw)
		} else {
			String::new()
		};
		if !is_summary_valid(&summary) {
			log_info!(
				"Compression aborted: AI said YES but summary is too short ({} chars). Skipping compression to avoid context loss.",
				summary.trim().chars().count()
			);
			return Ok((false, String::new()));
		}
		log_debug!(
			"AI compression decision: YES with summary ({} chars)",
			summary.len()
		);
		Ok((true, summary))
	} else {
		log_debug!("AI compression decision: NO");
		Ok((false, String::new()))
	}
}

pub(super) async fn ask_ai_decision_and_summary(
	session: &mut ChatSession,
	config: &Config,
	messages_to_compress: &[crate::session::Message],
	operation_rx: tokio::sync::watch::Receiver<bool>,
	force: bool,
	target_ratio: f64,
) -> Result<(bool, String)> {
	let (system_content, user_content) =
		build_compression_prompt(session, messages_to_compress, force, target_ratio);
	let response_content =
		call_ai_for_decision(session, config, system_content, user_content, operation_rx).await?;
	parse_ai_response(session, config, &response_content, force)
}
