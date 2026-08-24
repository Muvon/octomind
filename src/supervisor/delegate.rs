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

//! Delegation handback — verification outcomes reported by finished subagents.
//!
//! `tap run` and `agent_*` launch a CONTEXT-ISOLATED child: it sees only the
//! prompt string the parent wrote. Handoff quality is owned at prompt time by
//! the spawn tools' own descriptions (the child starts with zero context, so
//! the prompt must be self-contained); the child itself is the ground truth
//! for whether the handoff worked — it reports its own end-of-turn verdict
//! back over the ACP `_meta` channel, and the parent folds that into its
//! detector/gate state here.

/// Verification outcomes of subagent runs that finished since the parent last
/// folded a tool round.
///
/// A handoff collapses the child's WHOLE trajectory — change, then check —
/// into ONE parent tool round. The parent's own rule ("a verifier succeeded on
/// a tree this round did not change") is therefore unsatisfiable by
/// construction for delegated work: the same round both dirties and checks.
/// An orchestrator that works only through `tap` could never clear
/// `agent_dirty`, so every `done` re-triggered the mutation pre-gate until the
/// gate budget ran out — burning a subagent spawn per re-run on a check it
/// could not pass.
///
/// The fix keeps the module's invariant (measure effects, never classify
/// tools): the child IS octomind and measures its own tree the same way, one
/// level down, so the parent reads the child's verdict instead of re-deriving
/// it from a fingerprint that cannot see inside the round.
#[derive(Default, Clone, Copy)]
struct Handback {
	/// Subagent runs that reached a terminal state in this window.
	finished: usize,
	/// …of which ended with their own trajectory verified.
	verified: usize,
}

static HANDBACK: std::sync::RwLock<
	Option<std::collections::HashMap<crate::session::context::SessionId, Handback>>,
> = std::sync::RwLock::new(None);

/// Record how one finished subagent run ended. `verified` is the child's own
/// end-of-turn verdict, reported over the ACP `_meta` channel. A run that
/// failed, was cancelled, or reported nothing counts as UNVERIFIED — a missing
/// verdict can only ever be conservative.
pub fn note_handback(verified: bool) {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	let Ok(mut guard) = HANDBACK.write() else {
		return;
	};
	let entry = guard.get_or_insert_with(Default::default).entry(session_id);
	let slot = entry.or_default();
	slot.finished += 1;
	slot.verified += usize::from(verified);
}

/// Take and clear the outcomes accumulated since the last call, as
/// `(finished, verified)`. Drained once per tool round by the round fold.
pub fn take_handback() -> (usize, usize) {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return (0, 0);
	};
	let Ok(mut guard) = HANDBACK.write() else {
		return (0, 0);
	};
	let Some(map) = guard.as_mut() else {
		return (0, 0);
	};
	let h = map.remove(&session_id).unwrap_or_default();
	(h.finished, h.verified)
}

/// Drop the session's tally on session end.
pub fn clear_handback_for_session(session_id: &crate::session::context::SessionId) {
	if let Ok(mut guard) = HANDBACK.write() {
		if let Some(map) = guard.as_mut() {
			map.remove(session_id);
		}
	}
}
