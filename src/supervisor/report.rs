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

//! The session-only supervisor view shared by every /info and cost surface.

use super::{
	accounting::{SupervisorAccounting, TurnTime, Usage},
	evaluate::Seam,
	stats::CallKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mechanic {
	Gate,
	Authorize,
	Condense,
	Recall,
	Resolve,
	Plan,
	Distill,
	Retention,
	Evolution,
	Hindsight,
	Skills,
	Capabilities,
	Compression,
}

impl Mechanic {
	pub fn name(self) -> &'static str {
		match self {
			Self::Gate => "gate",
			Self::Authorize => "authorize",
			Self::Condense => "condense",
			Self::Recall => "recall",
			Self::Resolve => "resolve",
			Self::Plan => "plan",
			Self::Distill => "distill",
			Self::Retention => "retention",
			Self::Evolution => "evolution",
			Self::Hindsight => "hindsight",
			Self::Skills => "skills",
			Self::Capabilities => "capabilities",
			Self::Compression => "compression",
		}
	}
	pub(super) fn from_kind(kind: CallKind) -> Option<Self> {
		Some(match kind {
			CallKind::Gate => Self::Gate,
			CallKind::Authorize => Self::Authorize,
			CallKind::Condense => Self::Condense,
			CallKind::Recall => Self::Recall,
			CallKind::Resolve => Self::Resolve,
			CallKind::Plan => Self::Plan,
			CallKind::Distill => Self::Distill,
			CallKind::Retention => Self::Retention,
			CallKind::Evolution => Self::Evolution,
			CallKind::Hindsight => Self::Hindsight,
			CallKind::Evaluate => return None,
		})
	}
	pub fn from_seam(seam: Seam) -> Self {
		match seam {
			Seam::Gate => Self::Gate,
			Seam::Authorizer => Self::Authorize,
			Seam::Condense => Self::Condense,
			Seam::Recall => Self::Recall,
			Seam::Distill => Self::Distill,
			Seam::Plan => Self::Plan,
			Seam::Evolution => Self::Evolution,
			Seam::Skills => Self::Skills,
			Seam::Capabilities => Self::Capabilities,
			Seam::Compression => Self::Compression,
		}
	}
	pub(super) fn usage_mut(self, accounting: &mut SupervisorAccounting) -> &mut Usage {
		let kind = match self {
			Self::Gate => CallKind::Gate,
			Self::Authorize => CallKind::Authorize,
			Self::Condense => CallKind::Condense,
			Self::Recall => CallKind::Recall,
			Self::Resolve => CallKind::Resolve,
			Self::Plan => CallKind::Plan,
			Self::Distill => CallKind::Distill,
			Self::Retention => CallKind::Retention,
			Self::Evolution => CallKind::Evolution,
			Self::Hindsight => CallKind::Hindsight,
			Self::Skills => return accounting.seams.entry(Seam::Skills).or_default(),
			Self::Capabilities => return accounting.seams.entry(Seam::Capabilities).or_default(),
			Self::Compression => return accounting.seams.entry(Seam::Compression).or_default(),
		};
		accounting.kinds.entry(kind).or_default()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Runs,
	Pass,
	Fail,
	Stall,
	RefutedGaps,
	Checked,
	Blocked,
	Cached,
	Unavailable,
	SkippedJudge,
	Condensed,
	SavedTokens,
	Items,
	Used,
	Lessons,
	Experiences,
	Orientation,
	PlanCalls,
	Skips,
	Applied,
	DecisionUnavailable,
	Replaced,
	Consolidated,
	Archived,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MechanicUsage {
	#[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
	pub reported_findings: BTreeSet<String>,
	pub chat_calls: u64,
	pub decision_calls: u64,
	pub wall_blocking_ms: u64,
	pub wall_background_ms: u64,
	pub api_ms: u64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub reasoning_tokens: u64,
	pub cost: f64,
	pub outcome: BTreeMap<Outcome, u64>,
}

impl MechanicUsage {
	fn include(&mut self, usage: &Usage) {
		self.reported_findings.extend(usage.reported_findings.iter().cloned());
		self.api_ms += usage.api_time_ms;
		self.input_tokens += usage.input_tokens;
		self.output_tokens += usage.output_tokens;
		self.reasoning_tokens += usage.reasoning_tokens;
		self.cost += usage.cost;
		for (&outcome, &count) in &usage.outcome {
			*self.outcome.entry(outcome).or_default() += count;
		}
	}
	pub fn is_active(&self) -> bool {
		!self.reported_findings.is_empty()
			|| self.chat_calls > 0
			|| self.decision_calls > 0
			|| self.wall_blocking_ms > 0
			|| self.wall_background_ms > 0
			|| self.api_ms > 0
			|| self.input_tokens > 0
			|| self.output_tokens > 0
			|| self.reasoning_tokens > 0
			|| self.cost > 0.0
			|| self.outcome.values().any(|count| *count > 0)
	}
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
	pub chat_calls: u64,
	pub decision_calls: u64,
	pub wall_blocking_ms: u64,
	/// Pre-union journals cannot reconstruct overlapping background intervals.
	pub wall_background_ms: Option<u64>,
	pub api_ms: u64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub reasoning_tokens: u64,
	pub cost: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SupervisorReport {
	pub mechanics: BTreeMap<Mechanic, MechanicUsage>,
	pub totals: Totals,
	pub turn_time: TurnTime,
}

impl SupervisorReport {
	pub fn for_session(info: &crate::session::SessionInfo) -> Self {
		let accounting = &info.supervisor_accounting;
		let mut report = Self {
			turn_time: accounting.turn_time.clone(),
			..Default::default()
		};
		for (&kind, usage) in &accounting.kinds {
			if let Some(mechanic) = Mechanic::from_kind(kind) {
				let row = report.mechanics.entry(mechanic).or_default();
				row.chat_calls += usage.calls;
				row.wall_blocking_ms = usage.blocking_wall_time_ms;
				row.wall_background_ms = usage.background_wall_time_ms;
				row.include(usage);
			}
		}
		for (&seam, usage) in &accounting.seams {
			// Mechanic timers include their decision calls. A standalone seam also
			// supplies wall time when loading a journal that predates mechanic unions.
			let row = report
				.mechanics
				.entry(Mechanic::from_seam(seam))
				.or_insert_with(|| MechanicUsage {
					wall_blocking_ms: usage.blocking_wall_time_ms,
					wall_background_ms: usage.background_wall_time_ms,
					..Default::default()
				});
			row.decision_calls += usage.calls;
			row.include(usage);
		}
		// Authorization already has a persisted per-session authority.
		let authorizer = report.mechanics.entry(Mechanic::Authorize).or_default();
		for (outcome, count) in [
			(Outcome::Checked, info.authorization.checked),
			(Outcome::Blocked, info.authorization.blocked),
			(Outcome::Cached, info.authorization.cached),
			(Outcome::Unavailable, info.authorization.unavailable),
		] {
			if count > 0 {
				authorizer.outcome.insert(outcome, count);
			}
		}
		report.mechanics.retain(|_, usage| usage.is_active());
		for usage in report.mechanics.values() {
			report.totals.chat_calls += usage.chat_calls;
			report.totals.decision_calls += usage.decision_calls;
			report.totals.api_ms += usage.api_ms;
			report.totals.input_tokens += usage.input_tokens;
			report.totals.output_tokens += usage.output_tokens;
			report.totals.reasoning_tokens += usage.reasoning_tokens;
			report.totals.cost += usage.cost;
		}
		report.totals.wall_blocking_ms = accounting.blocking_wall_time_ms;
		report.totals.wall_background_ms = accounting.background_wall_time_ms;
		report
	}
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
