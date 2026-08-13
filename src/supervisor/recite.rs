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

//! Goal recitation — the most-cited fix for long-horizon goal drift.
//!
//! The session `Anchor` (intent + next_steps) survives every compaction, but it
//! is only rendered inside the mid-transcript compressed-knowledge message,
//! where the model attends to it weakly. Once a session has compacted at least
//! once (anchor non-empty), we re-emit a tiny goal block at the context TAIL
//! each turn — the recency slot the model attends to most — so the live goal
//! stays in view. No model call, no new schema: pure reuse of the existing
//! `Anchor` and the supervisor's pre-request injection point.

use crate::session::anchor::Anchor;

/// Cap on recited constraints — precision over recall; a wall of lines would
/// dilute the recency slot this block exists to exploit.
const CONSTRAINTS_MAX: usize = 8;
/// A genuine instruction is short; a long sentence merely *containing* a
/// negation is almost always descriptive prose, not a directive.
const CONSTRAINT_LEN_MAX: usize = 200;

/// Deterministically extract explicit prohibitions from the user's request —
/// "do not X", "never Y", "must not Z". These are the instructions models
/// violate mid-task as prompt attention decays, so they get re-recited at the
/// context tail verbatim. Domain-agnostic: matches directive phrasing, not any
/// particular subject. High precision by construction: unit must be a short
/// non-question sentence/line containing a strong negative imperative.
pub fn extract_constraints(task: &str) -> Vec<String> {
	const MARKERS: [&str; 6] = [
		"do not ",
		"don't ",
		"never ",
		"must not ",
		"not allowed",
		"forbidden",
	];
	let mut out: Vec<String> = Vec::new();
	let mut in_fence = false;
	for line in task.lines() {
		// Quoted material is not a directive: skip fenced code blocks,
		// blockquotes, transcript decoration (box-drawing UI captures), and
		// line-numbered excerpts ("185:Keys are revoked, never deleted…") —
		// pasted CONTENT that merely contains a negation must never be
		// recited back as a binding constraint.
		let trimmed = line.trim_start();
		if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
			in_fence = !in_fence;
			continue;
		}
		if in_fence || is_quoted_material(trimmed) {
			continue;
		}
		// Sentence-ish units: split lines on terminators so one long line
		// containing an instruction still yields just that instruction.
		for unit in line.split_inclusive(['.', '!', ';']) {
			let unit = unit
				.trim()
				.trim_start_matches(['-', '*', '•'])
				.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')')
				.trim();
			if unit.is_empty() || unit.len() > CONSTRAINT_LEN_MAX || unit.ends_with('?') {
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
/// (`plan_checklist`, re-read every turn from the plan tool's storage) over the
/// `next_steps` snapshot, which only refreshes at compaction and is stale
/// between. With an active plan it recites even before the first compaction —
/// that is exactly when goal drift on a long task is most expensive.
/// `constraints` (see [`extract_constraints`]) recite the request's explicit
/// prohibitions verbatim — the instructions models abandon first as attention
/// decays — and fire from turn one: a constraint is most violated exactly
/// while the raw request is still in context but no longer attended to.
/// Wrapped in `<pay-attention>` so
/// [`crate::supervisor::gate::is_supervisor_injection`] excludes it from the
/// verify-gate's real-task search.
pub fn recite_note(
	anchor: &Anchor,
	plan_checklist: Option<&str>,
	constraints: &[String],
	current_task_sig: Option<u64>,
) -> Option<String> {
	// A goal is only worth reciting while it still IS the goal. `intent` is
	// refreshed on compaction, but the user can ask for something else on any
	// turn — and a resumed session typically does so on a just-compacted context
	// that will not compact again for many turns. Reciting the previous request
	// at the tail then outranks the live one in the recency window, and the model
	// refuses the new ask as out of scope ("Goal (fixed)" is quoted back at the
	// user). The newest user message must always win.
	let goal_is_live = match (anchor.intent_task_sig, current_task_sig) {
		// Legacy anchor with no signature — recite exactly as before.
		(0, _) => true,
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
	{
		return None;
	}
	let mut s = String::from("<pay-attention>\nRe-anchor on your task:\n");
	if !intent.is_empty() {
		s.push_str("Goal (fixed): <intent>");
		s.push_str(intent.trim());
		s.push_str("</intent>\n");
	}
	// Prefer the live plan checklist (current every turn) over the stale
	// next_steps snapshot; fall back to next_steps only when no plan is active.
	if let Some(checklist) = plan_checklist.map(str::trim).filter(|c| !c.is_empty()) {
		s.push_str(checklist);
		s.push('\n');
	} else if !next_steps.is_empty() {
		s.push_str("Last-known next steps (may be stale — re-check against current state):\n");
		for step in next_steps {
			let step = step.trim();
			if !step.is_empty() {
				s.push_str("- ");
				s.push_str(step);
				s.push('\n');
			}
		}
	}
	if !constraints.is_empty() {
		s.push_str("Constraints from the request — verbatim, still binding; violating one voids the work:\n");
		for c in constraints {
			s.push_str("- ");
			s.push_str(c);
			s.push('\n');
		}
	}
	s.push_str("</pay-attention>");
	Some(s)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::anchor::AnchorUpdate;

	#[test]
	fn empty_anchor_recites_nothing() {
		assert!(recite_note(&Anchor::default(), None, &[], None).is_none());
	}

	#[test]
	fn constraints_extracted_high_precision() {
		let task = "System:\nYou are an engineer. Resolve the issue with the minimal change. \
Do NOT modify tests. When done, stop.\n\nInstruction:\nThe parser crashes on empty input. \
It never worked for unicode?\n- don't touch the CI config\n1. Never commit directly.\n\
This long descriptive sentence merely mentions that the value must not exceed the buffer size when the flag is set and goes on and on about internals of the allocation strategy across multiple clauses to exceed the length cap entirely and then some more.";
		let c = extract_constraints(task);
		assert!(c.iter().any(|x| x == "Do NOT modify tests."), "{c:?}");
		assert!(c.iter().any(|x| x == "don't touch the CI config"), "{c:?}");
		assert!(c.iter().any(|x| x == "Never commit directly."), "{c:?}");
		// Questions and over-long prose are excluded.
		assert!(!c.iter().any(|x| x.contains("unicode")), "{c:?}");
		assert!(!c.iter().any(|x| x.contains("buffer size")), "{c:?}");
	}

	#[test]
	fn constraints_skip_quoted_and_pasted_material() {
		// The two observed FPs: transcript-decorated line-numbered content and
		// a quoted phrase inside prose — both merely CONTAIN a negation.
		let task = "Update the post per the log below. Do not touch translations.\n\
│ 185:Keys are revoked, never deleted — usage records survive.\n\
> never deleted — usage records survive\n\
185:Keys are revoked, never deleted.\n\
\"rate windows\" the original post referenced but never defined.\n\
```\ndo not run this fenced example\n```\n\
~~~\nnever run this either\n~~~";
		let c = extract_constraints(task);
		assert_eq!(c, vec!["Do not touch translations."], "{c:?}");
	}

	#[test]
	fn constraints_deduped_and_capped() {
		let line = "Do not push.\n".repeat(20);
		let c = extract_constraints(&line);
		assert_eq!(c, vec!["Do not push."]);
		let many: String = (0..20).map(|i| format!("Never touch file{i}.\n")).collect();
		assert_eq!(extract_constraints(&many).len(), 8);
	}

	#[test]
	fn constraints_recite_without_anchor_or_plan() {
		let cs = vec!["Do NOT modify tests.".to_string()];
		let note =
			recite_note(&Anchor::default(), None, &cs, None).expect("constraints recite alone");
		assert!(crate::supervisor::gate::is_supervisor_injection(&note));
		assert!(note.contains("- Do NOT modify tests."));
		assert!(note.contains("still binding"));
	}

	#[test]
	fn recites_intent_and_next_steps() {
		let mut a = Anchor::default();
		a.extend(
			AnchorUpdate {
				intent: Some("Add the truncation detector".to_string()),
				next_steps: vec!["wire it into response.rs".to_string()],
				..Default::default()
			},
			0,
		);
		let note = recite_note(&a, None, &[], None).expect("should recite");
		// Excluded from the gate's real-task search.
		assert!(crate::supervisor::gate::is_supervisor_injection(&note));
		assert!(note.contains("<intent>Add the truncation detector</intent>"));
		assert!(note.contains("- wire it into response.rs"));
	}

	#[test]
	fn recites_intent_only_when_no_next_steps() {
		let mut a = Anchor::default();
		a.extend(
			AnchorUpdate {
				intent: Some("Refactor auth".to_string()),
				..Default::default()
			},
			0,
		);
		let note = recite_note(&a, None, &[], None).expect("should recite");
		assert!(note.contains("<intent>Refactor auth</intent>"));
		assert!(!note.contains("Last-known next steps"));
	}

	#[test]
	fn live_plan_checklist_preferred_over_stale_next_steps() {
		let mut a = Anchor::default();
		a.extend(
			AnchorUpdate {
				intent: Some("Ship the feature".to_string()),
				next_steps: vec!["STALE step".to_string()],
				..Default::default()
			},
			0,
		);
		let note = recite_note(
			&a,
			Some("Live plan (1/2 done):\n✅ done it\n🔄 do this ← current"),
			&[],
			None,
		)
		.expect("should recite");
		assert!(note.contains("<intent>Ship the feature</intent>"));
		assert!(note.contains("🔄 do this ← current"));
		// The live checklist replaces the stale snapshot entirely.
		assert!(!note.contains("STALE step"));
		assert!(!note.contains("Last-known next steps"));
	}

	#[test]
	fn goal_from_a_superseded_request_is_not_recited() {
		// The failure this guards: a session compacts while working request A, the
		// user then asks for B, and B's turn does not compact (the context was just
		// shrunk). Reciting A as "Goal (fixed)" at the tail outranks B in the
		// recency window, and the model refuses B as out of scope.
		let sig = crate::session::anchor::task_sig;
		let mut a = Anchor::default();
		a.extend(
			AnchorUpdate {
				intent: Some("Request A".to_string()),
				next_steps: vec!["finish A".to_string()],
				intent_task_sig: Some(sig("Request A")),
				..Default::default()
			},
			0,
		);
		// Still the live request -> recites exactly as before.
		let live = recite_note(&a, None, &[], Some(sig("Request A"))).expect("live goal recites");
		assert!(live.contains("<intent>Request A</intent>"));

		// User moved on -> the superseded goal and its next_steps stay out.
		assert!(
			recite_note(&a, None, &[], Some(sig("Request B"))).is_none(),
			"a goal from a superseded request must not be recited"
		);

		// Constraints come from the CURRENT request, so they still recite.
		let cs = vec!["Do not touch tests.".to_string()];
		let note = recite_note(&a, None, &cs, Some(sig("Request B")))
			.expect("current-request constraints still recite");
		assert!(!note.contains("Request A"), "{note}");
		assert!(!note.contains("finish A"), "{note}");
		assert!(note.contains("Do not touch tests."), "{note}");
	}

	#[test]
	fn live_plan_recites_even_with_empty_anchor() {
		let note = recite_note(
			&Anchor::default(),
			Some("Live plan (0/1 done):\n🔄 first ← current"),
			&[],
			None,
		)
		.expect("active plan recites pre-compaction");
		assert!(crate::supervisor::gate::is_supervisor_injection(&note));
		assert!(note.contains("🔄 first ← current"));
	}
}
