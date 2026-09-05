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
	TaskContinuation,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PacketLinkage {
	StructuredIds,
	ContiguousFallback,
	#[default]
	NotApplicable,
}

/// Inclusive line range in the canonical rendered packet. The digest covers
/// the exact UTF-8 bytes of those lines joined by `\n`, so archive validation
/// can prove that every fragment shown to the compressor is reconstructible
/// without trusting the compressor's wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceSpan {
	pub start_line: usize,
	pub end_line: usize,
	pub content_digest: String,
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
	pub linkage: PacketLinkage,
	pub tokens: usize,
	pub lane: Lane,
	/// Exact source fragments shown to the compressor. When bounded, omission
	/// markers name the original rendered line ranges; no facts are rewritten.
	pub prompt_content: String,
	pub exact_spans: Vec<SourceSpan>,
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
	pub verification_policy: crate::supervisor::VerificationPolicy,
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
	/// Live runtime-owned plan checklist at build time. Rendered inside
	/// <pinned_state> for the fold model only (the live context already gets
	/// the plan recited each turn by the supervisor), so a summary can never
	/// contradict the plan the model is re-anchored on after compaction.
	plan_focus: String,
	grounded_hints: Vec<GroundedHint>,
	known_provenance: BTreeMap<String, Provenance>,
	prior_recall: BTreeMap<String, super::archive::ArchivedBlockRef>,
	pub source_tokens: usize,
	pub target_tokens: usize,
	metrics: PactMetrics,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct PactMetrics {
	pub controller_and_model_latency_ms: u64,
	pub compression_api_time_ms: u64,
	pub compression_input_tokens: u64,
	pub compression_output_tokens: u64,
	pub compression_cost: f64,
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
	minimal_frontier: bool,
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

	let constraints = collect_constraints(session, task_source.as_deref());
	let verification_policy = session.session.info.verification_policy.effective(
		session
			.gate_task
			.as_ref()
			.is_some_and(|task| task.forbids_verification),
	);
	let governance_hash = governance_hash(
		&session.session.messages,
		&task_text,
		&constraints,
		verification_policy,
	);
	let pinned = PinnedState {
		task: PinnedItem {
			text: task_text,
			source: task_source,
		},
		constraints,
		verification_policy,
		governance_hash,
	};

	let source_tokens = packets.iter().map(|packet| packet.tokens).sum::<usize>();
	let target_tokens = ((source_tokens as f64) / target_ratio.max(1.0)).ceil() as usize;
	let grounded_hints = ground_self_report(session, drained, &packets);
	let mut plan_focus = crate::mcp::core::plan::core::get_current_plan_display()
		.await
		.unwrap_or_default();
	// The fold model must know when the plan predates the pinned task: aligned
	// folding is only correct for a live plan; a stale one is candidate state
	// the pinned task overrules.
	if let Some(marker) = crate::session::latest_task_timestamp(&session.session.messages)
		.and_then(crate::mcp::core::plan::plan_staleness_marker)
	{
		if !plan_focus.is_empty() {
			plan_focus.push('\n');
		}
		plan_focus.push_str(marker);
	}
	if attention_enabled {
		allocate_lanes(
			&mut packets,
			drained,
			&pinned,
			&grounded_hints,
			&plan_focus,
			target_tokens,
			minimal_frontier,
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
	// Carry forward only the prior IDs that the retained content still cites.
	// A prior summary packet embeds the previous cycle's own <recall_index>, so
	// testing raw content would re-match every historical ID and grow the live
	// index monotonically across compactions — the index itself becoming the
	// dominant term of the surviving context.
	let cited: Vec<String> = packets
		.iter()
		.map(|packet| super::knowledge::strip_recall_index(&packet.prompt_content))
		.collect();
	let prior_recall = registry
		.into_iter()
		.filter(|(id, _)| cited.iter().any(|content| content.contains(id)))
		.collect();

	Ok(PactContext {
		enabled: attention_enabled,
		packets,
		pinned,
		plan_focus,
		grounded_hints,
		known_provenance,
		prior_recall,
		source_tokens,
		target_tokens,
		metrics: PactMetrics::default(),
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
		let linkage = packet_linkage(slice, kind);
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
			linkage,
			tokens,
			lane: Lane::ArchiveReference,
			prompt_content: String::new(),
			exact_spans: Vec::new(),
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

fn packet_linkage(messages: &[Message], kind: PacketKind) -> PacketLinkage {
	if kind != PacketKind::ToolInteraction {
		return PacketLinkage::NotApplicable;
	}
	let Some(owner) = messages.first().filter(|message| has_tool_calls(message)) else {
		return PacketLinkage::ContiguousFallback;
	};
	let call_ids = tool_call_ids(owner);
	if call_ids.is_empty()
		|| messages.iter().skip(1).any(|message| {
			message.role == "tool"
				&& message
					.tool_call_id
					.as_deref()
					.is_none_or(|id| !call_ids.contains(id))
		}) {
		PacketLinkage::ContiguousFallback
	} else {
		PacketLinkage::StructuredIds
	}
}

fn classify_packet(messages: &[Message]) -> (PacketKind, Provenance) {
	let first = &messages[0];
	if first.role == "user" {
		if crate::session::continuation_task(&first.content).is_some() {
			return (PacketKind::TaskContinuation, Provenance::ValidatedSummary);
		}
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
	let mut latest_runtime_event: Option<String> = None;
	for index in 0..packets.len() {
		let mut dependencies = Vec::new();
		match packets[index].kind {
			PacketKind::UserTask | PacketKind::UserConstraintOrCorrection => {
				if let Some(task) = latest_task.as_ref() {
					dependencies.push(task.clone());
				}
				latest_task = Some(packets[index].id.clone());
				latest_runtime_event = None;
			}
			PacketKind::TaskContinuation => {
				if let Some(summary) = latest_summary.as_ref() {
					dependencies.push(summary.clone());
				}
				latest_task = Some(packets[index].id.clone());
				latest_runtime_event = None;
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
				latest_runtime_event = Some(packets[index].id.clone());
			}
			PacketKind::ToolInteraction => {
				if let Some(task) = latest_task.as_ref() {
					dependencies.push(task.clone());
				}
				if let Some(event) = latest_runtime_event.as_ref() {
					dependencies.push(event.clone());
				}
			}
			PacketKind::AssistantCheckpoint => {
				if index > 0 && packets[index - 1].kind == PacketKind::ToolInteraction {
					dependencies.push(packets[index - 1].id.clone());
				} else {
					if let Some(task) = latest_task.as_ref() {
						dependencies.push(task.clone());
					}
					if let Some(event) = latest_runtime_event.as_ref() {
						dependencies.push(event.clone());
					}
				}
			}
		}
		dependencies.sort();
		dependencies.dedup();
		packets[index].depends_on = dependencies;
	}
}

fn collect_constraints(session: &ChatSession, task_source: Option<&str>) -> Vec<PinnedItem> {
	let literal_task = crate::session::latest_real_user_task_content(&session.session.messages)
		.unwrap_or_default();
	crate::supervisor::recite::active_constraints(
		&session.session.messages,
		session.gate_task.as_ref(),
	)
	.into_iter()
	.map(|text| PinnedItem {
		// Cite the current real-user packet only when it contains the exact
		// constraint. A contextual resolver may legitimately carry a constraint
		// from its bounded evidence, but attributing that text to the literal
		// "continue" packet would be false provenance.
		source: task_source
			.filter(|_| literal_task.contains(&text))
			.map(str::to_string),
		text,
	})
	.collect()
}

fn governance_hash(
	messages: &[Message],
	task: &str,
	constraints: &[PinnedItem],
	verification_policy: crate::supervisor::VerificationPolicy,
) -> String {
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
	hasher.update([0]);
	hasher.update(verification_policy.as_str().as_bytes());
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

	let latest_real_user = packets
		.iter()
		.rposition(|packet| packet.provenance == Provenance::RealUser);
	let packet_texts: Vec<(usize, String, String)> = packets
		.iter()
		.enumerate()
		.map(|(index, packet)| {
			(
				index,
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
			.filter(|(index, id, content)| {
				latest_real_user.is_none_or(|latest| *index >= latest)
					&& (text.contains(id) || content.contains(&normalized))
			})
			.map(|(_, id, _)| id.clone())
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

/// Smallest preview budget for a summarize candidate. Below this the head/tail
/// extractor cannot fit even its omission marker, the render comes back with no
/// recoverable spans, and the packet (plus every closure containing it) is
/// silently dropped from the fold. At or above it, small packets render whole.
const MIN_SUMMARIZE_RENDER_TOKENS: usize = 64;

/// Text of the runtime-made reference unit that covers summarize packets the
/// folder did not cite. Deliberately a fixed pointer, not the packets'
/// descriptors: those already render per ID in `<recall_index>`.
const UNCITED_EVIDENCE_TEXT: &str = "Evidence not folded; recall by ref via <recall_index>.";

async fn allocate_lanes(
	packets: &mut [EvidencePacket],
	messages: &[Message],
	pinned: &PinnedState,
	grounded_hints: &[GroundedHint],
	plan_focus: &str,
	target_tokens: usize,
	minimal_frontier: bool,
) {
	if packets.is_empty() {
		return;
	}

	// /done marks a task-phase boundary: the next turn starts a NEW task, so
	// the heavy dependency-closure frontier of the finished one is noise, not
	// focus. The active task text survives in pinned_state; everything else
	// competes for the summarize lane or stays a recall pointer.
	let exact_ids = if minimal_frontier {
		HashSet::new()
	} else {
		active_dependency_closure(packets)
	};
	let mut exact_indices: Vec<usize> = packets
		.iter()
		.enumerate()
		.filter(|(_, packet)| exact_ids.contains(&packet.id))
		.map(|(index, _)| index)
		.collect();
	// Allocate the exact budget smallest-first: small closure members take only
	// what they need and the recomputed fair share rolls the surplus into the
	// largest packets, so truncation lands where head/tail extraction still
	// yields recoverable spans instead of starving a late packet to zero budget
	// (an empty-span KeepExact packet fails validation). Stable sort keeps ties
	// deterministic; output order is untouched — only budgets are assigned here.
	exact_indices.sort_by_key(|index| packets[*index].tokens);
	let mut exact_remaining = target_tokens;
	let mut exact_left = exact_indices.len();
	let mut used = 0usize;
	for index in exact_indices {
		let packet_budget = exact_remaining.div_ceil(exact_left);
		let rendered = render_packet_with_spans(messages, &packets[index], packet_budget);
		packets[index].lane = Lane::KeepExact;
		packets[index].prompt_content = rendered.content;
		packets[index].exact_spans = rendered.spans;
		let cost = crate::session::estimate_tokens(&packets[index].prompt_content);
		used = used.saturating_add(cost);
		exact_remaining = exact_remaining.saturating_sub(cost);
		exact_left -= 1;
	}

	// A prior summary is compression OUTPUT being drained this cycle: every
	// line is already-distilled state, so the fold model must see ALL of it —
	// any line it never saw silently vanishes from the session. Summarize
	// renders are fold-model INPUT only (render_live_bands emits just the
	// KeepExact packets into the live context), so the full render costs
	// compression-call tokens, not context budget: it is charged to neither
	// `used` nor `remaining`, and it must never pass through head/tail
	// extraction — that once deleted the middle 600 lines of a real session's
	// prior summary and the model rebuilt the task state wrong from the edges.
	for packet in packets.iter_mut() {
		if packet.kind != PacketKind::PriorSummary || packet.lane != Lane::ArchiveReference {
			continue;
		}
		let rendered = render_packet_with_spans(messages, packet, usize::MAX);
		if rendered.spans.is_empty() {
			continue;
		}
		packet.lane = Lane::Summarize;
		packet.prompt_content = rendered.content;
		packet.exact_spans = rendered.spans;
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
	// Render each candidate once at its desired budget: half its source size,
	// floored so a tiny packet renders whole. Below the floor the head/tail
	// extractor cannot even fit its omission marker and returns zero
	// recoverable spans — and one unrecoverable packet poisons every fold
	// closure depending on it, silently dropping the whole chain from the
	// fold. The map is reused by the ranked loop below whenever the
	// per-packet share covers the desired budget, so no candidate is
	// rendered twice.
	let previews: BTreeMap<usize, (PacketRender, usize)> = candidates
		.iter()
		.map(|index| {
			let budget = packets[*index]
				.tokens
				.div_ceil(2)
				.max(MIN_SUMMARIZE_RENDER_TOKENS);
			let rendered = render_packet_with_spans(messages, &packets[*index], budget);
			let cost = crate::session::estimate_tokens(&rendered.content);
			(*index, (rendered, cost))
		})
		.collect();
	let all_recoverable = previews
		.values()
		.all(|(rendered, _)| !rendered.spans.is_empty());
	let preview_total = previews
		.values()
		.map(|(_, cost)| *cost)
		.fold(0usize, usize::saturating_add);
	if all_recoverable && preview_total <= remaining {
		for (index, (rendered, cost)) in previews {
			packets[index].lane = Lane::Summarize;
			packets[index].prompt_content = rendered.content;
			packets[index].exact_spans = rendered.spans;
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
		let rendered: Vec<(usize, PacketRender, usize)> = pending
			.iter()
			.filter_map(|candidate| {
				let desired = packets[*candidate]
					.tokens
					.div_ceil(2)
					.max(MIN_SUMMARIZE_RENDER_TOKENS);
				// A preview rendered at `desired` is exact for any budget that
				// covers it; empty spans at `desired` stay empty at any smaller
				// budget, so a known-empty preview short-circuits the re-render.
				let (rendered, cost) = match previews.get(candidate) {
					Some((preview, _)) if preview.spans.is_empty() => return None,
					Some((preview, cost)) if per_packet >= desired => (preview.clone(), *cost),
					_ => {
						let rendered = render_packet_with_spans(
							messages,
							&packets[*candidate],
							desired.min(per_packet),
						);
						let cost = crate::session::estimate_tokens(&rendered.content);
						(rendered, cost)
					}
				};
				(!rendered.spans.is_empty()).then_some((*candidate, rendered, cost))
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
		for (candidate, rendered, _) in rendered {
			packets[candidate].lane = Lane::Summarize;
			packets[candidate].prompt_content = rendered.content;
			packets[candidate].exact_spans = rendered.spans;
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

fn active_dependency_closure(packets: &[EvidencePacket]) -> HashSet<String> {
	let Some(active) = packets.last() else {
		return HashSet::new();
	};
	if active.provenance == Provenance::RealUser || active.kind == PacketKind::PriorSummary {
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
				// duplicated into the exact frontier. A prior summary must never
				// be kept exact either: it is compression OUTPUT, so embedding it
				// verbatim nests summary inside summary and each fold then grows
				// by the size of the previous one until compaction frees nothing.
				// Its durable content re-folds through the summarize lane and
				// stays exactly recallable by block ID.
				if by_id.get(dependency.as_str()).is_some_and(|p| {
					matches!(p.provenance, Provenance::RealUser)
						|| p.kind == PacketKind::PriorSummary
				}) {
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
		sort_candidates(candidates, packets, grounded_refs, None);
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
	sort_candidates(candidates, packets, grounded_refs, scores.as_deref());
}

fn sort_candidates(
	candidates: &mut [usize],
	packets: &[EvidencePacket],
	grounded_refs: &HashSet<&str>,
	scores: Option<&[f32]>,
) {
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
		let relevance =
			scores.map(|values| values[right_position].total_cmp(&values[left_position]));
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
		PacketKind::TaskContinuation => 5,
		PacketKind::PriorSummary => 4,
		PacketKind::UserTask => 3,
		PacketKind::AssistantCheckpoint => 2,
		PacketKind::ToolInteraction => 1,
		PacketKind::RuntimeEvent => 0,
	}
}

fn render_packet(messages: &[Message], packet: &EvidencePacket, max_tokens: usize) -> String {
	render_packet_with_spans(messages, packet, max_tokens).content
}

#[derive(Debug, Clone)]
struct PacketRender {
	content: String,
	spans: Vec<SourceSpan>,
}

fn render_packet_with_spans(
	messages: &[Message],
	packet: &EvidencePacket,
	max_tokens: usize,
) -> PacketRender {
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
				if let Some(thinking) = crate::session::message_thinking_content(message) {
					rendered.push_str(&format!(
						"[MESSAGE {source} ASSISTANT THINKING]\n{thinking}\n"
					));
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
				if crate::session::continuation_task(&message.content).is_some() {
					"VALIDATED TASK CONTINUATION"
				} else if crate::session::is_real_user_task_message(message) {
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
		let lines: Vec<&str> = rendered.lines().collect();
		let spans = (!lines.is_empty())
			.then(|| source_span(&lines, 1, lines.len()))
			.into_iter()
			.collect();
		return PacketRender {
			content: rendered,
			spans,
		};
	}
	extractive_edges(&rendered, max_tokens)
}

fn extractive_edges(content: &str, max_tokens: usize) -> PacketRender {
	if max_tokens == 0 || content.is_empty() {
		return PacketRender {
			content: String::new(),
			spans: Vec::new(),
		};
	}
	let lines: Vec<&str> = content.lines().collect();
	if crate::session::estimate_tokens(content) <= max_tokens {
		return PacketRender {
			content: content.to_string(),
			spans: vec![source_span(&lines, 1, lines.len())],
		};
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
		let mut spans = Vec::new();
		if !head.is_empty() {
			spans.push(source_span(&lines, 1, head.len()));
		}
		if !tail.is_empty() {
			spans.push(source_span(
				&lines,
				lines.len() - tail.len() + 1,
				lines.len(),
			));
		}
		PacketRender {
			content: result,
			spans,
		}
	} else {
		PacketRender {
			content: crate::session::truncate_to_tokens(
				"[… exact packet omitted; recall by block ID …]",
				max_tokens,
			),
			spans: Vec::new(),
		}
	}
}

fn source_span(lines: &[&str], start_line: usize, end_line: usize) -> SourceSpan {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-source-span-v1\0");
	hasher.update(lines[start_line - 1..end_line].join("\n").as_bytes());
	SourceSpan {
		start_line,
		end_line,
		content_digest: short_hex(&hasher.finalize()),
	}
}

impl PactContext {
	pub(crate) fn record_metrics(&mut self, metrics: PactMetrics) {
		self.metrics = metrics;
	}

	pub(crate) fn prompt_view(&self) -> String {
		let mut out = String::new();
		out.push_str(&format!("controller: pact-v{}\n", CONTROLLER_VERSION));
		out.push_str(&format!(
			"budget: source_tokens={} target_tokens={}\n",
			self.source_tokens, self.target_tokens
		));
		out.push_str("<pinned_state>\n");
		out.push_str(&render_pinned_lines(&self.pinned));
		if !self.plan_focus.trim().is_empty() {
			out.push_str(&format!("live_plan:\n{}\n", self.plan_focus.trim()));
		}
		out.push_str("</pinned_state>\n");
		if !self.grounded_hints.is_empty() {
			out.push_str("<grounded_self_report>\n");
			for hint in &self.grounded_hints {
				out.push_str(&format!("{}: {}\n", hint.kind, hint.refs.join(" ")));
			}
			out.push_str("</grounded_self_report>\n");
		}
		out.push_str("<packets>\n");
		for packet in &self.packets {
			out.push_str(&packet_header(packet));
			if packet.lane == Lane::ArchiveReference {
				out.push_str(&format!("descriptor: {}\n", packet.descriptor));
			} else {
				out.push_str(packet.prompt_content.trim_end());
				out.push('\n');
			}
		}
		out.push_str("</packets>");
		out
	}

	pub(crate) fn render_live_bands(
		&self,
		archive: Option<&super::archive::ArchiveBundle>,
	) -> (String, String) {
		let pinned_band = format!(
			"<pinned_state>\n{}</pinned_state>",
			render_pinned_lines(&self.pinned)
		);
		if !self.enabled {
			return (pinned_band, String::new());
		}
		let mut frontier = String::new();
		for packet in self
			.packets
			.iter()
			.filter(|packet| packet.lane == Lane::KeepExact)
		{
			frontier.push_str(&packet_header(packet));
			frontier.push_str(packet.prompt_content.trim_end());
			frontier.push('\n');
		}
		let mut recall = String::new();
		if let Some(path) = bundle_path(archive) {
			recall.push_str(&format!("archive: {path}\n"));
		}
		if let Some(bundle) = archive {
			recall.push_str(&format!("sidecar: {}\n", bundle.index_path.display()));
		}
		for packet in self
			.packets
			.iter()
			.filter(|packet| packet.lane != Lane::KeepExact)
		{
			let lines = archive
				.and_then(|bundle| bundle.entry(&packet.id))
				.map(|entry| format!(" L{}-{}", entry.archive_line_start, entry.archive_line_end))
				.unwrap_or_default();
			recall.push_str(&format!("{}{} — {}\n", packet.id, lines, packet.descriptor));
		}
		for (id, entry) in &self.prior_recall {
			recall.push_str(&format!(
				"{} {} L{}-{} — {}\n",
				id,
				entry.archive_path.display(),
				entry.archive_line_start,
				entry.archive_line_end,
				entry.descriptor
			));
		}
		let frontier_band = if frontier.is_empty() {
			String::new()
		} else {
			format!("<active_frontier>\n{frontier}</active_frontier>\n")
		};
		(
			pinned_band,
			format!("{frontier_band}<recall_index>\n{recall}</recall_index>"),
		)
	}

	/// Recompute runtime-owned governance from the still-live transcript. This
	/// catches any mutation between packet construction and commit instead of
	/// trusting model-authored fields or a stale controller snapshot.
	pub(crate) fn verify_governance(&self, session: &ChatSession) -> Result<()> {
		let messages = &session.session.messages;
		let task = crate::session::latest_real_user_task_content(messages)
			.unwrap_or_default()
			.trim()
			.to_string();
		let constraints = collect_constraints(session, None);
		let actual = governance_hash(
			messages,
			&task,
			&constraints,
			session.session.info.verification_policy.effective(
				session
					.gate_task
					.as_ref()
					.is_some_and(|task| task.forbids_verification),
			),
		);
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
		for packet in &self.packets {
			let canonical = render_packet(source, packet, usize::MAX);
			let lines: Vec<&str> = canonical.lines().collect();
			for span in &packet.exact_spans {
				if span.start_line == 0
					|| span.start_line > span.end_line
					|| span.end_line > lines.len()
					|| source_span(&lines, span.start_line, span.end_line) != *span
				{
					return Err(anyhow!(
						"PACT exact span failed archive reconstruction for packet {} lines {}-{}",
						packet.id,
						span.start_line,
						span.end_line
					));
				}
			}
		}
		Ok(())
	}

	pub(crate) fn normalize_summary(&self, summary: &mut CompressionSummary) {
		if !self.pinned.task.text.trim().is_empty() {
			summary.original_request = self.pinned.task.text.clone();
			summary.current_task = self.pinned.task.text.clone();
		}
		// A runtime advisory or the assistant's own checkpoint may steer the
		// current turn, but neither may become the durable task to resume after
		// compaction. Keep next actions only when at least one cited source has
		// user, observed-tool, or already-validated-summary authority. This runs
		// even when the optional full validator is disabled.
		summary.folded_units.retain(|unit| {
			unit.kind != "next_action" || self.has_authoritative_continuation_support(unit)
		});
	}

	fn has_authoritative_continuation_support(&self, unit: &FoldedUnit) -> bool {
		unit.refs.iter().any(|source| {
			let authoritative = matches!(
				self.known_provenance.get(source),
				Some(
					Provenance::RealUser | Provenance::ToolObserved | Provenance::ValidatedSummary
				)
			);
			if !authoritative {
				return false;
			}
			match self.packets.iter().find(|packet| packet.id == *source) {
				Some(packet) => packet.lane != Lane::ArchiveReference,
				None => self
					.packets
					.iter()
					.any(|packet| packet.prompt_content.contains(source.as_str())),
			}
		})
	}

	/// Deterministic repair of model-authored folded units before validation.
	///
	/// The generative fold is already paid for; when it violates the
	/// attribution contract in mechanical ways (citing a recall descriptor,
	/// folding live frontier state as completed, skipping a summarize packet)
	/// the runtime can fix the violation without inventing content. Anything
	/// repair cannot save is dropped, and `validate_summary` remains the
	/// strict final gate.
	pub(crate) fn repair_summary(&self, summary: &mut CompressionSummary) {
		let lane_of = |id: &str| {
			self.packets
				.iter()
				.find(|packet| packet.id == id)
				.map(|packet| packet.lane)
		};
		for unit in summary.folded_units.iter_mut() {
			let mut seen = HashSet::new();
			unit.refs.retain(|source| seen.insert(source.clone()));
			// Strip refs the validator can never accept: unknown blocks,
			// archive-only descriptors (recall pointers, not evidence), and
			// prior IDs that were not visible to the compressor.
			unit.refs.retain(|source| {
				if !self.known_provenance.contains_key(source) {
					return false;
				}
				match lane_of(source) {
					Some(Lane::ArchiveReference) => false,
					Some(_) => true,
					None => self
						.packets
						.iter()
						.any(|packet| packet.prompt_content.contains(source.as_str())),
				}
			});
			unit.refs.truncate(16);
			if unit.text.chars().count() > 2_000 {
				unit.text = unit.text.chars().take(2_000).collect();
			}
			// Active-frontier packets are live state; folding them as
			// completed is lane amplification — downgrade instead of reject.
			if matches!(
				unit.status.as_str(),
				"established" | "failed" | "superseded"
			) && unit
				.refs
				.iter()
				.any(|source| lane_of(source) == Some(Lane::KeepExact))
			{
				unit.status = "tentative".into();
			}
			// Authority amplification: assistant/runtime-only support cannot
			// carry an established claim.
			if unit.status == "established"
				&& !unit.refs.is_empty()
				&& unit.refs.iter().all(|source| {
					matches!(
						self.known_provenance.get(source),
						Some(Provenance::AssistantReported | Provenance::RuntimeSystemManaged)
					)
				}) {
				unit.status = "tentative".into();
			}
		}
		// Drop what per-unit repair could not save (empty text/refs, invalid
		// kind or status, unrecoverable prior sources).
		summary.folded_units.retain(|unit| {
			if self.validate_folded_unit(0, unit).is_err() {
				return false;
			}
			let referenced: BTreeSet<String> = unit.refs.iter().cloned().collect();
			self.verify_prior_references(&referenced).is_ok()
		});
		// Coverage: every summarize-lane packet must be represented by a
		// folded unit. Represent the ones the model skipped with reference
		// units so their recall coordinates survive the drain. The unit carries
		// only its refs: the per-packet descriptor already renders verbatim in
		// <recall_index>, and repeating it here was ~180 tokens of duplicated
		// filler per unit that rode every request until the next fold (measured:
		// 1.5k such units / 272k tokens across 288 archived sessions).
		let referenced: HashSet<&str> = summary
			.folded_units
			.iter()
			.flat_map(|unit| unit.refs.iter().map(String::as_str))
			.collect();
		let uncovered: Vec<&EvidencePacket> = self
			.packets
			.iter()
			.filter(|packet| {
				packet.lane == Lane::Summarize && !referenced.contains(packet.id.as_str())
			})
			.collect();
		for chunk in uncovered.chunks(16) {
			if summary.folded_units.len() >= 40 {
				break;
			}
			summary.folded_units.push(FoldedUnit {
				text: UNCITED_EVIDENCE_TEXT.into(),
				kind: "reference".into(),
				status: "unknown".into(),
				refs: chunk.iter().map(|packet| packet.id.clone()).collect(),
			});
		}
	}

	pub(crate) fn validate_summary(
		&self,
		summary: &CompressionSummary,
	) -> Result<ValidationReport> {
		if summary.folded_units.len() > 40 {
			return Err(anyhow!("PACT summary exceeds the 40-unit fold bound"));
		}
		// The prior summary folds at FULL fidelity by design (see
		// allocate_lanes): its render is fold-model input that never reaches
		// the live context — the folded units replacing it are bounded by the
		// 40-unit fold cap. Counting it here would veto exactly the
		// compressions that carry the most prior state.
		let selected_tokens = self
			.packets
			.iter()
			.filter(|packet| {
				packet.lane != Lane::ArchiveReference && packet.kind != PacketKind::PriorSummary
			})
			.map(|packet| crate::session::estimate_tokens(&packet.prompt_content))
			.sum::<usize>();
		if selected_tokens > self.target_tokens {
			return Err(anyhow!(
				"PACT selected evidence exceeds its token budget ({selected_tokens} > {})",
				self.target_tokens
			));
		}
		if let Some(packet) = self
			.packets
			.iter()
			.find(|packet| packet.lane == Lane::KeepExact && packet.exact_spans.is_empty())
		{
			return Err(anyhow!(
				"PACT exact-frontier packet has no recoverable source span: {}",
				packet.id
			));
		}
		// A prior summary selected for folding must be its COMPLETE render:
		// exactly one span from line 1 over every rendered line, digest-exact
		// against the content. Head/tail extraction leaves two edge spans and
		// omission markers, and post-render truncation breaks the digest —
		// either way the cycle is vetoed instead of silently folding from a
		// gutted summary. This is the runtime backstop for the allocate_lanes
		// full-render guarantee: any future budget reintroduced on that path
		// trips here instead of deleting distilled session state.
		for packet in self.packets.iter().filter(|packet| {
			packet.kind == PacketKind::PriorSummary && packet.lane == Lane::Summarize
		}) {
			let lines: Vec<&str> = packet.prompt_content.lines().collect();
			let complete = !lines.is_empty()
				&& packet.exact_spans.len() == 1
				&& packet.exact_spans[0] == source_span(&lines, 1, lines.len());
			if !complete {
				return Err(anyhow!(
					"PACT prior summary {} was not folded from its complete render",
					packet.id
				));
			}
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
				// An archive-lane prior summary is not a missing live dependency:
				// it is deliberately never kept exact (see active_dependency_closure)
				// and it normally re-folds through the summarize lane at full
				// fidelity — it stays archived only when its render is empty,
				// where its content is still recallable by block ID. Rejecting
				// here would veto that legitimate case.
				if packets_by_id
					.get(dependency.as_str())
					.is_some_and(|source| {
						source.kind != PacketKind::PriorSummary
							&& source.provenance != Provenance::RealUser
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
		self.verify_prior_references(&referenced)?;
		for packet in self
			.packets
			.iter()
			.filter(|packet| packet.lane == Lane::Summarize)
		{
			if !referenced.contains(&packet.id) {
				return Err(anyhow!(
					"PACT selected summarize packet has no folded unit: {}",
					packet.id
				));
			}
		}
		Ok(ValidationReport {
			attribution_valid: true,
			fallback_reason: None,
			valid_units: summary.folded_units.len(),
			referenced_blocks: referenced.len(),
			governance_hash: self.pinned.governance_hash.clone(),
		})
	}

	fn verify_prior_references(&self, referenced: &BTreeSet<String>) -> Result<()> {
		let current: HashSet<&str> = self
			.packets
			.iter()
			.map(|packet| packet.id.as_str())
			.collect();
		let mut by_sidecar: BTreeMap<&std::path::Path, Vec<String>> = BTreeMap::new();
		for id in referenced {
			if current.contains(id.as_str()) {
				continue;
			}
			let entry = self.prior_recall.get(id).ok_or_else(|| {
				anyhow!("PACT prior source {id} has no visible archive coordinate")
			})?;
			by_sidecar
				.entry(entry.index_path.as_path())
				.or_default()
				.push(id.clone());
		}
		for (sidecar, ids) in by_sidecar {
			super::archive::read_blocks(sidecar, &ids).with_context(|| {
				format!(
					"PACT prior-source recovery failed for {}",
					sidecar.display()
				)
			})?;
		}
		Ok(())
	}

	pub(crate) fn sanitize_for_forced_compression(&self, summary: &mut CompressionSummary) {
		summary.folded_units.retain(|unit| {
			if self.validate_folded_unit(0, unit).is_err() {
				return false;
			}
			let referenced = unit.refs.iter().cloned().collect::<BTreeSet<_>>();
			self.verify_prior_references(&referenced).is_ok()
		});
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
		if unit.kind == "next_action" && !self.has_authoritative_continuation_support(unit) {
			return Err(anyhow!(
				"PACT folded unit {index} promotes assistant/runtime state to a continuation action"
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
				let archive_location =
					archive
						.and_then(|bundle| bundle.entry(&packet.id))
						.map(|entry| {
							serde_json::json!({
								"archive": bundle_path(archive),
								"sidecar": archive.map(|bundle| bundle.index_path.display().to_string()),
								"jsonl_lines": [entry.archive_line_start, entry.archive_line_end],
							})
						});
				serde_json::json!({
					"id": packet.id,
					"provenance": packet.provenance,
					"dependencies": packet.depends_on,
					"linkage": packet.linkage,
					"representation": packet.lane,
					"tokens": packet.tokens,
					"exact_spans": packet.exact_spans,
					"archive_location": archive_location,
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
			"metrics": self.metrics,
			"governance_hash": self.pinned.governance_hash,
			"plan_focus": &self.plan_focus,
			"pinned_block_ids": self.pinned.task.source.iter()
				.chain(self.pinned.constraints.iter().filter_map(|item| item.source.as_ref()))
				.collect::<Vec<_>>(),
			"packets": packets,
			"folded_units": folded_units,
			"grounded_self_report": self.grounded_hints,
			"prior_recall_ids": self.prior_recall.keys().collect::<Vec<_>>(),
			"validation": report,
			"archive_recovery_verified": archive.is_some(),
			"exact_span_recovery_verified": archive.is_some(),
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

/// One-line, model-facing packet header: stable ID plus the attribution
/// metadata the compressor's citation rules need. Runtime-only fields
/// (exact_spans digests, linkage) are deliberately not rendered — they cost
/// tokens without informing the fold.
fn packet_header(packet: &EvidencePacket) -> String {
	let lane = match packet.lane {
		Lane::KeepExact => "keep_exact",
		Lane::Summarize => "summarize",
		Lane::ArchiveReference => "archive_reference",
	};
	let deps = if packet.depends_on.is_empty() {
		String::new()
	} else {
		format!(" deps={}", packet.depends_on.join(","))
	};
	format!(
		"[{} {} kind={:?} origin={:?}{}]\n",
		packet.id, lane, packet.kind, packet.provenance, deps
	)
}

/// Compact plain-line rendering of pinned state — replaces the pretty-JSON
/// serialization that stayed in the model's context every turn.
fn render_pinned_lines(pinned: &PinnedState) -> String {
	let mut out = String::new();
	let source = pinned
		.task
		.source
		.as_deref()
		.map(|id| format!(" (source: {id})"))
		.unwrap_or_default();
	out.push_str(&format!("task{source}: {}\n", pinned.task.text));
	for constraint in &pinned.constraints {
		let source = constraint
			.source
			.as_deref()
			.map(|id| format!(" (source: {id})"))
			.unwrap_or_default();
		out.push_str(&format!("constraint{source}: {}\n", constraint.text));
	}
	match pinned.verification_policy {
		crate::supervisor::VerificationPolicy::Forbidden => out.push_str(
			"verification_policy: forbidden for this turn; do not execute verification\n",
		),
		crate::supervisor::VerificationPolicy::Allowed => out.push_str(
			"verification_policy: allowed; any prior no-verification rule is revoked; verification is permitted, not required\n",
		),
		crate::supervisor::VerificationPolicy::Unspecified => {}
	}
	out.push_str(&format!("governance_hash: {}\n", pinned.governance_hash));
	out
}

pub(crate) fn folded_unit_id(unit: &FoldedUnit) -> String {
	let mut hasher = Sha256::new();
	hasher.update(b"octomind-pact-fold-v1\0");
	let encoded = serde_json::to_vec(unit).expect("folded units are serializable");
	hasher.update(encoded);
	format!("s:{}", short_hex(&hasher.finalize()))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
