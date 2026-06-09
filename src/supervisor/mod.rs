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

pub mod detect;
pub mod gate;
pub mod learning;
pub mod stats;

use serde::{Deserialize, Serialize};

/// Top-level supervisor configuration. Maps to the `[supervisor]` TOML section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
	/// Master switch for the whole control plane.
	pub enabled: bool,
	/// Shared cheap model for supervisor mechanics (e.g. the verify-gate).
	pub model: String,
	/// Cross-session learning mechanic (distill + recall).
	pub learning: learning::LearningConfig,
	/// Orientation memory (durable subject understanding).
	pub orientation: OrientationConfig,
	/// Deterministic detectors (loop / no-progress / stop-intent).
	pub detectors: DetectorsConfig,
	/// Verify-gate on self-reported completion.
	pub gate: GateConfig,
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
	/// Inject the self-report status-token instruction and parse it back.
	pub self_report: bool,
}

/// Verify-gate configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
	pub enabled: bool,
	/// Max gate re-entry iterations before giving up (bounds the
	/// self-verification dilemma).
	pub max_iterations: u8,
}
