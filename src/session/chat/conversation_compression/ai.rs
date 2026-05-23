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

	// Provider should return the validated JSON in `structured_output`. When
	// it's absent, try a lenient recovery from `response.content` before
	// erroring — some providers (notably OctoHub) ship a strict text-to-JSON
	// extractor that misses valid JSON wrapped in markdown fences or with
	// a chatty preamble, even when the model genuinely followed the schema.
	// Native-structured-output models (GPT, Claude, Gemini direct) hit the
	// happy path; the fallback covers cross-routing quirks for DeepSeek,
	// Kimi, OctoHub-routed models, etc.
	let raw = match response.structured_output {
		Some(v) => v,
		None => {
			let recovered = extract_json_lenient(&response.content).ok_or_else(|| {
				anyhow::anyhow!(
					"Compression model '{}' returned no structured_output and no recoverable JSON in text content",
					decision_config.model
				)
			})?;
			log_debug!(
				"Compression model '{}' omitted structured_output; recovered JSON from text content",
				decision_config.model
			);
			recovered
		}
	};

	let summary: CompressionSummary = serde_json::from_value(raw).map_err(|e| {
		anyhow::anyhow!(
			"Failed to deserialize compression schema response: {}. The provider returned JSON that does not match the expected shape.",
			e
		)
	})?;

	Ok(summary)
}

/// Best-effort JSON extraction from a text response when the provider didn't
/// populate `structured_output`. Handles three common provider patterns:
///
///   1. Bare JSON: `{"…": …}`
///   2. Fenced JSON: <code>```json\n{…}\n```</code> or unlabeled fences
///   3. Prose preamble: `"Here is the analysis: {…}"`
///
/// Returns `None` if no parseable JSON object/array can be located.
fn extract_json_lenient(content: &str) -> Option<serde_json::Value> {
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

#[cfg(test)]
mod extract_json_lenient_tests {
	use super::extract_json_lenient;

	#[test]
	fn parses_bare_object() {
		let v = extract_json_lenient(r#"{"should_compress": true, "x": 1}"#).unwrap();
		assert_eq!(v["should_compress"], true);
		assert_eq!(v["x"], 1);
	}

	#[test]
	fn parses_bare_array() {
		let v = extract_json_lenient(r#"[1, 2, 3]"#).unwrap();
		assert_eq!(v.as_array().unwrap().len(), 3);
	}

	#[test]
	fn strips_json_labeled_markdown_fence() {
		let input = "```json\n{\"should_compress\": false}\n```";
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["should_compress"], false);
	}

	#[test]
	fn strips_unlabeled_markdown_fence() {
		let input = "```\n{\"k\": \"v\"}\n```";
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["k"], "v");
	}

	#[test]
	fn recovers_from_chatty_preamble() {
		let input = "Here is the analysis:\n{\"should_compress\": true, \"target\": 2.0}";
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["should_compress"], true);
		assert_eq!(v["target"], 2.0);
	}

	#[test]
	fn recovers_from_preamble_with_fence() {
		let input = "Sure, here you go:\n```json\n{\"a\": 1}\n```\nDone!";
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["a"], 1);
	}

	#[test]
	fn respects_braces_inside_strings() {
		// Naive brace-counting would balance early on the `{` inside the string;
		// the scanner must skip string contents.
		let input = r#"text {"label": "value with } brace", "n": 7}"#;
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["label"], "value with } brace");
		assert_eq!(v["n"], 7);
	}

	#[test]
	fn handles_escaped_quotes_in_strings() {
		let input = r#"prefix {"msg": "she said \"hi\""}"#;
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["msg"], "she said \"hi\"");
	}

	#[test]
	fn returns_none_for_empty_input() {
		assert!(extract_json_lenient("").is_none());
		assert!(extract_json_lenient("   \n\t  ").is_none());
	}

	#[test]
	fn returns_none_for_no_json() {
		assert!(extract_json_lenient("just a plain text response with no JSON").is_none());
	}

	#[test]
	fn returns_none_for_truncated_json() {
		// Opener with no matching close — provider got cut off.
		assert!(extract_json_lenient(r#"{"incomplete": "no closing brace"#).is_none());
	}

	#[test]
	fn skips_invalid_object_finds_later_valid_one() {
		// First {…} has a syntax error; scanner must keep going and find the second.
		let input = r#"garbage {not valid json} more text {"ok": true}"#;
		let v = extract_json_lenient(input).unwrap();
		assert_eq!(v["ok"], true);
	}
}
