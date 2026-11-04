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
// It is an upper bound, not an allocation: the available half of the fold's
// residency target below usually limits exact material before this cap does.
pub(super) const MAX_FRONTIER_TOKENS: usize = 2_000;

pub(super) fn budget(messages: &[Message], target: usize) -> usize {
	// target_tokens prices evidence shown to the folder, not bytes to retain.
	// Apply the controller's existing deepest fold ratio to that evidence
	// target; otherwise exact material can consume nearly a firing threshold.
	let target = target / super::super::decision::MAX_COMPRESSION_RATIO as usize;
	messages
		.iter()
		.filter(|m| m.name.as_deref() == Some(super::super::apply::COMPRESSION_MESSAGE_NAME))
		.map(|m| crate::session::estimate_tokens(&m.content))
		.fold(target, usize::min)
}

pub(super) fn frontier_budget(live: usize, pinned: &PinnedState) -> usize {
	// Reserve two minimum fold units for facts/navigation while the model
	// runs. Rendering later charges their actual size before exact material.
	live.saturating_sub(crate::session::estimate_tokens(&render_pinned_lines(pinned, true)))
		.saturating_sub(2 * MIN_SUMMARIZE_RENDER_TOKENS)
		.min(MAX_FRONTIER_TOKENS)
}

fn attribute<'a>(unit: &'a str, name: &str) -> &'a str {
	unit.split(&format!("{name}=\""))
		.nth(1)
		.and_then(|s| s.split('"').next())
		.unwrap_or("")
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

fn unit_rank(unit: &str) -> (bool, bool, bool) {
	(
		attribute(unit, "kind") == "next_action" || attribute(unit, "kind") == "open_loop",
		matches!(attribute(unit, "status"), "pending" | "failed"),
		attribute(unit, "kind") == "correction",
	)
}

pub(super) fn render(
	pact: &PactContext,
	body: &str,
	file_context: &str,
	compression_id: &str,
	archive: Option<&super::super::archive::ArchiveBundle>,
	carried_request: Option<&str>,
) -> String {
	let (pinned, _) = pact.render_live_bands(None, carried_request, "");
	let wrapper = |sections: &str| {
		format!("<conversation_summary id=\"{compression_id}\" controller=\"pact-v{}\">\n{sections}\n</conversation_summary>", CONTROLLER_VERSION)
	};
	let guide = archive
		.map(|bundle| {
			format!(
				"index: {} (all block IDs, subjects and archive lines; use recall on an ID)\n",
				bundle.index_path.display()
			)
		})
		.unwrap_or_default();
	let frame = |folded: &str, frontier: &str, index: &str| {
		wrapper(&format!("{pinned}\n<folded_state>\n{folded}</folded_state>\n<active_frontier>\n{frontier}</active_frontier>\n<recall_index>\n{guide}{index}</recall_index>"))
	};
	let fixed = crate::session::estimate_tokens(&frame("", "", ""));
	let available = pact.live_tokens.saturating_sub(fixed);
	// Exact observations get half the discretionary space. Atomic facts and
	// concise navigation share the other half; whole units are evicted, never
	// cut into unsupported half-claims. The sidecar keeps every source findable.
	let mut state_budget = available / 4;
	let mut candidates = units(body);
	candidates.reverse();
	candidates.sort_by_key(|unit| std::cmp::Reverse(unit_rank(unit)));
	// Repeated folds must not halve away an already concise live action just
	// because the prior render left unused headroom. Preserve that whole atom
	// when it fits; optional old facts still compete in the smaller share.
	if let Some(unit) = candidates.iter().find(|unit| attribute(unit, "kind") != "reference") {
		let cost = crate::session::estimate_tokens(&compact_unit(unit));
		if cost <= available {
			state_budget = state_budget.max(cost);
		}
	}
	let mut folded = String::new();
	let mut seen = HashSet::new();
	for unit in candidates {
		if attribute(unit, "kind") == "reference" {
			continue;
		}
		let text = unit.split_once('>').map_or("", |(_, s)| s.trim_end_matches("</unit>"));
		if !seen.insert(text) || pinned.contains(text) {
			continue;
		}
		let candidate = format!("{folded}{}\n", compact_unit(unit));
		if crate::session::estimate_tokens(&candidate) <= state_budget {
			folded = candidate;
		}
	}
	let mut index = String::new();

	let frontier_budget = pact
		.live_tokens
		.saturating_sub(crate::session::estimate_tokens(&frame(&folded, "", &index)))
		.min(pact.frontier_tokens);
	let mut frontier = String::new();
	for part in working_set::live_fragments(&pact.packets, &pact.pinned) {
		let candidate = format!("{frontier}{part}\n");
		if crate::session::estimate_tokens(&candidate) <= frontier_budget
			&& crate::session::estimate_tokens(&frame(&folded, &candidate, &index)) <= pact.live_tokens
		{
			frontier = candidate;
		}
	}
	// Metadata yields to material already selected for the open step. The
	// full sidecar remains linked even if no individual entry fits.

	let mut entries: Vec<_> = pact.packets.iter().rev().collect();
	entries.sort_by_key(|packet| std::cmp::Reverse(folded.contains(&packet.id)));
	for packet in entries {
		let lines = archive
			.and_then(|a| a.entry(&packet.id))
			.map(|e| format!(" L{}-{}", e.archive_line_start, e.archive_line_end))
			.unwrap_or_default();
		let candidate = format!("{index}{}{lines} {}\n", packet.id, packet.descriptor.replace('\n', " "));
		if crate::session::estimate_tokens(&candidate) <= available / 4
			&& crate::session::estimate_tokens(&frame(&folded, &frontier, &candidate)) <= pact.live_tokens
		{
			index = candidate;
		}
	}
	for (id, entry) in &pact.prior_recall {
		if !folded.contains(id) {
			continue;
		}
		let candidate = format!(
			"{index}{id} {} L{}-{} {}\n",
			entry.archive_path.display(),
			entry.archive_line_start,
			entry.archive_line_end,
			entry.descriptor.replace('\n', " ")
		);
		if crate::session::estimate_tokens(&candidate) <= available / 4
			&& crate::session::estimate_tokens(&frame(&folded, &frontier, &candidate)) <= pact.live_tokens
		{
			index = candidate;
		}
	}
	let mut rendered = frame(&folded, &frontier, &index);
	if !file_context.is_empty() {
		let candidate = rendered.replace(
			"</conversation_summary>",
			&format!("<file_context>\n{file_context}\n</file_context>\n</conversation_summary>"),
		);
		if crate::session::estimate_tokens(&candidate) <= pact.live_tokens {
			rendered = candidate;
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
