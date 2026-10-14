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

//! Spend by models that run outside the main agent loop — subagents (`agent_*`,
//! `tap run`, tap workflows), layers, and the supervisor's own cheap-model calls.
//!
//! None of them hold a `&mut Session` at the moment they spend, so the cost is
//! banked here and drained into `SessionInfo::total_cost` by
//! [`crate::session::Session::fold_external_spend`] at the next point that does.
//! Draining (rather than reading a running total) is what makes every dollar
//! land exactly once, including across the monotonic-max merge that
//! `persistence` applies on resume.
//!
//! Banked per session, by the recording task's session context: `octomind
//! server` runs every session in one process, and a single accumulator handed
//! one session's delegated spend to whichever session folded next. Detached runs
//! re-enter their session's context (`tap`, `agent_*`). Spend recorded outside
//! any session has no owner to wait for, so the
//! next fold takes it, as before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::session::context::{current_session_id, SessionId};

/// `None` = spend recorded outside any session context.
type PendingSpend = Arc<Mutex<HashMap<Option<SessionId>, f64>>>;

#[cfg(test)]
tokio::task_local! { static TEST_PENDING: PendingSpend; }

fn pending() -> PendingSpend {
	#[cfg(test)]
	if let Ok(bank) = TEST_PENDING.try_with(Arc::clone) {
		return bank;
	}
	static P: OnceLock<PendingSpend> = OnceLock::new();
	Arc::clone(P.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))))
}

#[cfg(test)]
pub(crate) async fn isolated<F: std::future::Future>(future: F) -> F::Output {
	TEST_PENDING.scope(Arc::default(), future).await
}

/// Bank spend by a model that runs outside the main loop, against the current session.
pub fn record(cost: f64) {
	if cost <= 0.0 {
		return;
	}
	if let Ok(mut p) = pending().lock() {
		*p.entry(current_session_id()).or_insert(0.0) += cost;
	}
}

/// Take everything banked for the supplied owner (plus unowned spend),
/// leaving both empty.
fn take_owner(own: Option<SessionId>) -> f64 {
	let bank = pending();
	let Ok(mut p) = bank.lock() else {
		return 0.0;
	};
	let unowned = if own.is_some() {
		p.remove(&None).unwrap_or(0.0)
	} else {
		0.0
	};
	p.remove(&own).unwrap_or(0.0) + unowned
}

/// Drain against the receiving session, even when called from a different task scope.
pub fn take_for(owner: &str) -> f64 {
	take_owner(Some(owner.to_owned()))
}

#[cfg(test)]
pub(crate) fn take() -> f64 {
	take_owner(current_session_id())
}

#[cfg(test)]
#[path = "external_spend_tests.rs"]
mod tests;
