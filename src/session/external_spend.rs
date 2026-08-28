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
//! `tap run`), layers, and the supervisor's own cheap-model calls.
//!
//! None of them hold a `&mut Session` at the moment they spend, so the cost
//! lands in this process-global accumulator and is drained into
//! `SessionInfo::total_cost` by [`crate::session::Session::fold_external_spend`]
//! at the next point that does. Draining (rather than reading a running total)
//! is what makes every dollar land exactly once, including across the
//! monotonic-max merge that `persistence` applies on resume.
//!
//! One process == one interactive session, so a global is effectively
//! session-scoped (same assumption as `supervisor::stats`).

use std::sync::{Mutex, OnceLock};

fn pending() -> &'static Mutex<f64> {
	static P: OnceLock<Mutex<f64>> = OnceLock::new();
	P.get_or_init(|| Mutex::new(0.0))
}

/// Bank spend by a model that runs outside the main loop.
pub fn record(cost: f64) {
	if cost <= 0.0 {
		return;
	}
	if let Ok(mut p) = pending().lock() {
		*p += cost;
	}
}

/// Take everything banked so far, leaving the accumulator empty.
pub fn take() -> f64 {
	pending()
		.lock()
		.map(|mut p| std::mem::take(&mut *p))
		.unwrap_or(0.0)
}

#[cfg(test)]
#[path = "external_spend_tests.rs"]
mod tests;
