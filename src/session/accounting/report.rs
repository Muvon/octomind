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

use super::{CallKind, Seam, TurnTime, Usage};
use crate::plugins::protocol;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Every variant stays: session files written by earlier octomind versions name these rows.
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
	Advise,
	Taps,
}

impl Mechanic {
	/// `None` for a name octomind does not know, such as a plugin's own mechanic.
	pub fn from_name(name: &str) -> Option<Self> {
		serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
	}
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
			Self::Advise => "advise",
			Self::Taps => "taps",
		}
	}
	fn from_kind(kind: CallKind) -> Option<Self> {
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
			CallKind::Advise => Self::Advise,
			CallKind::Compression => Self::Compression,
			CallKind::Evaluate => return None,
		})
	}
	fn from_seam(seam: Seam) -> Self {
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
			Seam::Advise => Self::Advise,
			Seam::Taps => Self::Taps,
		}
	}
}

// Every variant stays: session files written by earlier octomind versions name these rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Runs,
	Pass,
	Fail,
	Stall,
	RefutedGaps,
	Readback,
	FormatRepair,
	Reported,
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
	Advised,
	AdviceSkipped,
	AdviceControl,
}

impl Outcome {
	/// The name the outcome has in session files and cost records.
	pub fn name(self) -> String {
		let name = serde_json::to_value(self).expect("an outcome serializes");
		name.as_str().expect("an outcome serializes as a name").to_owned()
	}

	/// `None` for a name octomind does not know, such as one a plugin made up.
	pub fn from_name(name: &str) -> Option<Self> {
		serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
	}
}

/// One row of the report. A plugin's rows are read back into this shape for
/// display, so every field may be missing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MechanicUsage {
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub reported_findings: Vec<String>,
	#[serde(default)]
	pub chat_calls: u64,
	#[serde(default)]
	pub decision_calls: u64,
	#[serde(default)]
	pub wall_blocking_ms: u64,
	#[serde(default)]
	pub wall_background_ms: u64,
	#[serde(default)]
	pub api_ms: u64,
	#[serde(default)]
	pub input_tokens: u64,
	#[serde(default)]
	pub output_tokens: u64,
	#[serde(default)]
	pub reasoning_tokens: u64,
	#[serde(default)]
	pub cost: f64,
	/// Keyed by the serialized name of an [`Outcome`], or by a name of the plugin's own.
	#[serde(default)]
	pub outcome: BTreeMap<String, u64>,
}

impl MechanicUsage {
	fn include(&mut self, usage: &Usage) {
		if usage.replaces_findings() {
			self.reported_findings = usage.reported_findings.clone();
		}
		self.api_ms += usage.api_time_ms;
		self.input_tokens += usage.input_tokens;
		self.output_tokens += usage.output_tokens;
		self.reasoning_tokens += usage.reasoning_tokens;
		self.cost += usage.cost;
		for (&outcome, &count) in &usage.outcome {
			*self.outcome.entry(outcome.name()).or_default() += count;
		}
	}

	/// The row of a plugin that sent only `usage`, no report.
	fn of_plugin_usage(usage: &Usage) -> Self {
		Self {
			chat_calls: usage.calls,
			wall_blocking_ms: usage.blocking_wall_time_ms,
			wall_background_ms: usage.background_wall_time_ms,
			input_tokens: usage.input_tokens,
			output_tokens: usage.output_tokens,
			reasoning_tokens: usage.reasoning_tokens,
			cost: usage.cost,
			..Self::default()
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

impl Totals {
	fn include(&mut self, usage: &MechanicUsage) {
		self.chat_calls += usage.chat_calls;
		self.decision_calls += usage.decision_calls;
		self.api_ms += usage.api_ms;
		self.input_tokens += usage.input_tokens;
		self.output_tokens += usage.output_tokens;
		self.reasoning_tokens += usage.reasoning_tokens;
		self.cost += usage.cost;
	}

	fn include_report(&mut self, other: &protocol::ReportTotals) {
		self.chat_calls += other.chat_calls;
		self.decision_calls += other.decision_calls;
		self.wall_blocking_ms += other.wall_blocking_ms;
		self.wall_background_ms = self
			.wall_background_ms
			.zip(other.wall_background_ms)
			.map(|(a, b)| a + b);
		self.api_ms += other.api_ms;
		self.input_tokens += other.input_tokens;
		self.output_tokens += other.output_tokens;
		self.reasoning_tokens += other.reasoning_tokens;
		self.cost += other.cost;
	}
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SupervisorReport {
	/// One row per mechanic, by name. Octomind's own rows are [`MechanicUsage`];
	/// a plugin's rows are whatever the plugin sent.
	pub mechanics: BTreeMap<String, serde_json::Value>,
	pub totals: Totals,
	pub turn_time: TurnTime,
}

impl SupervisorReport {
	pub fn for_session(info: &crate::session::SessionInfo) -> Self {
		let accounting = &info.supervisor_accounting;
		let mut own: BTreeMap<Mechanic, MechanicUsage> = BTreeMap::new();
		for (&kind, usage) in &accounting.kinds {
			if let Some(mechanic) = Mechanic::from_kind(kind) {
				let row = own.entry(mechanic).or_default();
				row.chat_calls += usage.calls;
				row.wall_blocking_ms = usage.blocking_wall_time_ms;
				row.wall_background_ms = usage.background_wall_time_ms;
				row.include(usage);
			}
		}
		for (&seam, usage) in &accounting.seams {
			// Mechanic timers include their decision calls. A standalone seam also
			// supplies wall time when loading a journal that predates mechanic unions.
			let row = own.entry(Mechanic::from_seam(seam)).or_insert_with(|| MechanicUsage {
				wall_blocking_ms: usage.blocking_wall_time_ms,
				wall_background_ms: usage.background_wall_time_ms,
				..Default::default()
			});
			row.decision_calls += usage.calls;
			row.include(usage);
		}
		own.retain(|_, usage| usage.is_active());
		let mut totals = Totals::default();
		for usage in own.values() {
			totals.include(usage);
		}
		totals.wall_blocking_ms = accounting.blocking_wall_time_ms;
		totals.wall_background_ms = accounting.background_wall_time_ms;
		let mut mechanics: BTreeMap<String, serde_json::Value> = own
			.into_iter()
			.map(|(mechanic, usage)| {
				(
					mechanic.name().to_owned(),
					serde_json::to_value(usage).expect("a mechanic row serializes"),
				)
			})
			.collect();
		if accounting.plugin_reports.is_empty() {
			// A plugin that sends only `usage` stays visible. Its blocking time is
			// already in `accounting.blocking_wall_time_ms`, so only the rest is added.
			for (label, usage) in &accounting.plugin_usage {
				let row = MechanicUsage::of_plugin_usage(usage);
				totals.include(&row);
				totals.wall_background_ms = totals.wall_background_ms.map(|total| total + row.wall_background_ms);
				mechanics.insert(
					label.clone(),
					serde_json::to_value(row).expect("a mechanic row serializes"),
				);
			}
		} else {
			// A report is the plugin's own total for the session. It covers the
			// blocking time that was booked from the plugin's `usage`, so that time
			// is taken out before the report's is added.
			let booked_from_usage: u64 = accounting
				.plugin_usage
				.values()
				.map(|usage| usage.blocking_wall_time_ms)
				.sum();
			totals.wall_blocking_ms = totals.wall_blocking_ms.saturating_sub(booked_from_usage);
		}
		for report in accounting.plugin_reports.values() {
			mechanics.extend(report.mechanics.iter().map(|(name, row)| (name.clone(), row.clone())));
			totals.include_report(&report.totals);
		}
		Self {
			mechanics,
			totals,
			turn_time: accounting.turn_time.clone(),
		}
	}
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
