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

// Materialise a compression decision against the session: drain the chosen
// range, insert the synthetic summary message (with inherited response_id for
// chain continuity), re-inject the most recent user turn, fold knowledge,
// update anchor + token bookkeeping. Pure side-effects on `ChatSession`.

use super::decision::estimate_future_turns;
use super::knowledge::format_compressed_entry_with_context;
use crate::log_debug;
use crate::session::chat::file_context;
use crate::session::chat::session::ChatSession;
use crate::session::estimate_tokens;
use anyhow::Result;

/// Open tag for the synthetic post-compression continuation wrapper.
/// Detected verbatim by `is_continuation_message` so the next compression
/// cycle's user-msg filter excludes these from `all_user_msgs`.
const CONTINUATION_TAG_OPEN: &str = "<continuation>";

/// True if `content` is a synthetic continuation wrapper inserted by a
/// prior compression cycle (not a real user ask). Mirrors the
/// skill-message detection pattern used elsewhere in the session.
pub(super) fn is_continuation_message(content: &str) -> bool {
	content.trim_start().starts_with(CONTINUATION_TAG_OPEN)
}

/// Build the SOTA continuation wrapper for the trailing user turn after a
/// compressed summary. `intent` is the most recent real user message
/// content (already trimmed); when absent, the wrapper points the model
/// at the summary itself as the source of truth.
///
/// Shape:
/// ```text
/// <continuation>
/// The conversation summary above is the complete record of prior work
/// on this task — partial progress, decisions, and findings are all
/// captured there. Resume from where the previous turn left off; do not
/// restart or re-discover what is already established.
///
/// <task>
/// {intent OR "see summary above for the active task"}
/// </task>
/// </continuation>
/// ```
fn build_continuation_content(intent: Option<&str>) -> String {
	let task_body = intent.unwrap_or("see summary above for the active task");
	format!(
		"<continuation>\n\
		The conversation summary above is the complete record of prior work on this task — partial progress, decisions, and findings are all captured there. Resume from where the previous turn left off; do not restart or re-discover what is already established.\n\n\
		<task>\n{}\n</task>\n\
		</continuation>",
		task_body
	)
}

/// Apply compression: drain all messages, insert summary, re-inject recent user messages.
/// Also parses and injects file contexts if given by AI.
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_compression(
	session: &mut ChatSession,
	start_idx: usize,
	end_idx: usize,
	context_summary: &str,
	tokens_before: u64,
	current_context_tokens: u64,
	user_tasks_msgs: Vec<String>,
	last_user_message: Option<crate::session::Message>,
	preserved_skills: Vec<crate::session::Message>,
) -> Result<()> {
	// Parse file contexts from AI summary (AI may request specific file ranges to re-inject)
	let file_contexts = file_context::parse_file_contexts(context_summary);

	// Generate file context content if any contexts found
	let file_context_content = if !file_contexts.is_empty() {
		crate::log_debug!(
			"Compression: AI requested {} file context(s) for continuation",
			file_contexts.len()
		);
		for (filepath, start, end) in &file_contexts {
			crate::log_debug!("  - {} (lines {}-{})", filepath, start, end);
		}
		file_context::generate_file_context_content(&file_contexts)
	} else {
		String::new()
	};

	// Format compressed entry with file context
	let compression_id = crate::mcp::core::plan::compression::get_compression_id()
		.unwrap_or_else(|| "unknown".to_string());

	let base_entry = format_compressed_entry_with_context(
		context_summary,
		&file_context_content,
		compression_id,
	);

	// Prepend USER TASKS section (last 4 user requests, excluding the appended one).
	// These are raw user messages — not AI-rephrased — so intent is never lost.
	let compressed_entry = if user_tasks_msgs.is_empty() {
		base_entry
	} else {
		let user_tasks = user_tasks_msgs
			.iter()
			.enumerate()
			.map(|(i, msg)| format!("{}. {}", i + 1, msg))
			.collect::<Vec<_>>()
			.join("\n");
		format!("## USER TASKS\n{}\n\n{}", user_tasks, base_entry)
	};

	// Append the current active plan (if any) to the summary so the model doesn't have
	// to spend an extra `plan(list)` turn right after compression just to recover state.
	// Absence of a plan → no section injected.
	let compressed_entry = match crate::mcp::core::plan::core::get_current_plan_display().await {
		Ok(plan_display) => format!(
			"{}\n\nCurrent plan we are working on:\n<plan>\n{}\n</plan>",
			compressed_entry,
			plan_display.trim()
		),
		Err(_) => compressed_entry,
	};

	let tokens_after = estimate_tokens(&compressed_entry) as u64;

	// CRITICAL: Capture the most recent assistant response_id from the range we're
	// about to drain. The Responses API (OpenAI + OctoHub) chains via this id —
	// the server stores prior turns under it and reconstructs full history from
	// the chain. If we drain every id-bearing assistant and leave the summary
	// without one, the next request finds no `previous_id`, falls into the
	// "initial request" branch of `messages_to_input`, which filters out the
	// summary (role=assistant) entirely. The model then receives only the
	// re-injected user turn with zero context — exactly the "lost YES / plan
	// approval" failure mode. Inheriting the id keeps the server-side chain
	// intact while local view shrinks for token budget.
	//
	// The inherited id must point to a SETTLED completion — one whose stored
	// output did not end with `function_call` items. When the server walks the
	// chain back from an unsettled id, the reconstructed history ends with
	// `assistant_with_tool_calls`, and the next request (whose `input` after
	// compression is a re-injected user message, not the matching tool_results)
	// produces `tool_use → user` upstream, which Anthropic rejects with:
	//   "tool_use ids were found without tool_result blocks immediately after".
	// An assistant message with non-empty `tool_calls` corresponds to a
	// completion whose stored output had `function_call` items, so we skip
	// those when scanning the drained range.
	let inherited_response_id: Option<String> = session.session.messages[start_idx + 1..=end_idx]
		.iter()
		.rev()
		.find(|m| {
			m.role == "assistant"
				&& m.id.is_some()
				&& match m.tool_calls.as_ref() {
					Some(serde_json::Value::Array(arr)) => arr.is_empty(),
					Some(_) => false,
					None => true,
				}
		})
		.and_then(|m| m.id.clone());

	if let Some(ref id) = inherited_response_id {
		log_debug!(
			"Compression: inheriting last assistant response_id={} onto summary to preserve chain continuity",
			id
		);
	} else {
		log_debug!(
			"Compression: no assistant response_id found in drained range; summary will start a fresh chain"
		);
	}

	// COMPRESS-ALL: Drain everything from start_idx+1 to end_idx
	let (messages_removed, _) = session.remove_messages_in_range(start_idx, end_idx)?;

	// Insert summary + re-injected user message in one shot with correct cache markers.
	// Cache markers: marker #1 on summary, marker #2 on re-injected user message.
	// Evict existing content markers first to enforce the 2-marker limit.
	let supports_caching = crate::session::model_supports_caching(&session.session.info.model);
	// Evict stale content markers — but preserve the anchor's marker.
	// The anchor (instructions) keeps its cache marker from session start.
	// Set 1h TTL on anchor when long cache is enabled — stable prefix, rarely changes.
	if supports_caching {
		for (i, msg) in session.session.messages.iter_mut().enumerate() {
			if i == start_idx {
				// Anchor: keep marker, set long TTL (always enabled — Anthropic 1h cache).
				msg.cached = true;
				msg.cache_ttl = Some("1h".to_string());
			} else if msg.cached && msg.role != "system" {
				msg.cached = false;
				msg.cache_ttl = None;
			}
		}
	}

	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	// Insert preserved active skills FIRST, between the anchor (which keeps
	// cache marker #1) and the summary. Skills carry no cache markers — the
	// two-marker budget is reserved for anchor + re-injected user. Order is
	// preserved relative to each other, matching the user's expectation that
	// active skills sit at the top of the recovered context:
	//   [system, anchor(marker#1), skill1, skill2, …, summary, user(marker#2), …]
	let skill_count = preserved_skills.len();
	for (i, mut skill_msg) in preserved_skills.into_iter().enumerate() {
		// Defensive: clear cache markers so we never blow the 2-marker budget.
		skill_msg.cached = false;
		skill_msg.cache_ttl = None;
		session
			.session
			.messages
			.insert(start_idx + 1 + i, skill_msg);
	}
	if skill_count > 0 {
		log_debug!(
			"Compression: preserved {} active skill message(s) across compression",
			skill_count
		);
	}

	// Summary message (no cache marker — sits between anchor marker and user marker).
	// The `id` is inherited from the most recent assistant turn in the drained range
	// so the provider can chain via `previous_response_id` on the next API call.
	let summary_msg = crate::session::Message {
		role: "assistant".to_string(),
		content: compressed_entry,
		timestamp: now,
		cached: false,
		name: Some("plan_compression".to_string()),
		id: inherited_response_id,
		..Default::default()
	};
	session
		.session
		.messages
		.insert(start_idx + 1 + skill_count, summary_msg);

	// Marker #2: re-injected continuation message — full content cache
	// boundary. This is ALWAYS a synthetic <continuation> wrapper, never
	// the raw user message verbatim. The wrapper:
	//   - signals to the model that this is an in-progress task (the
	//     summary above captures completed work), preventing "fresh
	//     start" hallucinations after compression;
	//   - carries the most recent real user intent inside <task> so the
	//     model has a clear current focus;
	//   - is tagged so the next compression cycle's user-msg filter skips
	//     it (see `is_continuation_message`), keeping USER TASKS sourced
	//     only from real user asks and preventing cross-cycle decay.
	//
	// `last_user_message = None` is only possible on a session with no
	// real user message anywhere (pathological bootstrap-only state); the
	// wrapper falls back to pointing at the summary itself.
	let continuation_intent = last_user_message
		.as_ref()
		.map(|m| m.content.trim().to_string());
	let continuation_msg = crate::session::Message {
		role: "user".to_string(),
		content: build_continuation_content(continuation_intent.as_deref()),
		timestamp: now,
		cached: supports_caching,
		..Default::default()
	};
	session
		.session
		.messages
		.insert(start_idx + 2 + skill_count, continuation_msg);
	log_debug!(
		"Inserted continuation wrapper after compressed summary (USER TASKS: {}, intent_source: {})",
		user_tasks_msgs.len(),
		if continuation_intent.is_some() {
			"last_user_message"
		} else {
			"summary_fallback"
		}
	);

	// Update first_prompt_idx to the actual anchor used for this compression.
	session.first_prompt_idx = Some(start_idx);

	// Calculate metrics
	let tokens_saved = tokens_before.saturating_sub(tokens_after);

	let metrics = crate::mcp::core::plan::compression::CompressionMetrics::new(
		messages_removed,
		tokens_saved,
		tokens_before,
	);

	crate::session::chat::cost_tracker::CostTracker::display_compression_result(
		"Conversation",
		&metrics,
	);

	// Track stats
	session.session.info.compression_stats.add_compression(
		crate::session::CompressionKind::Conversation,
		messages_removed,
		tokens_saved,
	);

	// Token-based cooldown: record post-compression context size.
	// Next compression is allowed only after context grows ≥10% above this watermark,
	// preventing futile back-to-back compressions while reacting to actual growth.
	let post_compression_tokens = current_context_tokens.saturating_sub(tokens_saved);
	session.session.info.context_tokens_after_last_compression = post_compression_tokens as usize;

	// SELF-TUNING: Record checkpoint for incremental growth rate tracking.
	// output_tokens_at_last_compression lets estimate_future_turns measure growth since
	// this compression only, not the inflated lifetime average.
	let estimated_future_turns = estimate_future_turns(session, tokens_saved as f64);
	let api_calls_at_compression = session.session.info.total_api_calls;
	session.session.info.predicted_turns_at_last_compression = estimated_future_turns;
	session.session.info.api_calls_at_last_compression = api_calls_at_compression;
	session.session.info.output_tokens_at_last_compression = session.session.info.output_tokens;

	log_debug!(
		"Compression cooldown set: post_compression_tokens={}, consecutive={}, requires ≥{:.0}% growth before next compression",
		post_compression_tokens,
		session.session.info.consecutive_compressions,
		(0.10 * 2.0_f64.powi(session.session.info.consecutive_compressions as i32)).min(1.0) * 100.0
	);

	// CRITICAL: Log compression point to session file
	// This marker tells session loader to clear messages before this point on resume
	// Without this, all "compressed" messages are reloaded, defeating compression
	let _ = crate::session::logger::log_compression_point(
		&session.session.info.name,
		"conversation",
		messages_removed,
		tokens_saved,
		&session.session.messages,
	);

	// Extend the session anchor so conversation compaction contributes to
	// cross-compaction continuity. Heuristic update: record a marker entry
	// with the metrics; subsequent task compactions (which embed the anchor
	// in their compressed-knowledge messages) surface it in context.
	{
		let now_unix = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.unwrap_or_default()
			.as_secs();
		let intent_seed = if session.session.info.anchor.intent.is_empty() {
			Some("Free-form conversation session".to_string())
		} else {
			None
		};
		session.session.info.anchor.extend(
			crate::session::anchor::AnchorUpdate {
				intent: intent_seed,
				changes_made: vec![format!(
					"Conversation compaction: {} messages folded, {} tokens saved",
					messages_removed, tokens_saved
				)],
				..Default::default()
			},
			now_unix,
		);
	}

	// (dedup state is cleared inside `remove_messages_in_range` — see core.rs.)

	// CRITICAL FIX: Reset token tracking for fresh start after compression
	// This prevents token drift and ensures accurate cache/pricing calculations
	// Mirrors the behavior in context_truncation.rs::perform_smart_full_summarization()
	session.session.info.current_non_cached_tokens = 0;
	session.session.info.current_total_tokens = 0;

	// Reset cache checkpoint time
	session.session.info.last_cache_checkpoint_time = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs();

	Ok(())
}

/// Collect active skill messages from a compression drain range so they can be
/// re-inserted after the summary. Skill messages are user-role entries whose
/// content is wrapped in `<skill name="...">…</skill>` tags.
///
/// Only skills in `active_skill_names` are preserved — a skill the user
/// explicitly forgot (or that was never registered as active) is dropped.
///
/// Duplicate skill names (same skill injected multiple times) are deduped
/// keeping the LAST occurrence in the range, preserving the freshest content.
/// Relative order of distinct skills is preserved (by last-seen position).
pub(super) fn collect_preserved_skills(
	messages: &[crate::session::Message],
	range_start: usize,
	range_end: usize,
	active_skill_names: &[String],
) -> Vec<crate::session::Message> {
	if range_start > range_end || range_end >= messages.len() {
		return Vec::new();
	}

	// Walk the range once, recording the last index per skill name.
	// Using a Vec<(name, idx)> to preserve insertion order of first-seen names
	// while still letting us update the idx to the latest occurrence.
	let mut order: Vec<String> = Vec::new();
	let mut last_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

	for (offset, msg) in messages[range_start..=range_end].iter().enumerate() {
		if msg.role != "user" {
			continue;
		}
		if !crate::mcp::core::skill::is_skill_message(&msg.content) {
			continue;
		}
		let name = match crate::mcp::core::skill::extract_skill_name(&msg.content) {
			Some(n) => n.to_string(),
			None => continue,
		};
		if !active_skill_names.iter().any(|n| n == &name) {
			continue;
		}
		let idx = range_start + offset;
		if last_idx.insert(name.clone(), idx).is_none() {
			order.push(name);
		}
	}

	order
		.into_iter()
		.filter_map(|name| last_idx.get(&name).map(|&i| messages[i].clone()))
		.collect()
}
