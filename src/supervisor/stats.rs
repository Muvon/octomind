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

//! Supervisor activity + usage tally, surfaced in `/info`.
//!
//! The supervisor's own model calls (verify-gate, distill, recall-prep) run on a
//! separate cheap model and are otherwise invisible to the session totals. This
//! process-global accumulator captures their token/cost spend plus what the
//! supervisor *did* (gate runs, steers, lessons/orientation stored, recalls) so
//! `/info` can show it. One process == one interactive session, so a global is
//! effectively session-scoped (same approach as the agents stats).

use std::sync::{Mutex, OnceLock};

#[derive(Default, Clone)]
struct Stats {
	calls: u64,
	input_tokens: u64,
	output_tokens: u64,
	cost: f64,
	gate_runs: u64,
	gate_pass: u64,
	gate_fail: u64,
	steers: u64,
	lessons_stored: u64,
	orientation_stored: u64,
	recalls_injected: u64,
}

fn global() -> &'static Mutex<Stats> {
	static S: OnceLock<Mutex<Stats>> = OnceLock::new();
	S.get_or_init(|| Mutex::new(Stats::default()))
}

fn with<F: FnOnce(&mut Stats)>(f: F) {
	if let Ok(mut s) = global().lock() {
		f(&mut s);
	}
}

/// Record one supervisor model call's usage (verify-gate / distill / recall-prep).
pub fn record_call(input_tokens: u64, output_tokens: u64, cost: f64) {
	with(|s| {
		s.calls += 1;
		s.input_tokens += input_tokens;
		s.output_tokens += output_tokens;
		s.cost += cost;
	});
}

/// A verify-gate verification ran (regardless of verdict).
pub fn gate_run() {
	with(|s| s.gate_runs += 1);
}
/// The verify-gate accepted the run.
pub fn gate_pass() {
	with(|s| s.gate_pass += 1);
}
/// The verify-gate gave up with gaps remaining (trajectory unverified).
pub fn gate_fail() {
	with(|s| s.gate_fail += 1);
}
/// A steer (advisory re-anchor) was queued.
pub fn steer() {
	with(|s| s.steers += 1);
}
/// `n` lessons were stored by distill.
pub fn lessons(n: u64) {
	with(|s| s.lessons_stored += n);
}
/// `n` orientation entries were stored by distill.
pub fn orientation(n: u64) {
	with(|s| s.orientation_stored += n);
}
/// One recall injection happened.
pub fn recall() {
	with(|s| s.recalls_injected += 1);
}

/// JSON snapshot for `/info`. Returns `None` when the supervisor did nothing,
/// so the section is omitted entirely on idle sessions.
pub fn snapshot() -> Option<serde_json::Value> {
	let s = global().lock().ok()?.clone();
	let idle = s.calls == 0
		&& s.gate_runs == 0
		&& s.steers == 0
		&& s.lessons_stored == 0
		&& s.orientation_stored == 0
		&& s.recalls_injected == 0;
	if idle {
		return None;
	}
	Some(serde_json::json!({
		"calls": s.calls,
		"input_tokens": s.input_tokens,
		"output_tokens": s.output_tokens,
		"cost": s.cost,
		"gate_runs": s.gate_runs,
		"gate_pass": s.gate_pass,
		"gate_fail": s.gate_fail,
		"steers": s.steers,
		"lessons_stored": s.lessons_stored,
		"orientation_stored": s.orientation_stored,
		"recalls_injected": s.recalls_injected,
	}))
}
