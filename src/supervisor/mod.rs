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
//!   (decisions, structure, constraints), stored as `memory_type = "orientation"`,
//!   managed inside `learning`.
//! - detectors — deterministic, free, every turn: loop / no-progress / recovery.
//!   Fused with the agent's own self-report token before any model is woken.
//!   Thresholds are fixed constants (`detect::LOOP_THRESHOLD`,
//!   `detect::NO_PROGRESS_WINDOW`) — good defaults, not knobs.
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
pub mod delegate;
pub mod detect;
pub mod gate;
pub mod learning;
pub mod plan;
pub mod recite;
pub mod resolve;
pub mod stats;
pub mod workdir;

use serde::{Deserialize, Serialize};

/// Session-level user policy for whether the assistant may run verification.
/// Kept outside detector streak state: it is an instruction, not an observed
/// trajectory signal, and survives resume until the user explicitly changes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationPolicy {
	#[default]
	Unspecified,
	Forbidden,
	Allowed,
}

impl VerificationPolicy {
	pub fn forbids(self) -> bool {
		self == Self::Forbidden
	}

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Unspecified => "unspecified",
			Self::Forbidden => "forbidden",
			Self::Allowed => "allowed",
		}
	}

	/// Effective boundary for this turn. Standing role instructions and a
	/// reply-local user restriction may forbid verification without mutating the
	/// persisted user-owned policy.
	pub fn effective(self, current_turn_forbids: bool) -> Self {
		if current_turn_forbids {
			Self::Forbidden
		} else {
			self
		}
	}

	/// Apply one classified user delta. Returns whether durable state changed.
	pub fn apply(&mut self, update: VerificationPolicyUpdate) -> bool {
		let previous = *self;
		match update {
			VerificationPolicyUpdate::Unchanged => {}
			VerificationPolicyUpdate::Forbid => *self = Self::Forbidden,
			// `Allow` REVOKES a prior prohibition. With nothing forbidden there is
			// nothing to revoke, and materializing `Allowed` makes every turn recite
			// a standing execution licence — which a review-only role reads as an
			// instruction to run the build (observed: session 260821-backend-2007-683c,
			// a `developer:brief` run that spent an hour on docker builds and test
			// suites it was never asked for).
			VerificationPolicyUpdate::Allow => {
				if *self == Self::Forbidden {
					*self = Self::Allowed;
				}
			}
		}
		*self != previous
	}
}

/// Delta emitted from one genuine user turn. Absence is distinct from an
/// explicit revocation, so ordinary follow-ups preserve standing policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VerificationPolicyUpdate {
	#[default]
	Unchanged,
	Forbid,
	Allow,
}

#[cfg(test)]
#[path = "verification_policy_tests.rs"]
mod verification_policy_tests;

/// Escape untrusted text before embedding it inside supervisor-owned XML-like
/// control blocks. This preserves field boundaries against literal closing tags.
pub(crate) fn escape_xml_text(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
}

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

/// Cap on the standing-instructions block handed to supervisor models.
const ROLE_CONTEXT_CHARS: usize = 4_000;

/// Standing role instructions — the session's system message: the durable rules
/// the agent operates under, distinct from the current user turn. Every
/// supervisor that judges intent (resolve, gate, delegate) receives this block
/// so a standing rule can exonerate or convict independently of the turn.
pub fn role_context(messages: &[crate::session::Message]) -> String {
	let Some(system) = messages.iter().find(|m| m.role == "system") else {
		return String::new();
	};
	let trimmed = system.content.trim();
	if trimmed.chars().count() <= ROLE_CONTEXT_CHARS {
		trimmed.to_string()
	} else {
		trimmed.chars().take(ROLE_CONTEXT_CHARS).collect()
	}
}

/// Top-level supervisor configuration. Maps to the `[supervisor]` TOML section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
	/// Master switch for the whole control plane.
	pub enabled: bool,
	/// One shared profile for gate, resolve, plan, and condense.
	#[serde(default)]
	pub model: crate::config::ModelProfileOverride,
	/// Cross-session learning mechanic (distill + recall + orientation).
	pub learning: learning::LearningConfig,
	/// Verify-gate on self-reported completion.
	pub gate: GateConfig,
	/// External, adaptive plan manager. The specialist sees plan state but cannot
	/// mutate it directly.
	pub plan: PlanConfig,
	/// Task-aware condensation of oversized tool outputs.
	pub condense: CondenseConfig,
}

/// Condense: task-aware narrowing of oversized tool outputs. A result whose own
/// output exceeds `tokens_threshold` is a candidate; smaller results in the same
/// round are untouched and never shown to the condenser. One cheap-model call
/// per round selects per candidate what the current task needs — by ORIGINAL
/// LINE RANGES over a bounded task-aware view, reconstructed verbatim (never
/// retyped). Full originals are spilled when the active role can read them back.
/// The hard `mcp_response_tokens_threshold` cap is applied BEFORE this, so
/// condensation only ever narrows what the agent would actually receive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CondenseConfig {
	pub enabled: bool,
	/// Adapt the trigger during this process-local session from the condenser's
	/// realized token savings. The configured threshold remains the baseline;
	/// the runtime multiplier starts at 1.0 and is bounded to 0.5x..2.0x.
	pub adaptive: bool,
	/// Per-result trigger (estimated tokens of that single text result); results
	/// above this are condensed. `0` disables. Keep well below
	/// `mcp_response_tokens_threshold`.
	pub tokens_threshold: usize,
}

/// Verify-gate configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
	pub enabled: bool,
}

/// External plan-manager configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanConfig {
	pub enabled: bool,
}

impl SupervisorConfig {
	pub fn resolved_model_profile(
		&self,
		main: &crate::config::ModelProfile,
	) -> crate::config::ModelProfile {
		self.model.resolve(main)
	}
}

#[cfg(test)]
#[path = "plan_e2e_tests.rs"]
mod plan_e2e_tests;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
