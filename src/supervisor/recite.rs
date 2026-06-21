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

/// Build the recitation note for the context tail, or `None` when there is
/// nothing durable to recite yet.
///
/// Recites `intent` verbatim — it is immutable (first-write-wins) so it never
/// drifts. For the "what to do now" part it prefers the LIVE plan checklist
/// (`plan_checklist`, re-read every turn from the plan tool's storage) over the
/// `next_steps` snapshot, which only refreshes at compaction and is stale
/// between. With an active plan it recites even before the first compaction —
/// that is exactly when goal drift on a long task is most expensive. Wrapped in
/// `<pay-attention>` so [`crate::supervisor::gate::is_supervisor_injection`]
/// excludes it from the verify-gate's real-task search.
pub fn recite_note(anchor: &Anchor, plan_checklist: Option<&str>) -> Option<String> {
	if anchor.intent.is_empty() && anchor.next_steps.is_empty() && plan_checklist.is_none() {
		return None;
	}
	let mut s =
		String::from("<pay-attention>\nYou are deep in this session — re-anchor on your goal:\n");
	if !anchor.intent.is_empty() {
		s.push_str("Goal (fixed): <intent>");
		s.push_str(anchor.intent.trim());
		s.push_str("</intent>\n");
	}
	// Prefer the live plan checklist (current every turn) over the stale
	// next_steps snapshot; fall back to next_steps only when no plan is active.
	if let Some(checklist) = plan_checklist.map(str::trim).filter(|c| !c.is_empty()) {
		s.push_str(checklist);
		s.push('\n');
	} else if !anchor.next_steps.is_empty() {
		s.push_str("Last-known next steps (may be stale — re-check against current state):\n");
		for step in &anchor.next_steps {
			let step = step.trim();
			if !step.is_empty() {
				s.push_str("- ");
				s.push_str(step);
				s.push('\n');
			}
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
		assert!(recite_note(&Anchor::default(), None).is_none());
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
		let note = recite_note(&a, None).expect("should recite");
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
		let note = recite_note(&a, None).expect("should recite");
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
		)
		.expect("should recite");
		assert!(note.contains("<intent>Ship the feature</intent>"));
		assert!(note.contains("🔄 do this ← current"));
		// The live checklist replaces the stale snapshot entirely.
		assert!(!note.contains("STALE step"));
		assert!(!note.contains("Last-known next steps"));
	}

	#[test]
	fn live_plan_recites_even_with_empty_anchor() {
		let note = recite_note(
			&Anchor::default(),
			Some("Live plan (0/1 done):\n🔄 first ← current"),
		)
		.expect("active plan recites pre-compaction");
		assert!(crate::supervisor::gate::is_supervisor_injection(&note));
		assert!(note.contains("🔄 first ← current"));
	}
}
