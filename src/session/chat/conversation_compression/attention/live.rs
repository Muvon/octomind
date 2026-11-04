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

//! Bounded live residency, separate from the compressor's evidence budget.
use super::*;

// The cap curve saturates once the complete live-state envelope is enforced.
// It is an upper bound, not an allocation: the frontier share of live
// residency below usually limits exact material before this cap does.
pub(super) const MAX_FRONTIER_TOKENS: usize = 2_000;

/// A deep fold still keeps the working set: exact residency for every working
/// window of `working_set::select`, even when live residency is smaller.
pub(super) const MIN_FRONTIER_TOKENS: usize = working_set::WORKING_WINDOWS * MIN_SUMMARIZE_RENDER_TOKENS;

/// Share of live residency the folded units keep in the view. The exact
/// frontier gets the rest and the recall index what the frontier leaves.
/// Allocation and rendering split on this one ratio, so material admitted at
/// allocation time is not evicted again at render time.
const STATE_SHARE: usize = 4;

pub(super) fn budget(target: usize) -> usize {
	// target_tokens prices evidence shown to the folder, not bytes to retain.
	// Apply the controller's existing deepest fold ratio to that evidence
	// target; otherwise exact material can consume nearly a firing threshold.
	// The cap follows the fold target alone: bounding it by the previous
	// summary's size would let one small early fold pin every later one.
	target / super::super::decision::MAX_COMPRESSION_RATIO as usize
}

pub(super) fn frontier_budget(live: usize) -> usize {
	// Pinned state and the carried request are task context, not folded
	// evidence: they are never charged against live residency.
	(live - live / STATE_SHARE).clamp(MIN_FRONTIER_TOKENS, MAX_FRONTIER_TOKENS)
}

/// Wrapper attribute carrying the unit budget of the live projection, so the
/// request-time projection reproduces the fold-time selection exactly.
const STATE_ATTRIBUTE: &str = "state";
const FOLDED_OPEN: &str = "<folded_state>\n";
const FOLDED_CLOSE: &str = "</folded_state>";

fn attribute<'a>(unit: &'a str, name: &str) -> &'a str {
	unit.split(&format!("{name}=\""))
		.nth(1)
		.and_then(|s| s.split('"').next())
		.unwrap_or("")
}

/// `open` through `close` inclusive.
fn band<'a>(content: &'a str, open: &str, close: &str) -> Option<&'a str> {
	let start = content.find(open)?;
	let end = content[start..].find(close)? + start + close.len();
	Some(&content[start..end])
}

fn units(body: &str) -> Vec<&str> {
	body.match_indices("<unit ")
		.filter_map(|(start, _)| {
			body[start..]
				.find("</unit>")
				.map(|end| &body[start..start + end + "</unit>".len()])
		})
		.collect()
}

fn compact_unit(unit: &str) -> String {
	let id = attribute(unit, "id");
	unit.replacen(&format!(" id=\"{id}\""), "", 1)
}

fn unit_text(unit: &str) -> &str {
	unit.split_once('>').map_or("", |(_, s)| s.trim_end_matches("</unit>"))
}

/// Open work first, then failed or pending items, then corrections and
/// decisions: those are what keeps a session from re-litigating. Plain
/// observations and runtime reference units leave the live view first.
fn unit_rank(unit: &str) -> (bool, bool, bool) {
	let kind = attribute(unit, "kind");
	(
		kind == "next_action" || kind == "open_loop",
		matches!(attribute(unit, "status"), "pending" | "failed"),
		kind == "correction" || kind == "decision",
	)
}

/// Units in rank order (later units first within a rank), de-duplicated by
/// text, without those the pinned band already states. A stable sort keeps
/// the relative order of any subsequence, so the persisted subset projects to
/// the same sequence at request time as the full body did at fold time.
fn ranked_units<'a>(body: &'a str, pinned: &str) -> Vec<&'a str> {
	let mut candidates = units(body);
	candidates.reverse();
	candidates.sort_by_key(|unit| std::cmp::Reverse(unit_rank(unit)));
	let mut seen = HashSet::new();
	candidates
		.into_iter()
		.filter(|unit| {
			let text = unit_text(unit);
			seen.insert(text) && !pinned.contains(text)
		})
		.collect()
}

/// The live view of a folded state: whole units in rank order within
/// `state_budget` tokens, ids dropped. Whole units are evicted, never cut into
/// unsupported half-claims; an evicted unit stays in the persisted summary and
/// returns to the next fold's input.
fn projected_units(body: &str, pinned: &str, state_budget: usize) -> String {
	let mut folded = String::new();
	for unit in ranked_units(body, pinned) {
		if attribute(unit, "kind") == "reference" {
			continue;
		}
		let candidate = format!("{folded}{}\n", compact_unit(unit));
		if crate::session::estimate_tokens(&candidate) <= state_budget {
			folded = candidate;
		}
	}
	folded
}

/// Every surviving unit in its original order, ids kept for provenance and
/// reference units kept so their prior block IDs stay citable at the next fold.
fn persisted_units(body: &str, pinned: &str) -> String {
	let mut keep: HashSet<&str> = ranked_units(body, pinned).into_iter().collect();
	let retained: Vec<_> = units(body)
		.into_iter()
		.rev()
		// Consume each selected value once, keeping its latest occurrence as
		// ranking does. Keeping the first copy would reorder equal-rank units
		// on the next projection; membership alone would restore every copy.
		.filter(|unit| keep.remove(unit))
		.collect();
	retained.into_iter().rev().map(|unit| format!("{unit}\n")).collect()
}

/// The bounded view the provider receives for a summary written by [`render`]:
/// `None` for a summary without a projection budget (legacy render) or one
/// whose units already fit.
pub(crate) fn project(content: &str) -> Option<String> {
	let open_tag = &content[..content.find('>')?];
	let state_budget: usize = attribute(open_tag, STATE_ATTRIBUTE).parse().ok()?;
	let start = content.find(FOLDED_OPEN)? + FOLDED_OPEN.len();
	let end = content[start..].find(FOLDED_CLOSE)? + start;
	let pinned = band(content, "<pinned_state>", "</pinned_state>").unwrap_or("");
	let projected = projected_units(&content[start..end], pinned, state_budget);
	if projected == content[start..end] {
		return None;
	}
	Some(format!("{}{}{}", &content[..start], projected, &content[end..]))
}

/// The persisted summary: every folded unit, plus the frontier and recall
/// index sized so that the projected view ([`project`]) fits `live_tokens`.
pub(super) fn render(
	pact: &PactContext,
	body: &str,
	file_context: &str,
	compression_id: &str,
	archive: Option<&super::super::archive::ArchiveBundle>,
	carried_request: Option<&str>,
) -> String {
	let (pinned, _) = pact.render_live_bands(None, carried_request, "");
	let guide = archive
		.map(|bundle| {
			format!(
				"index: {} (all block IDs, subjects and archive lines; use recall on an ID)\n",
				bundle.index_path.display()
			)
		})
		.unwrap_or_default();
	let frame = |folded: &str, frontier: &str, index: &str, state: usize| {
		format!(
			"<conversation_summary id=\"{compression_id}\" controller=\"pact-v{}\" {STATE_ATTRIBUTE}=\"{state}\">\n{pinned}\n{FOLDED_OPEN}{folded}{FOLDED_CLOSE}\n<active_frontier>\n{frontier}</active_frontier>\n<recall_index>\n{guide}{index}</recall_index>\n</conversation_summary>",
			CONTROLLER_VERSION
		)
	};
	// `live_tokens` bounds the folded units, the exact frontier and the recall
	// index together; the wrapper and pinned bands are task context on top.
	let cap = pact.live_tokens + crate::session::estimate_tokens(&frame("", "", "", 0));
	let state_share = pact.live_tokens / STATE_SHARE;
	// Whole units are evicted from the view, never cut into unsupported
	// half-claims. The sidecar keeps every source findable.
	let mut state_budget = state_share;
	// Repeated folds must not halve away an already concise live action just
	// because the prior render left unused headroom. Preserve that whole atom
	// when it fits; optional old facts still compete in the smaller share.
	if let Some(unit) = ranked_units(body, &pinned)
		.into_iter()
		.find(|unit| attribute(unit, "kind") != "reference")
	{
		let cost = crate::session::estimate_tokens(&compact_unit(unit));
		if cost <= pact.live_tokens {
			state_budget = state_budget.max(cost);
		}
	}
	let projected = projected_units(body, &pinned, state_budget);
	let view = |frontier: &str, index: &str| {
		crate::session::estimate_tokens(&frame(&projected, frontier, index, state_budget))
	};

	// Allocation already sized the frontier against `frontier_tokens` with the
	// same state share, so the view exceeds the cap only by what the open
	// step's first unit took beyond that share. Parts are priced one by one,
	// as allocation priced them: a tokenizer boundary at a join must not
	// evict an admitted window.
	let mut frontier = String::new();
	let mut frontier_cost = 0;
	for part in working_set::live_fragments(&pact.packets, &pact.pinned) {
		let cost = crate::session::estimate_tokens(&part);
		if frontier_cost + cost <= pact.frontier_tokens {
			frontier.push_str(&part);
			frontier.push('\n');
			frontier_cost += cost;
		}
	}
	// Metadata yields to material already selected for the open step: blocks
	// the live units cite first, then blocks any persisted unit cites. The full
	// sidecar remains linked even if no individual entry fits.
	let mut index = String::new();
	let mut entries: Vec<_> = pact.packets.iter().rev().collect();
	entries.sort_by_key(|packet| std::cmp::Reverse((projected.contains(&packet.id), body.contains(&packet.id))));
	for packet in entries {
		let lines = archive
			.and_then(|a| a.entry(&packet.id))
			.map(|e| format!(" L{}-{}", e.archive_line_start, e.archive_line_end))
			.unwrap_or_default();
		let candidate = format!("{index}{}{lines} {}\n", packet.id, packet.descriptor.replace('\n', " "));
		if crate::session::estimate_tokens(&candidate) <= state_share && view(&frontier, &candidate) <= cap {
			index = candidate;
		}
	}
	for (id, entry) in &pact.prior_recall {
		if !body.contains(id) {
			continue;
		}
		let candidate = format!(
			"{index}{id} {} L{}-{} {}\n",
			entry.archive_path.display(),
			entry.archive_line_start,
			entry.archive_line_end,
			entry.descriptor.replace('\n', " ")
		);
		if crate::session::estimate_tokens(&candidate) <= state_share && view(&frontier, &candidate) <= cap {
			index = candidate;
		}
	}
	let mut rendered = frame(&persisted_units(body, &pinned), &frontier, &index, state_budget);
	if !file_context.is_empty() {
		let block = format!("<file_context>\n{file_context}\n</file_context>\n</conversation_summary>");
		let viewed = frame(&projected, &frontier, &index, state_budget).replace("</conversation_summary>", &block);
		if crate::session::estimate_tokens(&viewed) <= cap {
			rendered = rendered.replace("</conversation_summary>", &block);
		} else {
			crate::log_debug!(
				"Compression: dropped requested file context ({} tokens) — live state is capped at {} tokens",
				crate::session::estimate_tokens(file_context),
				pact.live_tokens
			);
		}
	}
	rendered
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
