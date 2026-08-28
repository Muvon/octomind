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
//! separate cheap model. This process-global accumulator captures their
//! token/cost spend plus what the supervisor *did* (gate runs, steers,
//! lessons/orientation stored, recalls) so `/info` can show it as its own
//! breakdown; the cost also feeds `session::external_spend` so it lands in the
//! session total. One process == one interactive session, so a global is
//! effectively session-scoped (same approach as the agents stats).

use std::sync::{Mutex, OnceLock};

/// Which supervisor mechanic made a model call — so `/info` can break the count
/// down instead of showing one opaque total.
#[derive(Clone, Copy, Debug)]
pub enum CallKind {
	/// Recall keyword/query preparation.
	Recall,
	/// Verify-gate completion check.
	Gate,
	/// Current-turn dependency classification and minimal resolution.
	Resolve,
	/// External plan creation and phase transition decision.
	Plan,
	/// End-of-trajectory lesson/orientation extraction.
	Distill,
	/// Tool-output condensation (task-aware narrowing).
	Condense,
}

#[derive(Default, Clone)]
struct Stats {
	calls: u64,
	recall_calls: u64,
	gate_calls: u64,
	resolve_calls: u64,
	plan_calls: u64,
	distill_calls: u64,
	condense_calls: u64,
	condensed_results: u64,
	condense_saved_tokens: u64,
	memory_pack_items: u64,
	memory_pack_tokens: u64,
	memory_credit_positive: u64,
	memory_credit_negative: u64,
	memory_consolidations: u64,
	memory_archived: u64,
	input_tokens: u64,
	output_tokens: u64,
	/// Wall time of the supervisor's own API requests, for throughput.
	api_time_ms: u64,
	cost: f64,
	gate_runs: u64,
	gate_pass: u64,
	gate_fail: u64,
	gate_stall: u64,
	steers: u64,
	// Per-signal steer breakdown — which detector signal fired each steer.
	steer_loop: u64,
	steer_no_progress: u64,
	steer_recovery: u64,
	pregate_blocks: u64,
	lessons_stored: u64,
	orientation_stored: u64,
	experiences_stored: u64,
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

/// Record one supervisor model call's usage, attributed to the mechanic that
/// made it (task resolution / verify-gate / distill / recall-prep).
pub fn record_call(
	kind: CallKind,
	input_tokens: u64,
	output_tokens: u64,
	api_time_ms: u64,
	cost: f64,
) {
	with(|s| {
		s.calls += 1;
		match kind {
			CallKind::Recall => s.recall_calls += 1,
			CallKind::Gate => s.gate_calls += 1,
			CallKind::Resolve => s.resolve_calls += 1,
			CallKind::Plan => s.plan_calls += 1,
			CallKind::Distill => s.distill_calls += 1,
			CallKind::Condense => s.condense_calls += 1,
		}
		s.input_tokens += input_tokens;
		s.output_tokens += output_tokens;
		s.api_time_ms += api_time_ms;
		s.cost += cost;
	});
	crate::session::external_spend::record(cost);
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
/// The verify-gate returned an unchanged finding after the re-run gathered new
/// evidence — the check could not converge, so the loop stopped charging it.
/// A rising count means the verifier is asking for something unreachable.
pub fn gate_stall() {
	with(|s| s.gate_stall += 1);
}
/// A steer (advisory re-anchor) was queued, attributed to the detector signal
/// that fired it so `/info` can break the total down by signal.
pub fn steer(signal: crate::supervisor::detect::DetectorSignal) {
	use crate::supervisor::detect::DetectorSignal;
	with(|s| {
		s.steers += 1;
		match signal {
			DetectorSignal::Loop => s.steer_loop += 1,
			DetectorSignal::NoProgress => s.steer_no_progress += 1,
			DetectorSignal::Recovery => s.steer_recovery += 1,
			DetectorSignal::None => {}
		}
	});
}
/// The deterministic pre-gate refused a `done` (code changed, no check ran).
pub fn pregate_block() {
	with(|s| s.pregate_blocks += 1);
}
/// `n` lessons were stored by distill.
pub fn lessons(n: u64) {
	with(|s| s.lessons_stored += n);
}
/// `n` orientation entries were stored by distill.
pub fn orientation(n: u64) {
	with(|s| s.orientation_stored += n);
}
/// `n` grounded long-lived experience records were stored by distill.
pub fn experience(n: u64) {
	with(|s| s.experiences_stored += n);
}
/// One recall injection happened.
pub fn recall() {
	with(|s| s.recalls_injected += 1);
}
/// `results` tool outputs were condensed this round, saving `saved_tokens`
/// estimated tokens of agent-model context.
pub fn condensed(results: u64, saved_tokens: u64) {
	with(|s| {
		s.condensed_results += results;
		s.condense_saved_tokens += saved_tokens;
	});
}
/// One bounded active-memory pack was selected for a genuine user turn.
pub fn memory_pack(items: u64, tokens: u64) {
	with(|s| {
		s.memory_pack_items += items;
		s.memory_pack_tokens += tokens;
	});
}
/// One materially used memory received outcome credit from the verify-gate.
pub fn memory_credit(positive: bool) {
	with(|s| {
		if positive {
			s.memory_credit_positive += 1;
		} else {
			s.memory_credit_negative += 1;
		}
	});
}
/// File-authoritative long-run maintenance completed. These process-local
/// counters answer how much consolidation/archive work occurred; they are not
/// anonymous telemetry and detached exit distillers have their own process.
pub fn memory_retention(consolidated: u64, archived: u64) {
	with(|s| {
		s.memory_consolidations += consolidated;
		s.memory_archived += archived;
	});
}
/// JSON snapshot for `/info`. Returns `None` when the supervisor did nothing,
/// so the section is omitted entirely on idle sessions.
pub fn snapshot() -> Option<serde_json::Value> {
	let s = global().lock().ok()?.clone();
	let idle = s.calls == 0
		&& s.gate_runs == 0
		&& s.steers == 0
		&& s.pregate_blocks == 0
		&& s.lessons_stored == 0
		&& s.orientation_stored == 0
		&& s.experiences_stored == 0
		&& s.memory_consolidations == 0
		&& s.memory_archived == 0
		&& s.recalls_injected == 0;
	if idle {
		return None;
	}
	// Steer breakdown by signal — ordered, non-zero only, so display stays generic.
	let steer_signals: Vec<serde_json::Value> = [
		("loop", s.steer_loop),
		("no-progress", s.steer_no_progress),
		("recovery", s.steer_recovery),
	]
	.into_iter()
	.filter(|(_, n)| *n > 0)
	.map(|(label, n)| serde_json::json!({ "label": label, "count": n }))
	.collect();
	Some(serde_json::json!({
		"calls": s.calls,
		"recall_calls": s.recall_calls,
		"gate_calls": s.gate_calls,
		"resolve_calls": s.resolve_calls,
		"plan_calls": s.plan_calls,
		"distill_calls": s.distill_calls,
		"condense_calls": s.condense_calls,
		"condensed_results": s.condensed_results,
		"condense_saved_tokens": s.condense_saved_tokens,
		"memory_pack_items": s.memory_pack_items,
		"memory_pack_tokens": s.memory_pack_tokens,
		"memory_credit_positive": s.memory_credit_positive,
		"memory_credit_negative": s.memory_credit_negative,
		"memory_consolidations": s.memory_consolidations,
		"memory_archived": s.memory_archived,
		"input_tokens": s.input_tokens,
		"output_tokens": s.output_tokens,
		"tokens_per_second": if s.api_time_ms > 0 {
			s.output_tokens as f64 / (s.api_time_ms as f64 / 1000.0)
		} else {
			0.0
		},
		"cost": s.cost,
		"gate_runs": s.gate_runs,
		"gate_pass": s.gate_pass,
		"gate_fail": s.gate_fail,
		"gate_stall": s.gate_stall,
		"steers": s.steers,
		"steer_signals": steer_signals,
		"pregate_blocks": s.pregate_blocks,
		"lessons_stored": s.lessons_stored,
		"orientation_stored": s.orientation_stored,
		"experiences_stored": s.experiences_stored,
		"recalls_injected": s.recalls_injected,
	}))
}

#[cfg(test)]
#[path = "stats_tests.rs"]
mod tests;
