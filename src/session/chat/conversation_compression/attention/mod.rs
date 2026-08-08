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

//! PACT Core: deterministic evidence selection around the existing compressor.
//!
//! The model still performs the useful generative fold. The runtime owns the
//! task/constraint pins, tool-call atomicity, exact active frontier, source
//! identifiers, archive references, and attribution checks.

use super::schema::{CompressionSummary, FoldedUnit};
use crate::session::chat::session::ChatSession;
use crate::session::Message;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(crate) const CONTROLLER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PacketKind {
	UserTask,
	UserConstraintOrCorrection,
	AssistantCheckpoint,
	ToolInteraction,
	RuntimeEvent,
	PriorSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provenance {
	RealUser,
	RuntimeSystemManaged,
	AssistantReported,
	ToolObserved,
	ValidatedSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lane {
	KeepExact,
	Summarize,
	ArchiveReference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EvidencePacket {
	pub id: String,
	pub kind: PacketKind,
	pub provenance: Provenance,
	/// Inclusive offsets into the exact drained message slice.
	pub message_start: usize,
	pub message_end: usize,
	pub depends_on: Vec<String>,
	pub tokens: usize,
	pub lane: Lane,
	/// Exact source fragments shown to the compressor. When bounded, omission
	/// markers name the original rendered line ranges; no facts are rewritten.
	pub prompt_content: String,
	pub descriptor: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PinnedItem {
	pub text: String,
	pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PinnedState {
	pub task: PinnedItem,
	pub constraints: Vec<PinnedItem>,
	pub governance_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct GroundedHint {
	kind: &'static str,
	refs: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PactContext {
	enabled: bool,
	pub packets: Vec<EvidencePacket>,
	pub pinned: PinnedState,
	grounded_hints: Vec<GroundedHint>,
	known_provenance: BTreeMap<String, Provenance>,
	prior_recall: BTreeMap<String, super::archive::ArchivedBlockRef>,
	pub source_tokens: usize,
	pub target_tokens: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ValidationReport {
	pub attribution_valid: bool,
	pub fallback_reason: Option<String>,
	pub valid_units: usize,
	pub referenced_blocks: usize,
	pub governance_hash: String,
}

/// Build PACT over the exact range that will be archived and drained.
pub(crate) async fn build(
	session: &ChatSession,
	drain_start: usize,
	drain_end: usize,
	target_ratio: f64,
	attention_enabled: bool,
) -> Result<PactContext> {
	if drain_start > drain_end || drain_end >= session.session.messages.len() {
		return Err(anyhow!(
			"invalid PACT drain range {drain_start}..={drain_end}"
		));
	}
	let drained = &session.session.messages[drain_start..=drain_end];
	let mut packets = build_packets(&session.session.info.name, drained);
	link_dependencies(&mut packets);

	let task_turn = crate::session::latest_task_turn_index(&session.session.messages);
	let task_text = crate::session::latest_real_user_task_content(&session.session.messages)
		.unwrap_or_default()
		.trim()
		.to_string();
	let task_source = task_turn
		.filter(|index| *index >= drain_start && *index <= drain_end)
		.and_then(|index| packet_for_offset(&packets, index - drain_start))
		.map(|packet| packet.id.clone());

	let constraints = collect_constraints(&session.session.messages, drain_start, &packets);
	let governance_hash = governance_hash(&session.session.messages, &task_text, &constraints);
	let pinned = PinnedState {
		task: PinnedItem {
			text: task_text,
			source: task_source,
		},
		constraints,
		governance_hash,
	};

	let source_tokens = packets.iter().map(|packet| packet.tokens).sum::<usize>();
	let target_tokens = ((source_tokens as f64) / target_ratio.max(1.0)).ceil() as usize;
	let grounded_hints = ground_self_report(session, drained, &packets);
	if attention_enabled {
		let plan_focus = crate::mcp::core::plan::core::get_current_plan_display()
			.await
			.unwrap_or_default();
		allocate_lanes(
			&mut packets,
			drained,
			&pinned,
			&grounded_hints,
			&plan_focus,
			target_tokens,
		)
		.await;
	}

	let registry = super::archive::read_session_block_registry(&session.session.info.name);
	let mut known_provenance: BTreeMap<String, Provenance> = registry
		.iter()
		.map(|(id, entry)| (id.clone(), entry.provenance))
		.collect();
	for packet in &packets {
		known_provenance.insert(packet.id.clone(), packet.provenance);
	}
	let prior_recall = registry
		.into_iter()
		.filter(|(id, _)| {
			packets
				.iter()
				.any(|packet| packet.prompt_content.contains(id))
		})
		.collect();

	Ok(PactContext {
		enabled: attention_enabled,
		packets,
		pinned,
		grounded_hints,
		known_provenance,
		prior_recall,
		source_tokens,
		target_tokens,
	})
}

fn build_packets(session_name: &str, messages: &[Message]) -> Vec<EvidencePacket> {
	let mut packets = Vec::new();
	let mut index = 0usize;
	while index < messages.len() {
		let message = &messages[index];
		if message.role == "system"
			|| (message.role == "user"
				&& (crate::mcp::runtime::skill::is_skill_message(&message.content)
					|| message.content.trim_start().starts_with("<instructions>")))
		{
			index += 1;
			continue;
		}

		let start = index;
		let mut end = index;
		if message.role == "assistant" && has_tool_calls(message) {
			let call_ids = tool_call_ids(message);
			while end + 1 < messages.len() && messages[end + 1].role == "tool" {
				let result_id = messages[end + 1].tool_call_id.as_deref();
				if call_ids.is_empty() || result_id.is_none_or(|id| call_ids.contains(id)) {
					end += 1;
				} else {
					break;
				}
			}
		}

		let slice = &messages[start..=end];
		let (kind, provenance) = classify_packet(slice);
		let tokens = slice
			.iter()
			.map(crate::session::estimate_message_tokens)
			.sum();
		let id = stable_packet_id(session_name, slice);
		packets.push(EvidencePacket {
			id,
			kind,
			provenance,
			message_start: start,
			message_end: end,
			depends_on: Vec::new(),
			tokens,
			lane: Lane::ArchiveReference,
			prompt_content: String::new(),
			descriptor: format!(
				"{:?} / {:?}; {} message(s), approximately {} tokens",
				kind,
				provenance,
				end - start + 1,
				tokens
			),
		});
		index = end + 1;
	}
	packets
}

fn classify_packet(messages: &[Message]) -> (PacketKind, Provenance) {
	let first = &messages[0];
	if first.role == "user" {
		if crate::session::is_real_user_task_message(first) {
			let has_constraint =
				!crate::supervisor::recite::extract_constraints(&first.content).is_empty();
			return (
				if has_constraint {
					PacketKind::UserConstraintOrCorrection
				} else {
					PacketKind::UserTask
				},
				Provenance::RealUser,
			);
		}
		return (PacketKind::RuntimeEvent, Provenance::RuntimeSystemManaged);
	}
	if first.role == "tool" || messages.iter().any(|message| message.role == "tool") {
		return (PacketKind::ToolInteraction, Provenance::ToolObserved);
	}
	if has_tool_calls(first) {
		return (PacketKind::ToolInteraction, Provenance::AssistantReported);
	}
	if first.role == "assistant"
		&& (first.name.as_deref() == Some(super::apply::COMPRESSION_MESSAGE_NAME)
			|| first
				.content
				.contains(super::knowledge::SUMMARY_TAG_OPEN_PREFIX))
	{
		return (PacketKind::PriorSummary, Provenance::ValidatedSummary);
	}
	(
		PacketKind::AssistantCheckpoint,
		Provenance::AssistantReported,
	)
}

fn has_tool_calls(message: &Message) -> bool {
	message
		.tool_calls
		.as_ref()
		.and_then(serde_json::Value::as_array)
		.is_some_and(|calls| !calls.is_empty())
}

fn tool_call_ids(message: &Message) -> HashSet<&str> {
	message
		.tool_calls
		.as_ref()
		.and_then(serde_json::Value::as_array)
		.into_iter()
		.flatten()
		.filter_map(|call| call.get("id").and_then(serde_json::Value::as_str))
		.collect()
}

fn stable_packet_id(session_name: &str, messages: &[Message]) -> String {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-packet-v1\0");
	hasher.update(session_name.as_bytes());
	for message in messages {
		hasher.update([0]);
		let encoded = serde_json::to_vec(message).expect("session messages are serializable");
		hasher.update(encoded);
	}
	format!("b:{}", short_hex(&hasher.finalize()))
}

fn short_hex(bytes: &[u8]) -> String {
	bytes
		.iter()
		.take(16)
		.map(|byte| format!("{byte:02x}"))
		.collect()
}

fn packet_for_offset(packets: &[EvidencePacket], offset: usize) -> Option<&EvidencePacket> {
	packets
		.iter()
		.find(|packet| (packet.message_start..=packet.message_end).contains(&offset))
}

fn link_dependencies(packets: &mut [EvidencePacket]) {
	let mut latest_task: Option<String> = None;
	let mut latest_summary: Option<String> = None;
	for index in 0..packets.len() {
		let mut dependencies = Vec::new();
		match packets[index].kind {
			PacketKind::UserTask | PacketKind::UserConstraintOrCorrection => {
				if let Some(task) = latest_task.as_ref() {
					dependencies.push(task.clone());
				}
				latest_task = Some(packets[index].id.clone());
			}
			PacketKind::PriorSummary => {
				latest_summary = Some(packets[index].id.clone());
			}
			PacketKind::RuntimeEvent => {
				if let Some(task) = latest_task.as_ref() {
					dependencies.push(task.clone());
				}
				if let Some(summary) = latest_summary.as_ref() {
					dependencies.push(summary.clone());
				}
			}
			PacketKind::ToolInteraction => {
				if let Some(task) = latest_task.as_ref() {
					dependencies.push(task.clone());
				}
			}
			PacketKind::AssistantCheckpoint => {
				if index > 0 && packets[index - 1].kind == PacketKind::ToolInteraction {
					dependencies.push(packets[index - 1].id.clone());
				} else if let Some(task) = latest_task.as_ref() {
					dependencies.push(task.clone());
				}
			}
		}
		dependencies.sort();
		dependencies.dedup();
		packets[index].depends_on = dependencies;
	}
}

fn collect_constraints(
	messages: &[Message],
	drain_start: usize,
	packets: &[EvidencePacket],
) -> Vec<PinnedItem> {
	let mut seen = BTreeSet::new();
	let mut constraints = Vec::new();
	for (index, message) in messages.iter().enumerate() {
		if !crate::session::is_real_user_task_message(message) {
			continue;
		}
		let source = index
			.checked_sub(drain_start)
			.and_then(|offset| packet_for_offset(packets, offset))
			.map(|packet| packet.id.clone());
		for constraint in crate::supervisor::recite::extract_constraints(&message.content) {
			if seen.insert(constraint.clone()) {
				constraints.push(PinnedItem {
					text: constraint,
					source: source.clone(),
				});
			}
		}
	}
	constraints
}

fn governance_hash(messages: &[Message], task: &str, constraints: &[PinnedItem]) -> String {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-governance-v1\0");
	for message in messages.iter().filter(|message| message.role == "system") {
		hasher.update(message.content.as_bytes());
		hasher.update([0]);
	}
	hasher.update(task.as_bytes());
	for constraint in constraints {
		hasher.update([0]);
		hasher.update(constraint.text.as_bytes());
	}
	short_hex(&hasher.finalize())
}

fn ground_self_report(
	session: &ChatSession,
	messages: &[Message],
	packets: &[EvidencePacket],
) -> Vec<GroundedHint> {
	let Some(handoff) = session.last_self_report_handoff.as_ref() else {
		return Vec::new();
	};
	ground_handoff(handoff, messages, packets)
}

fn ground_handoff(
	handoff: &crate::supervisor::detect::SelfReportHandoff,
	messages: &[Message],
	packets: &[EvidencePacket],
) -> Vec<GroundedHint> {
	let mut candidates: Vec<(&'static str, &str)> = Vec::new();
	if !handoff.focus.trim().is_empty() {
		candidates.push(("focus", handoff.focus.trim()));
	}
	if !handoff.next.trim().is_empty() {
		candidates.push(("next", handoff.next.trim()));
	}
	for carry in &handoff.carry {
		if !carry.trim().is_empty() {
			candidates.push(("carry", carry.trim()));
		}
	}

	let packet_texts: Vec<(String, String)> = packets
		.iter()
		.map(|packet| {
			(
				packet.id.clone(),
				normalize_for_match(&render_packet(messages, packet, usize::MAX)),
			)
		})
		.collect();
	let mut grounded = Vec::new();
	for (kind, text) in candidates {
		let normalized = normalize_for_match(text);
		if normalized.len() < 8 {
			continue;
		}
		let refs: Vec<String> = packet_texts
			.iter()
			.filter(|(id, content)| text.contains(id) || content.contains(&normalized))
			.map(|(id, _)| id.clone())
			.collect();
		if !refs.is_empty() {
			grounded.push(GroundedHint { kind, refs });
		}
	}
	grounded
}

fn normalize_for_match(text: &str) -> String {
	text.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ")
		.to_lowercase()
}

async fn allocate_lanes(
	packets: &mut [EvidencePacket],
	messages: &[Message],
	pinned: &PinnedState,
	grounded_hints: &[GroundedHint],
	plan_focus: &str,
	target_tokens: usize,
) {
	if packets.is_empty() {
		return;
	}

	let exact_ids = active_dependency_closure(packets);
	let mut exact_remaining = target_tokens;
	let mut exact_left = exact_ids.len();
	let mut used = 0usize;
	for packet in packets.iter_mut() {
		if exact_ids.contains(&packet.id) {
			let packet_budget = if exact_left == 0 {
				0
			} else {
				exact_remaining.div_ceil(exact_left)
			};
			packet.lane = Lane::KeepExact;
			packet.prompt_content = render_packet(messages, packet, packet_budget);
			let cost = crate::session::estimate_tokens(&packet.prompt_content);
			used = used.saturating_add(cost);
			exact_remaining = exact_remaining.saturating_sub(cost);
			exact_left = exact_left.saturating_sub(1);
		}
	}

	let mut candidates: Vec<usize> = packets
		.iter()
		.enumerate()
		.filter(|(_, packet)| {
			packet.lane == Lane::ArchiveReference && packet.provenance != Provenance::RealUser
		})
		.map(|(index, _)| index)
		.collect();
	let mut remaining = target_tokens.saturating_sub(used);
	if remaining == 0 || candidates.is_empty() {
		return;
	}
	let previews: Vec<(usize, String, usize)> = candidates
		.iter()
		.filter_map(|index| {
			let budget = packets[*index].tokens.div_ceil(2).max(1);
			let content = render_packet(messages, &packets[*index], budget);
			if !prompt_has_exact_fragment(&content) {
				return None;
			}
			let cost = crate::session::estimate_tokens(&content);
			Some((*index, content, cost))
		})
		.collect();
	let preview_total = previews
		.iter()
		.map(|(_, _, cost)| *cost)
		.fold(0usize, usize::saturating_add);
	if previews.len() == candidates.len() && preview_total <= remaining {
		for (index, content, cost) in previews {
			packets[index].lane = Lane::Summarize;
			packets[index].prompt_content = content;
			remaining = remaining.saturating_sub(cost);
		}
		return;
	}
	let query = format!(
		"{}\n{}\n{}",
		pinned.task.text,
		pinned
			.constraints
			.iter()
			.map(|item| item.text.as_str())
			.collect::<Vec<_>>()
			.join("\n"),
		plan_focus
	);
	let grounded_refs: HashSet<&str> = grounded_hints
		.iter()
		.flat_map(|hint| hint.refs.iter().map(String::as_str))
		.collect();
	rank_candidates(&mut candidates, packets, messages, &query, &grounded_refs).await;

	for index in candidates {
		if remaining == 0 {
			break;
		}
		if packets[index].lane != Lane::ArchiveReference {
			continue;
		}
		let closure = summarization_closure(index, packets);
		let pending: Vec<usize> = closure
			.into_iter()
			.filter(|candidate| {
				packets[*candidate].lane == Lane::ArchiveReference
					&& packets[*candidate].provenance != Provenance::RealUser
			})
			.collect();
		if pending.is_empty() {
			continue;
		}
		let per_packet = remaining.div_ceil(pending.len());
		let rendered: Vec<(usize, String, usize)> = pending
			.iter()
			.filter_map(|candidate| {
				let desired = packets[*candidate].tokens.div_ceil(2).max(1);
				let content =
					render_packet(messages, &packets[*candidate], desired.min(per_packet));
				prompt_has_exact_fragment(&content).then(|| {
					let cost = crate::session::estimate_tokens(&content);
					(*candidate, content, cost)
				})
			})
			.collect();
		if rendered.len() != pending.len() {
			continue;
		}
		let cost = rendered
			.iter()
			.map(|(_, _, cost)| *cost)
			.fold(0usize, usize::saturating_add);
		if cost > remaining {
			continue;
		}
		for (candidate, content, _) in rendered {
			packets[candidate].lane = Lane::Summarize;
			packets[candidate].prompt_content = content;
		}
		remaining = remaining.saturating_sub(cost);
	}
}

fn summarization_closure(index: usize, packets: &[EvidencePacket]) -> Vec<usize> {
	let by_id: BTreeMap<&str, usize> = packets
		.iter()
		.enumerate()
		.map(|(index, packet)| (packet.id.as_str(), index))
		.collect();
	let mut selected = BTreeSet::new();
	let mut stack = vec![index];
	while let Some(current) = stack.pop() {
		if !selected.insert(current) {
			continue;
		}
		for dependency in &packets[current].depends_on {
			if let Some(dependency_index) = by_id.get(dependency.as_str()) {
				stack.push(*dependency_index);
			}
		}
	}
	selected.into_iter().collect()
}

fn prompt_has_exact_fragment(content: &str) -> bool {
	content.lines().any(|line| {
		line.starts_with("[MESSAGE ")
			|| line
				.split_once("| ")
				.is_some_and(|(number, _)| number.chars().all(|ch| ch.is_ascii_digit()))
	})
}

fn active_dependency_closure(packets: &[EvidencePacket]) -> HashSet<String> {
	let Some(active) = packets.last() else {
		return HashSet::new();
	};
	if active.provenance == Provenance::RealUser {
		return HashSet::new();
	}
	let by_id: BTreeMap<&str, &EvidencePacket> = packets
		.iter()
		.map(|packet| (packet.id.as_str(), packet))
		.collect();
	let mut selected = HashSet::new();
	let mut stack = vec![active.id.as_str()];
	while let Some(id) = stack.pop() {
		if !selected.insert(id.to_string()) {
			continue;
		}
		if let Some(packet) = by_id.get(id) {
			for dependency in &packet.depends_on {
				// The genuine task is rendered in pinned_state and need not be
				// duplicated into the exact frontier.
				if by_id
					.get(dependency.as_str())
					.is_some_and(|p| matches!(p.provenance, Provenance::RealUser))
				{
					continue;
				}
				stack.push(dependency);
			}
		}
	}
	selected
}

async fn rank_candidates(
	candidates: &mut [usize],
	packets: &[EvidencePacket],
	messages: &[Message],
	query: &str,
	grounded_refs: &HashSet<&str>,
) {
	if candidates.len() < 2 || query.trim().is_empty() {
		candidates.sort_by_key(|index| {
			(
				!grounded_refs.contains(packets[*index].id.as_str()),
				std::cmp::Reverse(structural_rank(packets[*index].kind)),
				std::cmp::Reverse(*index),
			)
		});
		return;
	}
	let mut inputs: Vec<String> = candidates
		.iter()
		.map(|index| {
			let content = render_packet(messages, &packets[*index], 512);
			crate::embeddings::chunk_to_token_limit(
				&content,
				crate::embeddings::EMBED_MAX_INPUT_TOKENS,
			)
			.into_iter()
			.next()
			.unwrap_or_default()
		})
		.collect();
	inputs.push(
		crate::embeddings::chunk_to_token_limit(query, crate::embeddings::EMBED_MAX_INPUT_TOKENS)
			.into_iter()
			.next()
			.unwrap_or_default(),
	);
	let scores = match crate::embeddings::embed_many(&inputs).await {
		Ok(vectors) if vectors.len() == inputs.len() => {
			let query_vector = vectors.last().expect("non-empty inputs");
			Some(
				vectors[..vectors.len() - 1]
					.iter()
					.map(|vector| crate::embeddings::cosine(vector, query_vector))
					.collect::<Vec<_>>(),
			)
		}
		Ok(_) => None,
		Err(error) => {
			crate::log_debug!("PACT packet ranking fell back to structure: {}", error);
			None
		}
	};
	let original_position: BTreeMap<usize, usize> = candidates
		.iter()
		.copied()
		.enumerate()
		.map(|(position, index)| (index, position))
		.collect();
	candidates.sort_by(|left, right| {
		let grounded = grounded_refs
			.contains(packets[*right].id.as_str())
			.cmp(&grounded_refs.contains(packets[*left].id.as_str()));
		if !grounded.is_eq() {
			return grounded;
		}
		let left_position = original_position[left];
		let right_position = original_position[right];
		let relevance = scores
			.as_ref()
			.map(|values| values[right_position].total_cmp(&values[left_position]));
		relevance
			.filter(|ordering| !ordering.is_eq())
			.unwrap_or_else(|| {
				structural_rank(packets[*right].kind)
					.cmp(&structural_rank(packets[*left].kind))
					.then_with(|| right.cmp(left))
			})
	});
}

fn structural_rank(kind: PacketKind) -> u8 {
	match kind {
		PacketKind::UserConstraintOrCorrection => 5,
		PacketKind::PriorSummary => 4,
		PacketKind::UserTask => 3,
		PacketKind::AssistantCheckpoint => 2,
		PacketKind::ToolInteraction => 1,
		PacketKind::RuntimeEvent => 0,
	}
}

fn render_packet(messages: &[Message], packet: &EvidencePacket, max_tokens: usize) -> String {
	let mut rendered = String::new();
	for (offset, message) in messages[packet.message_start..=packet.message_end]
		.iter()
		.enumerate()
	{
		let source = packet.message_start + offset + 1;
		match message.role.as_str() {
			"assistant" => {
				let content = if message.name.as_deref()
					== Some(super::apply::COMPRESSION_MESSAGE_NAME)
					|| message
						.content
						.trim_start()
						.starts_with(super::knowledge::SUMMARY_TAG_OPEN_PREFIX)
				{
					super::knowledge::strip_regrown_sections(&message.content)
				} else {
					message.content.trim().to_string()
				};
				if !content.is_empty() {
					rendered.push_str(&format!("[MESSAGE {source} ASSISTANT]\n{}\n", content));
				}
				if let Some(calls) = message.tool_calls.as_ref() {
					rendered.push_str(&format!(
						"[MESSAGE {source} STRUCTURED TOOL CALLS]\n{calls}\n"
					));
				}
			}
			"tool" => rendered.push_str(&format!(
				"[MESSAGE {source} TOOL RESULT id={} name={}]\n{}\n",
				message.tool_call_id.as_deref().unwrap_or("unknown"),
				message.name.as_deref().unwrap_or("tool"),
				message.content.trim()
			)),
			"user" => rendered.push_str(&format!(
				"[MESSAGE {source} {}]\n{}\n",
				if crate::session::is_real_user_task_message(message) {
					"REAL USER"
				} else {
					"RUNTIME EVENT"
				},
				message.content.trim()
			)),
			_ => {}
		}
	}
	let rendered = rendered.trim_end().to_string();
	if max_tokens == usize::MAX || crate::session::estimate_tokens(&rendered) <= max_tokens {
		return rendered;
	}
	extractive_edges(&rendered, max_tokens)
}

fn extractive_edges(content: &str, max_tokens: usize) -> String {
	if max_tokens == 0 || content.is_empty() {
		return String::new();
	}
	let lines: Vec<&str> = content.lines().collect();
	if crate::session::estimate_tokens(content) <= max_tokens {
		return content.to_string();
	}
	let marker = |first: usize, last: usize| {
		format!("[… lines {first}-{last} omitted; exact recall by block ID …]")
	};
	let marker_tokens = crate::session::estimate_tokens(&marker(1, lines.len())).min(max_tokens);
	let payload_budget = max_tokens.saturating_sub(marker_tokens);
	let head_budget = payload_budget.div_ceil(2);
	let tail_budget = payload_budget.saturating_sub(head_budget);
	let mut head = Vec::new();
	for (index, line) in lines.iter().enumerate() {
		let candidate = format!("{}| {}", index + 1, line);
		let mut proposed = head.clone();
		proposed.push(candidate.clone());
		if crate::session::estimate_tokens(&proposed.join("\n")) > head_budget {
			break;
		}
		head.push(candidate);
	}
	let mut tail = Vec::new();
	for (index, line) in lines.iter().enumerate().rev() {
		if index < head.len() {
			break;
		}
		let candidate = format!("{}| {}", index + 1, line);
		let mut proposed = tail.clone();
		proposed.push(candidate.clone());
		if crate::session::estimate_tokens(&proposed.join("\n")) > tail_budget {
			break;
		}
		tail.push(candidate);
	}
	tail.reverse();
	let omitted_start = head.len() + 1;
	let omitted_end = lines.len().saturating_sub(tail.len());
	let mut parts = Vec::new();
	if !head.is_empty() {
		parts.push(head.join("\n"));
	}
	parts.push(marker(omitted_start, omitted_end));
	if !tail.is_empty() {
		parts.push(tail.join("\n"));
	}
	let mut result = parts.join("\n");
	while crate::session::estimate_tokens(&result) > max_tokens
		&& (head.len() > 1 || tail.len() > 1)
	{
		if head.len() >= tail.len() && head.len() > 1 {
			head.pop();
		} else if tail.len() > 1 {
			tail.remove(0);
		}
		let mut reduced = Vec::new();
		if !head.is_empty() {
			reduced.push(head.join("\n"));
		}
		reduced.push(marker(
			head.len() + 1,
			lines.len().saturating_sub(tail.len()),
		));
		if !tail.is_empty() {
			reduced.push(tail.join("\n"));
		}
		result = reduced.join("\n");
	}
	if crate::session::estimate_tokens(&result) <= max_tokens {
		result
	} else {
		crate::session::truncate_to_tokens(
			"[… exact packet omitted; recall by block ID …]",
			max_tokens,
		)
	}
}

impl PactContext {
	pub(crate) fn prompt_view(&self) -> String {
		#[derive(Serialize)]
		struct PacketView<'a> {
			id: &'a str,
			lane: Lane,
			kind: PacketKind,
			origin: Provenance,
			depends_on: &'a [String],
			content: Option<&'a str>,
			descriptor: Option<&'a str>,
		}
		let packets: Vec<PacketView<'_>> = self
			.packets
			.iter()
			.map(|packet| PacketView {
				id: &packet.id,
				lane: packet.lane,
				kind: packet.kind,
				origin: packet.provenance,
				depends_on: &packet.depends_on,
				content: (packet.lane != Lane::ArchiveReference)
					.then_some(packet.prompt_content.as_str()),
				descriptor: (packet.lane == Lane::ArchiveReference)
					.then_some(packet.descriptor.as_str()),
			})
			.collect();
		serde_json::to_string_pretty(&serde_json::json!({
			"controller": format!("pact-v{}", CONTROLLER_VERSION),
			"query_contract": [
				"continue the genuine current task under its binding constraints",
				"recover the next safe action and its exact required inputs",
				"distinguish established, tentative, failed, pending, and superseded state",
				"cite consequential completed-state claims with supplied block IDs"
			],
			"pinned_state": &self.pinned,
			"grounded_self_report": &self.grounded_hints,
			"evidence_packets": packets,
			"budgets": {
				"source_tokens": self.source_tokens,
				"target_tokens": self.target_tokens
			}
		}))
		.expect("PACT prompt view is serializable")
	}

	pub(crate) fn render_live_bands(
		&self,
		archive: Option<&super::archive::ArchiveBundle>,
	) -> (String, String) {
		if !self.enabled {
			let pinned =
				serde_json::to_string_pretty(&self.pinned).expect("pinned state is serializable");
			return (
				format!("<pinned_state format=\"json\">\n{pinned}\n</pinned_state>"),
				String::new(),
			);
		}
		let exact_packets: Vec<serde_json::Value> = self
			.packets
			.iter()
			.filter(|packet| packet.lane == Lane::KeepExact)
			.map(|packet| {
				serde_json::json!({
					"id": packet.id,
					"kind": packet.kind,
					"origin": packet.provenance,
					"depends_on": packet.depends_on,
					"content": packet.prompt_content,
				})
			})
			.collect();
		let mut recall_entries: Vec<serde_json::Value> = self
			.packets
			.iter()
			.filter(|packet| packet.lane != Lane::KeepExact)
			.map(|packet| {
				let location = archive
					.and_then(|bundle| bundle.entry(&packet.id))
					.map(|entry| {
						serde_json::json!({
							"archive": bundle_path(archive),
							"jsonl_lines": [entry.archive_line_start, entry.archive_line_end],
						})
					});
				serde_json::json!({
					"id": packet.id,
					"descriptor": packet.descriptor,
					"location": location,
				})
			})
			.collect();
		recall_entries.extend(self.prior_recall.iter().map(|(id, entry)| {
			serde_json::json!({
				"id": id,
				"descriptor": entry.descriptor,
				"location": {
					"archive": entry.archive_path.display().to_string(),
					"sidecar": entry.index_path.display().to_string(),
					"jsonl_lines": [entry.archive_line_start, entry.archive_line_end],
				},
			})
		}));

		let pinned =
			serde_json::to_string_pretty(&self.pinned).expect("pinned state is serializable");
		let frontier =
			serde_json::to_string_pretty(&exact_packets).expect("active frontier is serializable");
		let recall = serde_json::to_string_pretty(&serde_json::json!({
			"archive": bundle_path(archive),
			"sidecar": archive.map(|bundle| bundle.index_path.display().to_string()),
			"entries": recall_entries,
		}))
		.expect("recall index is serializable");
		(
			format!("<pinned_state format=\"json\">\n{pinned}\n</pinned_state>"),
			format!(
				"<active_frontier format=\"json\">\n{frontier}\n</active_frontier>\n\
<recall_index format=\"json\">\n{recall}\n</recall_index>"
			),
		)
	}

	/// Recompute runtime-owned governance from the still-live transcript. This
	/// catches any mutation between packet construction and commit instead of
	/// trusting model-authored fields or a stale controller snapshot.
	pub(crate) fn verify_governance(&self, messages: &[Message]) -> Result<()> {
		let task = crate::session::latest_real_user_task_content(messages)
			.unwrap_or_default()
			.trim()
			.to_string();
		let mut seen = BTreeSet::new();
		let constraints: Vec<PinnedItem> = messages
			.iter()
			.filter(|message| crate::session::is_real_user_task_message(message))
			.flat_map(|message| crate::supervisor::recite::extract_constraints(&message.content))
			.filter(|constraint| seen.insert(constraint.clone()))
			.map(|text| PinnedItem { text, source: None })
			.collect();
		let actual = governance_hash(messages, &task, &constraints);
		if actual != self.pinned.governance_hash {
			return Err(anyhow!(
				"PACT governance changed before commit (expected {}, got {})",
				self.pinned.governance_hash,
				actual
			));
		}
		Ok(())
	}

	/// Prove that every addressable packet resolves from the just-written
	/// sidecar to the byte-identical serialized messages that are about to be
	/// drained. Validation happens before removal, making optional compaction a
	/// transaction rather than a best-effort archive pointer.
	pub(crate) fn verify_archive(
		&self,
		archive: &super::archive::ArchiveBundle,
		source: &[Message],
	) -> Result<()> {
		if archive.entries.len() != self.packets.len()
			|| self
				.packets
				.iter()
				.any(|packet| archive.entry(&packet.id).is_none())
		{
			return Err(anyhow!("PACT archive sidecar does not cover every packet"));
		}

		let ids: Vec<String> = self
			.packets
			.iter()
			.map(|packet| packet.id.clone())
			.collect();
		let recovered = super::archive::read_blocks(&archive.index_path, &ids)?;
		let covered: BTreeSet<usize> = self
			.packets
			.iter()
			.flat_map(|packet| packet.message_start..=packet.message_end)
			.collect();
		let expected: Vec<&Message> = covered
			.into_iter()
			.map(|index| {
				source
					.get(index)
					.ok_or_else(|| anyhow!("PACT packet range points outside the archived drain"))
			})
			.collect::<Result<_>>()?;
		if recovered.len() != expected.len() {
			return Err(anyhow!(
				"PACT exact recall returned {} messages; expected {}",
				recovered.len(),
				expected.len()
			));
		}
		for (index, (actual, expected)) in recovered.iter().zip(expected).enumerate() {
			if serde_json::to_vec(actual)? != serde_json::to_vec(expected)? {
				return Err(anyhow!(
					"PACT exact recall differs from source at recovered message {index}"
				));
			}
		}
		Ok(())
	}

	pub(crate) fn normalize_summary(&self, summary: &mut CompressionSummary) {
		if !self.pinned.task.text.trim().is_empty() {
			summary.original_request = self.pinned.task.text.clone();
			summary.current_task = self.pinned.task.text.clone();
		}
	}

	pub(crate) fn validate_summary(
		&self,
		summary: &CompressionSummary,
	) -> Result<ValidationReport> {
		if summary.folded_units.len() > 40 {
			return Err(anyhow!("PACT summary exceeds the 40-unit fold bound"));
		}
		let selected_tokens = self
			.packets
			.iter()
			.filter(|packet| packet.lane != Lane::ArchiveReference)
			.map(|packet| crate::session::estimate_tokens(&packet.prompt_content))
			.sum::<usize>();
		if selected_tokens > self.target_tokens {
			return Err(anyhow!(
				"PACT selected evidence exceeds its token budget ({selected_tokens} > {})",
				self.target_tokens
			));
		}
		let packets_by_id: BTreeMap<&str, &EvidencePacket> = self
			.packets
			.iter()
			.map(|packet| (packet.id.as_str(), packet))
			.collect();
		for packet in self
			.packets
			.iter()
			.filter(|packet| packet.lane != Lane::ArchiveReference)
		{
			for dependency in &packet.depends_on {
				if packets_by_id
					.get(dependency.as_str())
					.is_some_and(|source| {
						source.provenance != Provenance::RealUser
							&& source.lane == Lane::ArchiveReference
					}) {
					return Err(anyhow!(
						"PACT selected packet {} is missing live dependency {}",
						packet.id,
						dependency
					));
				}
			}
		}
		let mut referenced = BTreeSet::new();
		for (index, unit) in summary.folded_units.iter().enumerate() {
			self.validate_folded_unit(index, unit)?;
			referenced.extend(unit.refs.iter().cloned());
		}
		Ok(ValidationReport {
			attribution_valid: true,
			fallback_reason: None,
			valid_units: summary.folded_units.len(),
			referenced_blocks: referenced.len(),
			governance_hash: self.pinned.governance_hash.clone(),
		})
	}

	pub(crate) fn sanitize_for_forced_compression(&self, summary: &mut CompressionSummary) {
		summary
			.folded_units
			.retain(|unit| self.validate_folded_unit(0, unit).is_ok());
		self.normalize_summary(summary);
	}

	fn validate_folded_unit(&self, index: usize, unit: &FoldedUnit) -> Result<()> {
		const ALLOWED_KINDS: &[&str] = &[
			"observation",
			"decision",
			"action",
			"outcome",
			"correction",
			"open_loop",
			"next_action",
			"reference",
			"synthesis",
		];
		const ALLOWED_STATUSES: &[&str] = &[
			"established",
			"tentative",
			"superseded",
			"failed",
			"pending",
			"unknown",
		];
		if unit.text.trim().is_empty() || unit.refs.is_empty() {
			return Err(anyhow!("PACT folded unit {index} has no text or support"));
		}
		if unit.text.chars().count() > 2_000 || unit.refs.len() > 16 {
			return Err(anyhow!(
				"PACT folded unit {index} exceeds its content or support bound"
			));
		}
		if unit.refs.iter().collect::<HashSet<_>>().len() != unit.refs.len() {
			return Err(anyhow!(
				"PACT folded unit {index} contains duplicate support IDs"
			));
		}
		if !ALLOWED_KINDS.contains(&unit.kind.as_str()) {
			return Err(anyhow!("PACT folded unit {index} has invalid kind"));
		}
		if !ALLOWED_STATUSES.contains(&unit.status.as_str()) {
			return Err(anyhow!("PACT folded unit {index} has invalid status"));
		}
		for source in &unit.refs {
			if !self.known_provenance.contains_key(source) {
				return Err(anyhow!(
					"PACT folded unit {index} cites unknown block {source}"
				));
			}
			if let Some(packet) = self.packets.iter().find(|packet| packet.id == *source) {
				if packet.lane == Lane::ArchiveReference {
					return Err(anyhow!(
						"PACT folded unit {index} cites archive-only descriptor {source} as evidence"
					));
				}
				if packet.lane == Lane::KeepExact
					&& matches!(
						unit.status.as_str(),
						"established" | "failed" | "superseded"
					) {
					return Err(anyhow!(
						"PACT folded unit {index} folds active-frontier packet {source} as completed state"
					));
				}
			} else if !self
				.packets
				.iter()
				.any(|packet| packet.prompt_content.contains(source))
			{
				return Err(anyhow!(
					"PACT folded unit {index} cites prior block {source} that was not visible to the compressor"
				));
			}
		}
		if unit.status == "established"
			&& unit.refs.iter().all(|source| {
				matches!(
					self.known_provenance.get(source),
					Some(Provenance::AssistantReported | Provenance::RuntimeSystemManaged)
				)
			}) {
			return Err(anyhow!(
				"PACT folded unit {index} amplifies assistant/runtime state to established"
			));
		}
		Ok(())
	}

	pub(crate) fn write_telemetry(
		&self,
		archive: &super::archive::ArchiveBundle,
		report: &ValidationReport,
		summary: &CompressionSummary,
		post_compression_tokens: u64,
	) -> Result<()> {
		let compression_id = archive
			.path
			.file_stem()
			.and_then(|value| value.to_str())
			.unwrap_or("unknown");
		self.write_telemetry_record(
			&archive.path.with_extension("pact.json"),
			compression_id,
			report,
			summary,
			post_compression_tokens,
			Some(archive),
			None,
		)
	}

	#[allow(clippy::too_many_arguments)]
	pub(crate) fn write_degraded_telemetry(
		&self,
		session_name: &str,
		compression_id: &str,
		report: &ValidationReport,
		summary: &CompressionSummary,
		post_compression_tokens: u64,
		fallback_reason: Option<&str>,
	) -> Result<()> {
		let dir = crate::directories::get_sessions_dir()?
			.join("archive")
			.join(session_name);
		std::fs::create_dir_all(&dir)
			.with_context(|| format!("failed to create PACT telemetry dir: {}", dir.display()))?;
		self.write_telemetry_record(
			&dir.join(format!("{compression_id}.pact.json")),
			compression_id,
			report,
			summary,
			post_compression_tokens,
			None,
			fallback_reason,
		)
	}

	#[allow(clippy::too_many_arguments)]
	fn write_telemetry_record(
		&self,
		path: &std::path::Path,
		compression_id: &str,
		report: &ValidationReport,
		summary: &CompressionSummary,
		post_compression_tokens: u64,
		archive: Option<&super::archive::ArchiveBundle>,
		fallback_reason: Option<&str>,
	) -> Result<()> {
		let packets: Vec<serde_json::Value> = self
			.packets
			.iter()
			.map(|packet| {
				serde_json::json!({
					"id": packet.id,
					"provenance": packet.provenance,
					"dependencies": packet.depends_on,
					"representation": packet.lane,
					"tokens": packet.tokens,
				})
			})
			.collect();
		let folded_units: Vec<serde_json::Value> = summary
			.folded_units
			.iter()
			.map(|unit| {
				serde_json::json!({
					"id": folded_unit_id(unit),
					"kind": unit.kind,
					"status": unit.status,
					"refs": unit.refs,
				})
			})
			.collect();
		let record = serde_json::json!({
			"compression_id": compression_id,
			"controller_version": CONTROLLER_VERSION,
			"source_tokens": self.source_tokens,
			"target_tokens": self.target_tokens,
			"selected_tokens": self.packets.iter()
				.filter(|packet| packet.lane != Lane::ArchiveReference)
				.map(|packet| crate::session::estimate_tokens(&packet.prompt_content))
				.sum::<usize>(),
			"post_compression_tokens": post_compression_tokens,
			"governance_hash": self.pinned.governance_hash,
			"pinned_block_ids": self.pinned.task.source.iter()
				.chain(self.pinned.constraints.iter().filter_map(|item| item.source.as_ref()))
				.collect::<Vec<_>>(),
			"packets": packets,
			"folded_units": folded_units,
			"grounded_self_report": self.grounded_hints,
			"prior_recall_ids": self.prior_recall.keys().collect::<Vec<_>>(),
			"validation": report,
			"archive_recovery_verified": archive.is_some(),
			"fallback_reason": fallback_reason,
			"archive": archive.map(|bundle| bundle.path.display().to_string()),
			"sidecar": archive.map(|bundle| bundle.index_path.display().to_string()),
		});
		std::fs::write(path, serde_json::to_vec_pretty(&record)?)
			.map_err(|error| anyhow!("failed to write PACT telemetry {}: {error}", path.display()))
	}
}

fn bundle_path(archive: Option<&super::archive::ArchiveBundle>) -> Option<String> {
	archive.map(|bundle| bundle.path.display().to_string())
}

pub(crate) fn folded_unit_id(unit: &FoldedUnit) -> String {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-fold-v1\0");
	let encoded = serde_json::to_vec(unit).expect("folded units are serializable");
	hasher.update(encoded);
	format!("s:{}", short_hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn message(role: &str, content: &str) -> Message {
		Message {
			role: role.to_string(),
			content: content.to_string(),
			..Default::default()
		}
	}

	fn packet(id: &str, provenance: Provenance, lane: Lane) -> EvidencePacket {
		EvidencePacket {
			id: id.to_string(),
			kind: PacketKind::ToolInteraction,
			provenance,
			message_start: 0,
			message_end: 0,
			depends_on: Vec::new(),
			tokens: 1,
			lane,
			prompt_content: "exact support".into(),
			descriptor: "test packet".into(),
		}
	}

	fn pact_with(packet: EvidencePacket) -> PactContext {
		let known_provenance = BTreeMap::from([(packet.id.clone(), packet.provenance)]);
		PactContext {
			enabled: true,
			packets: vec![packet],
			pinned: PinnedState {
				task: PinnedItem {
					text: "continue the task".into(),
					source: None,
				},
				constraints: Vec::new(),
				governance_hash: "hash".into(),
			},
			grounded_hints: Vec::new(),
			known_provenance,
			prior_recall: BTreeMap::new(),
			source_tokens: 1,
			target_tokens: 16,
		}
	}

	#[test]
	fn parallel_tool_calls_and_results_are_one_packet() {
		let mut assistant = message("assistant", "checking both sources");
		assistant.tool_calls = Some(serde_json::json!([
			{"id":"a","function":{"name":"one","arguments":"{}"}},
			{"id":"b","function":{"name":"two","arguments":"{}"}}
		]));
		let mut first = message("tool", "first result");
		first.tool_call_id = Some("a".into());
		let mut second = message("tool", "second result");
		second.tool_call_id = Some("b".into());
		let packets = build_packets("session", &[assistant, first, second]);
		assert_eq!(packets.len(), 1);
		assert_eq!(packets[0].kind, PacketKind::ToolInteraction);
		assert_eq!((packets[0].message_start, packets[0].message_end), (0, 2));
	}

	#[test]
	fn unresolved_structured_call_is_still_a_tool_interaction() {
		let mut assistant = message("assistant", "waiting for the call result");
		assistant.tool_calls = Some(serde_json::json!([
			{"id":"pending","function":{"name":"domain_tool","arguments":"{}"}}
		]));
		let packets = build_packets("session", &[assistant]);
		assert_eq!(packets[0].kind, PacketKind::ToolInteraction);
		assert_eq!(packets[0].provenance, Provenance::AssistantReported);
	}

	#[test]
	fn runtime_event_never_becomes_real_user_provenance() {
		let packets = build_packets(
			"session",
			&[
				message("user", "monitor the existing run"),
				message("assistant", "monitoring is active"),
				message("user", "<system-note>check now</system-note>"),
			],
		);
		assert_eq!(packets.last().unwrap().kind, PacketKind::RuntimeEvent);
		assert_eq!(
			packets.last().unwrap().provenance,
			Provenance::RuntimeSystemManaged
		);
	}

	#[test]
	fn stable_ids_depend_on_exact_packet_content() {
		let one = vec![message("assistant", "same")];
		let two = vec![message("assistant", "different")];
		assert_eq!(stable_packet_id("s", &one), stable_packet_id("s", &one));
		assert_ne!(stable_packet_id("s", &one), stable_packet_id("s", &two));
	}

	#[test]
	fn extractive_preview_keeps_exact_edges_and_line_numbers() {
		let source = (1..=200)
			.map(|line| format!("line {line}"))
			.collect::<Vec<_>>()
			.join("\n");
		let preview = extractive_edges(&source, 30);
		assert!(preview.contains("1| line 1"));
		assert!(preview.contains("200| line 200"));
		assert!(preview.contains("exact recall by block ID"));
		assert!(crate::session::estimate_tokens(&preview) <= 30);
	}

	#[test]
	fn validator_accepts_supported_fold_and_rejects_authority_or_lane_amplification() {
		let supported = pact_with(packet("b:tool", Provenance::ToolObserved, Lane::Summarize));
		let summary = CompressionSummary {
			folded_units: vec![FoldedUnit {
				text: "the observation completed".into(),
				kind: "outcome".into(),
				status: "established".into(),
				refs: vec!["b:tool".into()],
			}],
			..Default::default()
		};
		assert!(supported.validate_summary(&summary).is_ok());

		let runtime = pact_with(packet(
			"b:event",
			Provenance::RuntimeSystemManaged,
			Lane::Summarize,
		));
		let mut invalid = summary.clone();
		invalid.folded_units[0].refs = vec!["b:event".into()];
		assert!(runtime
			.validate_summary(&invalid)
			.unwrap_err()
			.to_string()
			.contains("amplifies"));

		let archive_only = pact_with(packet(
			"b:archived",
			Provenance::ToolObserved,
			Lane::ArchiveReference,
		));
		invalid.folded_units[0].refs = vec!["b:archived".into()];
		assert!(archive_only
			.validate_summary(&invalid)
			.unwrap_err()
			.to_string()
			.contains("archive-only"));

		let active = pact_with(packet(
			"b:active",
			Provenance::ToolObserved,
			Lane::KeepExact,
		));
		invalid.folded_units[0].refs = vec!["b:active".into()];
		assert!(active
			.validate_summary(&invalid)
			.unwrap_err()
			.to_string()
			.contains("active-frontier"));
	}

	#[test]
	fn self_report_grounding_emits_only_refs_not_reported_content() {
		let secret = "credential pointer vault/team/key with value ultra-secret-value";
		let messages = vec![message("assistant", secret)];
		let packets = build_packets("session", &messages);
		let handoff = crate::supervisor::detect::SelfReportHandoff {
			focus: String::new(),
			next: secret.into(),
			carry: Vec::new(),
		};
		let hints = ground_handoff(&handoff, &messages, &packets);
		let rendered = serde_json::to_string(&hints).unwrap();
		assert_eq!(hints.len(), 1);
		assert!(rendered.contains(&packets[0].id));
		assert!(!rendered.contains("ultra-secret-value"));
	}

	#[test]
	fn folded_unit_ids_are_stable_and_change_with_support() {
		let mut unit = FoldedUnit {
			text: "completed result".into(),
			kind: "outcome".into(),
			status: "established".into(),
			refs: vec!["b:one".into()],
		};
		let first = folded_unit_id(&unit);
		assert_eq!(first, folded_unit_id(&unit));
		unit.refs = vec!["b:two".into()];
		assert_ne!(first, folded_unit_id(&unit));
	}

	#[tokio::test]
	async fn active_frontier_allocation_obeys_total_token_budget() {
		let messages = vec![message(
			"assistant",
			&(1..=500)
				.map(|line| format!("exact line {line}"))
				.collect::<Vec<_>>()
				.join("\n"),
		)];
		let mut packets = build_packets("session", &messages);
		link_dependencies(&mut packets);
		let pinned = PinnedState {
			task: PinnedItem {
				text: "continue".into(),
				source: None,
			},
			constraints: Vec::new(),
			governance_hash: "hash".into(),
		};
		allocate_lanes(&mut packets, &messages, &pinned, &[], "", 30).await;
		let selected: usize = packets
			.iter()
			.filter(|packet| packet.lane != Lane::ArchiveReference)
			.map(|packet| crate::session::estimate_tokens(&packet.prompt_content))
			.sum();
		assert!(selected <= 30, "selected {selected} tokens");
		assert_eq!(packets[0].lane, Lane::KeepExact);
	}

	#[test]
	fn selected_packets_require_live_dependency_closure() {
		let dependency = packet(
			"b:dependency",
			Provenance::ToolObserved,
			Lane::ArchiveReference,
		);
		let mut child = packet("b:child", Provenance::AssistantReported, Lane::Summarize);
		child.depends_on = vec![dependency.id.clone()];
		let known_provenance = BTreeMap::from([
			(dependency.id.clone(), dependency.provenance),
			(child.id.clone(), child.provenance),
		]);
		let pact = PactContext {
			enabled: true,
			packets: vec![dependency, child],
			pinned: PinnedState {
				task: PinnedItem {
					text: "continue".into(),
					source: None,
				},
				constraints: Vec::new(),
				governance_hash: "hash".into(),
			},
			grounded_hints: Vec::new(),
			known_provenance,
			prior_recall: BTreeMap::new(),
			source_tokens: 2,
			target_tokens: 32,
		};
		let error = pact
			.validate_summary(&CompressionSummary {
				folded_units: vec![FoldedUnit {
					text: "pending checkpoint".into(),
					kind: "open_loop".into(),
					status: "pending".into(),
					refs: vec!["b:child".into()],
				}],
				..Default::default()
			})
			.unwrap_err();
		assert!(error.to_string().contains("missing live dependency"));
	}

	#[test]
	fn prior_summary_packet_strips_regenerated_file_context() {
		let mut prior = message(
			"assistant",
			"<conversation_summary id=\"old\">\n<progress>keep this</progress>\n<file_context>\nSECRET STALE FILE BYTES\n</file_context>\n</conversation_summary>",
		);
		prior.name = Some(super::super::apply::COMPRESSION_MESSAGE_NAME.into());
		let messages = vec![prior];
		let packets = build_packets("session", &messages);
		let rendered = render_packet(&messages, &packets[0], usize::MAX);
		assert!(rendered.contains("<progress>keep this</progress>"));
		assert!(!rendered.contains("SECRET STALE FILE BYTES"));
		assert!(!rendered.contains("<file_context>"));
	}

	#[test]
	fn repeated_compaction_keeps_visible_prior_block_recall_coordinates() {
		let mut pact = pact_with(packet(
			"b:current",
			Provenance::ValidatedSummary,
			Lane::Summarize,
		));
		pact.prior_recall.insert(
			"b:prior".into(),
			super::super::archive::ArchivedBlockRef {
				provenance: Provenance::ToolObserved,
				archive_path: "/tmp/prior.jsonl".into(),
				index_path: "/tmp/prior.blocks.jsonl".into(),
				archive_line_start: 7,
				archive_line_end: 9,
				descriptor: "prior exact tool packet".into(),
			},
		);
		let (_, recall) = pact.render_live_bands(None);
		assert!(recall.contains("b:prior"));
		assert!(recall.contains("/tmp/prior.jsonl"));
		assert!(recall.contains("7"));
		assert!(recall.contains("9"));
	}
}
