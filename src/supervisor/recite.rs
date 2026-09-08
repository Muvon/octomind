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
//! stays in view. The same system-managed tail block projects runtime-owned
//! execution policy without letting compressed/model-authored state own it.
//! No model call: pure reuse of persisted state and the supervisor's existing
//! pre-request injection point.

use crate::session::anchor::Anchor;
use crate::supervisor::escape_xml_text as xml_text;
use crate::supervisor::VerificationPolicy;

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
/// line-numbered excerpts).
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
	for raw in task.lines() {
		let trimmed = raw.trim_start();
		if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
			in_fence = !in_fence;
			continue;
		}
		if in_fence || is_quoted_material(trimmed) {
			continue;
		}
		let starts_item = trimmed.starts_with(['-', '*', '•'])
			|| trimmed.split_once(['.', ')']).is_some_and(|(head, _)| {
				!head.is_empty() && head.chars().all(|c| c.is_ascii_digit())
			});
		if trimmed.is_empty() || starts_item {
			if !current.trim().is_empty() {
				lines.push(std::mem::take(&mut current));
			}
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
			|| trimmed.split_once(['.', ')']).is_some_and(|(head, _)| {
				!head.is_empty() && head.chars().all(|c| c.is_ascii_digit())
			});
		let len_max = if listed {
			LISTED_CONSTRAINT_LEN_MAX
		} else {
			CONSTRAINT_LEN_MAX
		};
		// Sentence-ish units: split lines on terminators so one long line
		// containing an instruction still yields just that instruction.
		for unit in line.split_inclusive(['.', '!', ';']) {
			let unit = unit
				.trim()
				.trim_start_matches(['-', '*', '•'])
				.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
				.trim();
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
			if MARKERS.iter().any(|m| lower.contains(m)) && !out.iter().any(|e| e == unit) {
				out.push(unit.to_string());
				if out.len() >= CONSTRAINTS_MAX {
					return out;
				}
			}
		}
	}
	out
}

/// Constraints that belong to the active turn, not a union of every historic
/// prohibition in the session. A validated follow-up resolution may fill an
/// omitted referent; otherwise the literal latest request is authoritative.
/// This keeps a revoked or superseded rule from being pinned forever while
/// preserving constraints that a genuine contextual follow-up still carries.
pub fn active_constraints(
	messages: &[crate::session::Message],
	resolved: Option<&crate::supervisor::resolve::ResolvedTask>,
) -> Vec<String> {
	let mut constraints = resolved
		.map(|task| task.resolved_request.as_str())
		.map(str::trim)
		.filter(|request| !request.is_empty())
		.or_else(|| crate::session::latest_real_user_task_content(messages))
		.map(extract_constraints)
		.unwrap_or_default();
	// Affirmative operational facts ("we work on the remote server", "I
	// deploy it myself") carry no negation marker, so the deterministic
	// extractor above can never catch them; the resolver captured them
	// verbatim at the only moment they are provably user-stated. They ride
	// the same recitation/pin channel under the same cap, after the negation
	// constraints — those are the ones models violate first.
	for fact in resolved
		.into_iter()
		.flat_map(|task| task.operational_constraints.iter())
	{
		let fact = fact.trim();
		if !fact.is_empty()
			&& constraints.len() < CONSTRAINTS_MAX
			&& !constraints.iter().any(|existing| existing == fact)
		{
			constraints.push(fact.to_string());
		}
	}
	constraints
}

/// Is this trimmed line quoted/pasted material rather than the user's own
/// words? True for blockquotes, transcript/UI decoration (box-drawing
/// characters), and line-number-prefixed excerpts like `185:Keys are revoked`.
fn is_quoted_material(trimmed: &str) -> bool {
	let Some(first) = trimmed.chars().next() else {
		return false;
	};
	if first == '>' || matches!(first, '│' | '╭' | '╰' | '├' | '└' | '┃' | '▸' | '┆' | '║')
	{
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

/// Build the recitation note for the context tail, or `None` when there is
/// nothing durable to recite yet.
///
/// Recites `intent` verbatim — it only moves when a compaction carries a
/// sanctioned user pivot, so it never drifts through paraphrase. For the
/// "what to do now" part it prefers the LIVE plan checklist
/// (`plan_checklist`, re-read every turn from runtime-owned storage) over the
/// `next_steps` snapshot, which only refreshes at compaction and is stale
/// between. With an active plan it recites even before the first compaction —
/// that is exactly when goal drift on a long task is most expensive.
/// `constraints` (see [`extract_constraints`]) recite the request's explicit
/// prohibitions verbatim — the instructions models abandon first as attention
/// decays — and fire from turn one: a constraint is most violated exactly
/// while the raw request is still in context but no longer attended to.
/// `verification_policy` is a read-only projection of runtime state; it is
/// explicitly labeled as an execution boundary so it cannot replace the task.
/// Wrapped in `<pay-attention>` so
/// [`crate::supervisor::gate::is_supervisor_injection`] excludes it from the
/// verify-gate's real-task search.
pub fn recite_note(
	anchor: &Anchor,
	plan_checklist: Option<&str>,
	constraints: &[String],
	current_task_sig: Option<u64>,
	verification_policy: VerificationPolicy,
) -> Option<String> {
	// A goal is only worth reciting while it still IS the goal. `intent` is
	// refreshed on compaction, but the user can ask for something else on any
	// turn — and a resumed session typically does so on a just-compacted context
	// that will not compact again for many turns. Reciting the previous request
	// at the tail then outranks the live one in the recency window, and the model
	// refuses the new ask as out of scope ("Goal (fixed)" is quoted back at the
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
	let intent = if goal_is_live {
		anchor.intent.as_str()
	} else {
		""
	};
	// next_steps were recorded against that goal, so they go stale with it.
	let next_steps: &[String] = if goal_is_live {
		&anchor.next_steps
	} else {
		&[]
	};
	if intent.is_empty()
		&& next_steps.is_empty()
		&& plan_checklist.is_none()
		&& constraints.is_empty()
		&& verification_policy == VerificationPolicy::Unspecified
	{
		return None;
	}
	let mut s = String::from("<pay-attention>\nRe-anchor on your task:\n");
	if !intent.is_empty() {
		s.push_str("Goal (fixed): <intent>");
		s.push_str(&xml_text(intent.trim()));
		s.push_str("</intent>\n");
	}
	// Prefer the live plan checklist (current every turn) over the stale
	// next_steps snapshot; fall back to next_steps only when no plan is active.
	if let Some(checklist) = plan_checklist.map(str::trim).filter(|c| !c.is_empty()) {
		s.push_str(&xml_text(checklist));
		s.push('\n');
	} else if !next_steps.is_empty() {
		s.push_str("Last-known next steps (may be stale — re-check against current state):\n");
		for step in next_steps {
			let step = step.trim();
			if !step.is_empty() {
				s.push_str("- ");
				s.push_str(&xml_text(step));
				s.push('\n');
			}
		}
	}
	if !constraints.is_empty() {
		s.push_str("Constraints from the request — verbatim, still binding; violating one voids the work:\n");
		for c in constraints {
			s.push_str("- ");
			s.push_str(&xml_text(c));
			s.push('\n');
		}
	}
	match verification_policy {
		VerificationPolicy::Forbidden => s.push_str(
			"Current execution boundary (not task scope): do not execute tests, builds, checks, validators, or other verification. Source inspection and requested edits may continue.\n",
		),
		VerificationPolicy::Allowed => s.push_str(
			"Standing user execution boundary (not task scope): the user has revoked the prior no-verification rule. Relevant verification is permitted, not required.\n",
		),
		VerificationPolicy::Unspecified => {}
	}
	s.push_str("</pay-attention>");
	Some(s)
}

#[cfg(test)]
#[path = "recite_tests.rs"]
mod tests;
