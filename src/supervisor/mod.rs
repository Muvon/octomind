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

//! Supervisor — the out-of-band control plane around the agent loop.
//!
//! Runs *beside* the main loop, never in the user's transcript. Hosts:
//! - `learning` — distill (end-of-trajectory lessons) + recall (inject).
//! - orientation — a second memory kind: durable understanding of the subject
//!   (decisions, structure, constraints), stored as `memory_type = "orientation"`.
//! - detectors — deterministic, free, every turn: loop / no-progress / stop-intent.
//!   Fused with the agent's own self-report token before any model is woken.
//! - gate — verify-gate on self-reported `done`; labels the run for learning.
//! - condense — task-aware narrowing of oversized tool outputs (line-range
//!   selection, never retyping) so the agent model sees only what the task needs.
//!
//! Invariants:
//! 1. Free signals (counters + self-report) gate the model; model calls are rare.
//! 2. Injections are advisory system-side notes — never silent context rewrites.
//! 3. Out-of-band: status tokens are stripped from display; deliberation never
//!    reaches the user transcript.
//!
//! Config is STRICT: every field below is required. A missing `[supervisor]`
//! section or any missing key is a hard parse error — we own the schema, so we
//! fail loudly instead of degrading to silent defaults.

pub mod condense;
pub mod detect;
pub mod gate;
pub mod learning;
pub mod recite;
pub mod stats;

use serde::{Deserialize, Serialize};

/// Out-of-band notice (`· Supervisor: …`) so the user sees what the control
/// plane is doing — mirrors the skill-activation notice: dim, stderr,
/// interactive terminals only. Continuation lines (multi-line messages, e.g.
/// gate gaps) are indented under the first.
pub fn notify(message: &str) {
	let suppress = crate::config::with_thread_config(|c| c.output_mode())
		.map(|m| m.should_suppress_cli_output())
		.unwrap_or(false);
	if suppress || !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
		return;
	}
	use colored::Colorize;
	for (i, line) in message.lines().enumerate() {
		if i == 0 {
			eprintln!(
				"{} {} {}",
				"·".bright_black(),
				"Supervisor:".dimmed(),
				line.dimmed()
			);
		} else {
			eprintln!("  {}", line.dimmed());
		}
	}
}

/// Top-level supervisor configuration. Maps to the `[supervisor]` TOML section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
	/// Master switch for the whole control plane.
	pub enabled: bool,
	/// Shared cheap model for supervisor mechanics (e.g. the verify-gate).
	pub model: String,
	/// Evidence-bound claims: instruct the agent to back load-bearing repo facts
	/// with a verbatim `« »` quote, then deterministically verify each quote
	/// occurs in a tool result. Fabricated citations are re-grounded via the
	/// verify-gate's bounded re-run (so this is effective only when `gate.enabled`).
	pub claim_check: bool,
	/// Cross-session learning mechanic (distill + recall).
	pub learning: learning::LearningConfig,
	/// Orientation memory (durable subject understanding).
	pub orientation: OrientationConfig,
	/// Deterministic detectors (loop / no-progress / stop-intent).
	pub detectors: DetectorsConfig,
	/// Verify-gate on self-reported completion.
	pub gate: GateConfig,
	/// Goal recitation: re-anchor the live goal at the context tail.
	pub recite: ReciteConfig,
	/// Task-aware condensation of oversized tool outputs.
	pub condense: CondenseConfig,
	/// Circuit-breaker: hard-stop a turn after this many consecutive tool rounds that
	/// emitted (or backed-off-but-still-dominant) a steer without the model breaking out.
	/// `0` = unlimited (off). The terminal hard ceiling under the adaptive steer backoff,
	/// which is itself parameter-free (see the steer loop in `response.rs`).
	pub max_consecutive_steers: usize,
}

/// Goal recitation. On long (already-compacted) sessions the durable goal lives
/// in the `Anchor` but is only rendered inside the mid-transcript compressed
/// summary, where attention is weak. Recitation re-emits a tiny goal block at
/// the context *tail* each turn — the recency slot — so it stays salient. No
/// model call, no new memory: pure reuse of the existing `Anchor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReciteConfig {
	/// Master switch. When on, recitation fires only once the anchor is
	/// populated (i.e. the session has compacted at least once), so short
	/// sessions pay nothing.
	pub enabled: bool,
}

/// Condense: task-aware narrowing of oversized tool outputs. When a round
/// returns results over `tokens_threshold`, one cheap-model call selects per
/// result what the current task needs — by LINE RANGES over a numbered copy,
/// reconstructed verbatim (never retyped). Full originals are spilled to
/// session files first, so it is lossless; the hard
/// `mcp_response_tokens_threshold` cap still applies after as the ceiling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CondenseConfig {
	pub enabled: bool,
	/// Per-result trigger (estimated tokens); results above this are condensed.
	/// `0` disables. Keep well below `mcp_response_tokens_threshold`.
	pub tokens_threshold: usize,
	/// Model that does the narrowing (cheap + fast recommended).
	pub model: String,
}

/// Orientation memory: durable, expensive-to-re-derive understanding of the
/// subject. Stored in the same backend as lessons under `memory_type =
/// "orientation"`. Recalled as *working assumptions to verify*, never truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrientationConfig {
	pub enabled: bool,
	/// Max orientation entries injected per session.
	pub max_inject: usize,
	/// Soft time-decay: entries unused for this many days lose confidence.
	pub decay_days: u64,
}

/// Deterministic detector thresholds. These never call a model themselves —
/// they are the cheap trigger that decides when (rarely) to wake the Reflector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorsConfig {
	/// Identical tool+args this many times in a row → loop fired.
	pub loop_threshold: usize,
	/// Turns without new information → drift candidate.
	pub no_progress_window: usize,
	/// Consecutive truncated tool results this many times in a row → the model
	/// is ignoring the truncation notice and re-querying without narrowing.
	/// Tool-agnostic: keyed on the truncation sentinel, not on tool identity.
	pub truncation_threshold: usize,
	/// Consecutive deduplicated tool results this many times in a row → the model
	/// is re-issuing calls whose output it already received this session.
	/// Tool-agnostic: keyed on the dedup sentinel, not on tool identity.
	pub dedup_threshold: usize,
	/// Consecutive off-task RESULTS this many times in a row → the model has drifted
	/// from the line of work it was pursuing (distractor failure mode). `0` disables
	/// the signal entirely (no embedding cost). When enabled it costs one embedding
	/// per sizable result and scores it against a centroid of recent on-task results
	/// — self-referential, so no task anchor is needed (robust to abstract requests).
	pub distraction_threshold: usize,
	/// Off-task FLOOR (not a relevance boundary): a result is drift only when its
	/// cosine to the working-set centroid falls BELOW this. Embeddings are reliable
	/// at "clearly unrelated" (the far-low tail), not at "is this relevant", so this
	/// is a high-precision tail cutoff. Model-dependent; the drift decision is logged
	/// at debug when the signal is on, so tune from real data. Lower = stricter.
	pub drift_floor: f32,
	/// Inject the self-report status-token instruction and parse it back.
	pub self_report: bool,
	/// Consecutive single-tool-call ROUNDS this many times in a row → the model is
	/// issuing one tool call per turn when independent calls could be batched into a
	/// single parallel round. `0` = OFF: single calls are often legitimate (genuinely
	/// dependent calls, or one real action), so this is advisory and conservative.
	pub sequential_threshold: usize,
}

/// Verify-gate configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
	pub enabled: bool,
	/// Max gate re-entry iterations before giving up (bounds the
	/// self-verification dilemma). Shared budget across the free pre-gate and
	/// the LLM verify-gate.
	pub max_iterations: u8,
	/// Model the gate verifies WITH (`provider:model`). Deliberately separate
	/// from the generator: a same-family verifier inherits the same blind spots
	/// and rubber-stamps them, so the strongest signal comes from a *different*
	/// family. Required — no silent fallback to the generator model.
	pub verifier_model: String,
	/// Free deterministic pre-gate: refuse a self-reported `done` when code was
	/// changed but no successful check (build/test/lint/etc.) ran since the last
	/// change. Tool-agnostic — keyed on the mutation-without-clean-check pattern,
	/// not on tool names. Runs before the LLM verify-gate (zero model calls).
	pub require_check_after_mutation: bool,
	/// Free deterministic pre-gate: refuse a self-reported `done` while the live
	/// plan checklist still has open items — the drift-by-omission failure. The
	/// agent must finish them or close them out via the plan tool first. Runs
	/// before the LLM verify-gate (zero model calls).
	pub require_plan_complete: bool,
}
