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

//! Goal and execution-boundary recitation for long-horizon drift.
//!
//! The session `Anchor` (intent + next_steps) survives every compaction, but it
//! is only rendered inside the mid-transcript compressed-knowledge message,
//! where the model attends to it weakly. Once a session has compacted at least
//! once (anchor non-empty), we re-emit a tiny goal block at the context TAIL
//! each turn — the recency slot the model attends to most — so the live goal
//! stays in view. The same system-managed tail block carries the texts the
//! plugins pinned, verbatim.
//! No model call: pure reuse of persisted state and octomind's pre-request
//! injection point.

use crate::session::anchor::Anchor;

/// Cap on recited constraints — precision over recall; a wall of lines would
/// dilute the recency slot this block exists to exploit.
const CONSTRAINTS_MAX: usize = 8;
/// A genuine instruction is short; a long sentence merely *containing* a
/// negation is almost always descriptive prose, not a directive.
const CONSTRAINT_LEN_MAX: usize = 200;
/// A bullet is the requester marking a line AS a requirement, so its length says
/// nothing about whether it is one — an acceptance criterion routinely spells out
/// the condition, the exception and the reason in one sentence. Prose keeps the
/// strict cap; only explicitly listed items get the longer one.
const LISTED_CONSTRAINT_LEN_MAX: usize = 400;
/// Opening of every recitation; also how the previous one is found in history.
const RECITE_HEADER: &str = "<pay-attention>\nRe-anchor on your task:\n";
/// Closing of every recitation.
const RECITE_CLOSER: &str = "</pay-attention>";
/// Context the model reads after an unchanged recitation before it is repeated.
/// History is append-only (rewriting the earlier copy would void the cached
/// prefix), so an identical copy every turn only stacks duplicates; once this
/// much has landed on top, the copy has left the recency slot and earns a
/// fresh one at the tail.
const RECITE_REFRESH_TOKENS: usize = 8_000;

/// Deterministically extract the request's binding requirements — both explicit
/// prohibitions ("do not X", "never Y", "must not Z") and explicit acceptance
/// criteria ("must remain W", "preserve V"). These are the instructions models
/// violate mid-task as prompt attention decays, so they get re-recited at the
/// context tail verbatim. Domain-agnostic: matches directive phrasing, not any
/// particular subject. High precision by construction: unit must be a short
/// non-question sentence/line containing a strong imperative.
///
/// Prohibitions alone are not the whole contract. A request that spells out the
/// behaviours an accepted fix must exhibit is stating requirements just as
/// binding as its "do not"s, and reciting only the negatives under a heading
/// that claims to list what "voids the work" hands the model an authoritative
/// but partial spec — it then satisfies the recited subset and stops.
/// Rejoin a request into the lines its author meant, dropping the material that
/// is never a directive (fenced code, blockquotes, transcript decoration,
/// line-numbered excerpts, client-supplied `<untrusted>` data blocks).
///
/// Requests are hard-wrapped, so a single criterion routinely spans several
/// physical lines. Reading them one at a time truncates it at the wrap — a rule
/// arrives as "preserve the tab's remaining" and loses the part that says what
/// to preserve it as. A new logical line starts at a list marker or a blank
/// line; anything else continues the current one.
fn logical_lines(task: &str) -> Vec<String> {
	let mut lines: Vec<String> = Vec::new();
	let mut current = String::new();
	let mut in_fence = false;
	let mut in_untrusted = false;
	let mut in_item = false;
	for raw in task.lines() {
		let trimmed = raw.trim_start();
		if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
			in_fence = !in_fence;
			continue;
		}
		// A client wraps runtime state it attaches to the user's words (browser
		// tabs, page titles) in `<untrusted>`: data, never the requester's rule.
		// It also breaks the logical line, or the closing tag's neighbour would be
		// glued onto it.
		if !in_fence && (in_untrusted || trimmed.starts_with("<untrusted")) {
			in_untrusted = !trimmed.contains("</untrusted>");
			if !current.trim().is_empty() {
				lines.push(std::mem::take(&mut current));
			}
			current.clear();
			in_item = false;
			continue;
		}
		if in_fence || is_quoted_material(trimmed) {
			continue;
		}
		let starts_item = trimmed.starts_with(['-', '*', '•'])
			|| trimmed
				.split_once(['.', ')'])
				.is_some_and(|(head, _)| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()));
		// A list item's continuation is indented under it. Unindented text after
		// an item is new prose, not part of the item — joining it would carry the
		// item's laxer length budget onto a paragraph that never earned it.
		let continues_item = in_item && raw.starts_with([' ', '\t']);
		if trimmed.is_empty() || starts_item || (in_item && !continues_item) {
			if !current.trim().is_empty() {
				lines.push(std::mem::take(&mut current));
			}
			in_item = starts_item;
			if trimmed.is_empty() {
				continue;
			}
			current = raw.to_string();
			continue;
		}
		if current.is_empty() {
			current = raw.to_string();
		} else {
			current.push(' ');
			current.push_str(trimmed);
		}
	}
	if !current.trim().is_empty() {
		lines.push(current);
	}
	lines
}

/// Split a line into sentence-ish units. A `.` between digits is a decimal point,
/// not a terminator: splitting "below 8.2 must stay as it is" mid-version left the
/// requirement recited as a subjectless "must stay as it is" — an unanchored
/// directive in the slot the agent trusts most. A terminator inside an inline code
/// span belongs to the code: splitting at the `.` in `0.1.2-pre.0` recited a rule
/// as "excludes `0.1.2-pre." and dropped the clause after it.
fn sentence_units(line: &str) -> Vec<&str> {
	let bytes = line.as_bytes();
	let mut units = Vec::new();
	let mut start = 0;
	let mut in_code = false;
	for (idx, ch) in line.char_indices() {
		if ch == '`' {
			in_code = !in_code;
			continue;
		}
		if in_code || !matches!(ch, '.' | '!' | ';') {
			continue;
		}
		if ch == '.' {
			let prev = bytes[..idx].last().is_some_and(u8::is_ascii_digit);
			let next = bytes.get(idx + 1).is_some_and(|byte| byte.is_ascii_digit());
			if prev && next {
				continue;
			}
		}
		let end = idx + ch.len_utf8();
		units.push(&line[start..end]);
		start = end;
	}
	if start < line.len() {
		units.push(&line[start..]);
	}
	units
}

pub fn extract_constraints(task: &str) -> Vec<String> {
	// "must" is the normative keyword requesters actually use (RFC 2119), so it
	// covers the prohibitions ("must not") and the acceptance criteria ("must
	// remain", "must be", "must still") in one marker rather than a growing list
	// of phrasings. "preserve" is the common imperative that carries no "must".
	const MARKERS: [&str; 7] = [
		"do not ",
		"don't ",
		"never ",
		"must ",
		"not allowed",
		"forbidden",
		"preserve ",
	];
	let mut out: Vec<String> = Vec::new();
	for line in logical_lines(task) {
		let line = line.as_str();
		let trimmed = line.trim_start();
		let listed = trimmed.starts_with(['-', '*', '•'])
			|| trimmed
				.split_once(['.', ')'])
				.is_some_and(|(head, _)| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()));
		let len_max = if listed {
			LISTED_CONSTRAINT_LEN_MAX
		} else {
			CONSTRAINT_LEN_MAX
		};
		// Sentence-ish units: split lines on terminators so one long line
		// containing an instruction still yields just that instruction.
		for (index, unit) in sentence_units(line).into_iter().enumerate() {
			let unit = unit
				.trim()
				.trim_start_matches(['-', '*', '•'])
				.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
				.trim();
			// Reciting under "violating one voids the work" is an assertion that the
			// text IS the requirement. A unit after the first that opens mid-sentence
			// means the split landed inside one, so what survives is a clause whose
			// subject is elsewhere — the request said what must not change, the block
			// would say only "must stay exactly as it is today". Stay silent instead:
			// no constraint beats a binding-looking one that names nothing.
			if index > 0 && unit.starts_with(|c: char| c.is_lowercase()) {
				continue;
			}
			if unit.is_empty() || unit.len() > len_max || unit.ends_with('?') {
				continue;
			}
			// Units are cut at line ends as well as sentence ends, so a wrapped
			// sentence can yield a stub ("Preserve the"). One that stops without
			// a terminator has to carry a whole clause to be worth a slot.
			if !unit.ends_with(['.', '!', ';']) && unit.split_whitespace().count() < 4 {
				continue;
			}
			// A unit opening with a quote character is cited material, not an
			// instruction the user issued.
			if unit.starts_with(['"', '\'', '“', '”', '‘', '`']) {
				continue;
			}
			let lower = unit.to_lowercase();
			if MARKERS.iter().any(|m| contains_marker(&lower, m)) && !out.iter().any(|e| e == unit) {
				out.push(unit.to_string());
				if out.len() >= CONSTRAINTS_MAX {
					return out;
				}
			}
		}
	}
	out
}

/// Markers match as whole words: a bare substring test read "whenever" as
/// "never" and recited a client's plain reminder as the user's binding rule.
/// A marker ending in a space already bounds its right edge.
fn contains_marker(lower: &str, marker: &str) -> bool {
	lower.match_indices(marker).any(|(at, _)| {
		let left_bounded = !lower[..at].chars().next_back().is_some_and(char::is_alphanumeric);
		let right_bounded = marker.ends_with(' ')
			|| !lower[at + marker.len()..]
				.chars()
				.next()
				.is_some_and(char::is_alphanumeric);
		left_bounded && right_bounded
	})
}

/// Constraints that belong to the active turn, not a union of every historic
/// prohibition in the session: the literal latest request is authoritative.
/// This keeps a revoked or superseded rule from being pinned forever.
pub fn active_constraints(messages: &[crate::session::Message]) -> Vec<String> {
	crate::session::latest_real_user_task_content(messages)
		.map(extract_constraints)
		.unwrap_or_default()
}

/// Is this trimmed line quoted/pasted material rather than the user's own
/// words? True for blockquotes, transcript/UI decoration (box-drawing
/// characters), and line-number-prefixed excerpts like `185:Keys are revoked`.
fn is_quoted_material(trimmed: &str) -> bool {
	let Some(first) = trimmed.chars().next() else {
		return false;
	};
	if first == '>' || matches!(first, '│' | '╭' | '╰' | '├' | '└' | '┃' | '▸' | '┆' | '║') {
		return true;
	}
	// A markdown heading names a section or restates a symptom ("# Code blocks
	// sometimes don't render first character"); it is never the directive the
	// requester issued, so reciting it as binding crowds out the real ones.
	if first == '#' {
		return true;
	}
	// `NN:` line-number prefix — a pasted code/file excerpt.
	let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
	digits > 0 && trimmed[digits..].starts_with(':')
}

/// The anchor's goal and next steps, or empty ones when the goal is no longer
/// the live request.
fn live_goal(anchor: &Anchor, current_task_sig: Option<u64>) -> (&str, &[String]) {
	// A goal is only worth reciting while it still IS the goal. `intent` is
	// refreshed on compaction, but the user can ask for something else on any
	// turn — and a resumed session typically does so on a just-compacted context
	// that will not compact again for many turns. Reciting the previous request
	// at the tail then outranks the live one in the recency window, and the model
	// refuses the new ask as out of scope (the goal line, once labelled "fixed", is quoted back at the
	// user). The newest user message must always win.
	let goal_is_live = match (anchor.intent_task_sig, current_task_sig) {
		// Unsigned intent: provenance unknown, so we cannot claim it is the live
		// goal. Fail SAFE — skip the recitation rather than risk asserting a
		// superseded one as "fixed". This is deliberately the strict default: the
		// bug recurred once because a third writer set `intent` without signing it,
		// and treating unsigned as current made that silently wrong again. The only
		// cost is a session carried over from an older binary losing recitation
		// until its next compaction re-signs the goal.
		(0, _) => false,
		(sig, Some(current)) => sig == current,
		// No user turn resolvable this turn: nothing to contradict.
		(_, None) => true,
	};
	let intent = if goal_is_live { anchor.intent.as_str() } else { "" };
	// next_steps were recorded against that goal, so they go stale with it.
	let next_steps: &[String] = if goal_is_live { &anchor.next_steps } else { &[] };
	(intent, next_steps)
}

/// Escape untrusted text before embedding it inside XML-like control blocks.
/// This preserves field boundaries against literal closing tags.
fn escape_xml_text(value: &str) -> String {
	value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The goal line and the "what to do now" part of the note.
fn push_goal_and_plan(s: &mut String, intent: &str, next_steps: &[String], plan_checklist: Option<&str>) {
	if !intent.is_empty() {
		s.push_str("Goal (the current request governs where it differs): <intent>");
		s.push_str(&escape_xml_text(intent.trim()));
		s.push_str("</intent>\n");
	}
	// Prefer the live plan checklist (current every turn) over the stale
	// next_steps snapshot; fall back to next_steps only when no plan is active.
	if let Some(checklist) = plan_checklist.map(str::trim).filter(|c| !c.is_empty()) {
		s.push_str(&escape_xml_text(checklist));
		s.push('\n');
	} else if !next_steps.is_empty() {
		s.push_str("Last-known next steps (may be stale — re-check against current state):\n");
		for step in next_steps {
			let step = step.trim();
			if !step.is_empty() {
				s.push_str("- ");
				s.push_str(&escape_xml_text(step));
				s.push('\n');
			}
		}
	}
}

/// The recitation note: octomind writes the header, the goal, the plan and
/// the closer; the plugins' pinned texts sit between, verbatim and in the
/// order the plugin sent them.
pub fn recite_note_with_pins(
	anchor: &Anchor,
	plan_checklist: Option<&str>,
	current_task_sig: Option<u64>,
	pins: &[crate::plugins::PluginBlock],
) -> Option<String> {
	let (intent, next_steps) = live_goal(anchor, current_task_sig);
	if intent.is_empty()
		&& next_steps.is_empty()
		&& plan_checklist.is_none()
		&& pins.iter().all(|pin| pin.text.is_empty())
	{
		return None;
	}
	let mut s = String::from(RECITE_HEADER);
	push_goal_and_plan(&mut s, intent, next_steps, plan_checklist);
	for pin in pins {
		s.push_str(&pin.text);
	}
	s.push_str(RECITE_CLOSER);
	Some(s)
}

/// When an unchanged recitation may be repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecitalRefresh {
	/// Turn entry: an unchanged copy returns once [`RECITE_REFRESH_TOKENS`] of
	/// context have buried it.
	ByVolume,
	/// Inside the tool loop: only a changed note, or one compaction removed, lands.
	/// Refreshing by volume there stacks a full copy every few calls (measured: 33
	/// copies of a ~1.1k-token note on one long run, ~13% extra context), while a
	/// state change (step done, new condition) is exactly when drift starts.
	OnChangeOnly,
}

/// Whether `note` should be appended now: it differs from the latest
/// recitation in history, none is left (compaction dropped it), or — for
/// [`RecitalRefresh::ByVolume`] — more than [`RECITE_REFRESH_TOKENS`] of context
/// has landed since that copy.
pub fn needs_recital(messages: &[crate::session::Message], note: &str, refresh: RecitalRefresh) -> bool {
	let Some(last) = messages
		.iter()
		.rposition(|message| message.role == "user" && message.content.starts_with(RECITE_HEADER))
	else {
		return true;
	};
	if messages[last].content != note {
		return true;
	}
	if refresh == RecitalRefresh::OnChangeOnly {
		return false;
	}
	// Whole messages: tool-call arguments are context the model reads too, and in a
	// tool-heavy stretch they are most of what buries the previous copy.
	let since: usize = messages[last + 1..]
		.iter()
		.map(crate::session::estimate_message_tokens)
		.sum();
	since > RECITE_REFRESH_TOKENS
}

/// Append the goal and execution-boundary recitation before a provider request.
/// Called at turn entry and before every tool-loop follow-up: most requests of a
/// long turn come from the tool loop and never pass through the turn-entry
/// executor. Once the session has compacted, the durable goal lives only in the
/// mid-transcript summary, where attention is weak; this re-emits a tiny goal
/// block at the tail, in the recency window, BEFORE the cache-marker advance so
/// the cached prefix stays intact.
pub fn inject_before_request(
	chat_session: &mut crate::session::chat::session::ChatSession,
	config: &crate::config::Config,
	refresh: RecitalRefresh,
) -> anyhow::Result<()> {
	// Without a plugin nothing is pinned and nothing is recited.
	if config.has_plugins() {
		return inject_pinned_recital(chat_session, refresh);
	}
	Ok(())
}

/// `inject_before_request` for a session with plugins: the sections between
/// the plan and the closer are their pins. The note includes the pin texts,
/// so a changed pin makes `needs_recital` see a changed note.
fn inject_pinned_recital(
	chat_session: &mut crate::session::chat::session::ChatSession,
	refresh: RecitalRefresh,
) -> anyhow::Result<()> {
	let plan_checklist = crate::mcp::core::plan::render_plan_checklist_with_staleness(
		crate::session::latest_task_timestamp(&chat_session.session.messages),
	);
	let live_task = crate::session::latest_real_user_task_content(&chat_session.session.messages);
	let pins = crate::plugins::pinned_blocks(&chat_session.session.info.name);
	let Some(note) = recite_note_with_pins(
		&chat_session.session.info.anchor,
		plan_checklist.as_deref(),
		live_task.map(crate::session::anchor::task_sig),
		&pins,
	) else {
		return Ok(());
	};
	let waiting = waiting_on_background_jobs(
		&chat_session.session.messages,
		crate::session::shell_jobs::has_pending(),
	);
	if !waiting && needs_recital(&chat_session.session.messages, &note, refresh) {
		chat_session.add_system_managed_user_message(&note)?;
		crate::log_debug!("Plugin recitation injected ({refresh:?})");
	}
	Ok(())
}

/// Whether the model ended its last reply to wait for background jobs that are
/// still running. Such a turn is re-entered by job status or completion
/// messages, not by new work, and a recitation landing there reads as a binding
/// instruction to keep going: the model abandons the wait to resurvey sources or
/// rerun checks. Recitation resumes once the jobs have settled.
pub fn waiting_on_background_jobs(messages: &[crate::session::Message], jobs_pending: bool) -> bool {
	jobs_pending
		&& messages
			.iter()
			.rev()
			.find(|message| message.role == "assistant" || crate::session::is_real_user_task_message(message))
			.is_some_and(|message| {
				message.role == "assistant"
					&& message
						.tool_calls
						.as_ref()
						.and_then(serde_json::Value::as_array)
						.is_none_or(Vec::is_empty)
			})
}

#[cfg(test)]
#[path = "recite_tests.rs"]
mod tests;
