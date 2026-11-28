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

//! The judgements octomind asks its plugins for: which skill, capability or
//! agent fits a request, and which tool results a fold of the history
//! may drop. Each has a rule of octomind's own that stands when no plugin
//! answers. This file holds what is octomind's part of a judgement: the
//! question, and what an answer must reach before octomind acts on it.
//! Whether a judgement is made at all, and by which model, is the plugin's.

use super::protocol::{Judge, JudgeItem, JudgeItems, Judgement, Question};
use serde_json::Value;
use std::collections::BTreeMap;

/// The option of a choice that stands for "nothing in the list fits".
pub const NONE: &str = "none";

/// A skill is activated when the choice gives it at least this probability.
pub const SKILL_ACTIVATE_AT: f64 = 0.6;
/// What a skill activated by a judgement is logged as triggered by.
pub const SKILL_TRIGGER: &str = "evaluate";

/// The capability octomind's own matching picked stays unless the choice
/// gives it less than this. A choice over several candidates spreads the
/// probability, so only a near-zero share counts as a veto.
pub const CAPABILITY_CONFIRM_AT: f64 = 0.05;
/// With no pick of octomind's own, the chosen capability is activated at
/// this probability or above.
pub const CAPABILITY_ACTIVATE_AT: f64 = 0.6;
/// How many of the best-matching capabilities the choice lists.
pub const CAPABILITY_TOP_K: usize = 5;

/// How many of the best-matching agents the choice lists.
pub const TAP_TOP_K: usize = 16;

/// A tool result is dropped from a fold when the probability that later work
/// needs it is below this.
pub const COMPRESSION_KEEP_AT: f64 = 0.5;
/// How much of a tool result the judgement reads: its start and end.
pub const COMPRESSION_SAMPLE_TOKENS: usize = 512;
/// The key the tool results of a fold are listed under.
const COMPRESSION_UNITS_KEY: &str = "units";
const COMPRESSION_QUESTION: &str = "`units[{slot}]` (packet {id}): Does the work that continues after this fold still need the content of this tool interaction? Yes when it holds an unresolved error, a user-facing correction, or facts the pinned task, constraints or plan still depend on; no when it is a completed step whose outcome is already established or a lookup the task has moved past.";

/// One kind of choice among named entries and [`NONE`].
pub struct Roster {
	/// The plugin switches a kind of judgement on or off by this name.
	pub purpose: &'static str,
	id: &'static str,
	question: &'static str,
	none: &'static str,
}

pub const SKILLS: Roster = Roster {
	purpose: "skills",
	id: "skill",
	question: "Which skill applies to the request? Choose the one whose description matches what the request asks for; choose none when no listed skill fits.",
	none: "No skill in this list applies to the request",
};

pub const CAPABILITIES: Roster = Roster {
	purpose: "capabilities",
	id: "capability",
	question: "Which capability applies to the request? Choose the one whose description matches what the request asks for; choose none when no listed capability fits.",
	none: "No capability in this list applies to the request",
};

pub const TAPS: Roster = Roster {
	purpose: "taps",
	id: "specialist",
	question: "Which agent should do the task? Choose the one whose description covers the work the task asks for; choose none when no listed agent fits.",
	none: "No agent in this list fits the task",
};

/// The purpose of the judgement over the tool results of a fold.
pub const COMPRESSION: &str = "compression";

/// A plugin's choice among the entries of a [`Roster`].
pub struct Pick {
	plugin: String,
	purpose: &'static str,
	/// The most probable option: an entry's name, or [`NONE`].
	pub choice: String,
	probabilities: BTreeMap<String, f64>,
}

impl Pick {
	pub fn probability(&self, option: &str) -> f64 {
		self.probabilities.get(option).copied().unwrap_or(0.0)
	}

	/// Tell the plugin that this answer changed what octomind did.
	pub fn applied(&self, session_id: &str) {
		super::record_judged(session_id, &self.plugin, self.purpose, 1);
	}
}

impl Roster {
	/// Ask which of `entries`, each a name with its description, fits
	/// `subject`. `None` when no plugin answered, or when the answer names an
	/// option that was not offered: the caller keeps the outcome of its own rule.
	pub async fn pick<'a>(
		&self,
		session_id: &str,
		subject: &str,
		entries: impl IntoIterator<Item = (&'a str, &'a str)>,
	) -> Option<Pick> {
		let mut options: BTreeMap<String, String> = entries
			.into_iter()
			.map(|(name, description)| (name.to_string(), description.to_string()))
			.collect();
		options.insert(NONE.to_string(), self.none.to_string());
		let question = Question::Choice {
			text: self.question.to_string(),
			options: options.clone(),
		};
		let payload = Judge {
			purpose: self.purpose.to_string(),
			subject: Value::String(subject.to_string()),
			questions: BTreeMap::from([(self.id.to_string(), question)]),
			items: None,
		};
		let mut judged = super::lifecycle::judge_detached(session_id, payload).await?;
		let Some(Judgement::Choice { choice, probabilities }) = judged.judgements.remove(self.id) else {
			return None;
		};
		if !options.contains_key(&choice) {
			crate::log_debug!(
				"Plugin '{}' chose '{choice}' for {}, which was not offered",
				judged.plugin,
				self.purpose
			);
			return None;
		}
		Some(Pick {
			plugin: judged.plugin,
			purpose: self.purpose,
			choice,
			probabilities,
		})
	}
}

/// A plugin's answer on which tool results of a fold later work still needs.
pub struct Needed {
	plugin: String,
	probabilities: BTreeMap<String, f64>,
}

impl Needed {
	/// The probability that the result `id` is still needed. `None` when the
	/// plugin left it out.
	pub fn probability(&self, id: &str) -> Option<f64> {
		self.probabilities.get(id).copied()
	}

	/// Tell the plugin that `count` of these answers changed what octomind did.
	pub fn applied(&self, session_id: &str, count: u64) {
		super::record_judged(session_id, &self.plugin, COMPRESSION, count);
	}
}

/// Ask, for each tool result of a fold, whether the work after the fold still
/// needs it. `kept` is what the fold keeps in any case (task, constraints,
/// plan); `units` are the results, each with its id. `None` when no plugin
/// answered: every result stays as the fold allocated it.
pub async fn still_needed(session_id: &str, kept: Value, units: Vec<JudgeItem>) -> Option<Needed> {
	let payload = Judge {
		purpose: COMPRESSION.to_string(),
		subject: kept,
		questions: BTreeMap::new(),
		items: Some(JudgeItems {
			key: COMPRESSION_UNITS_KEY.to_string(),
			question: COMPRESSION_QUESTION.to_string(),
			items: units,
		}),
	};
	let judged = super::lifecycle::judge_detached(session_id, payload).await?;
	Some(Needed {
		plugin: judged.plugin,
		probabilities: judged
			.judgements
			.into_iter()
			.filter_map(|(id, judgement)| match judgement {
				Judgement::YesNo { yes } => Some((id, yes)),
				Judgement::Choice { .. } => None,
			})
			.collect(),
	})
}

#[cfg(test)]
#[path = "judgement_tests.rs"]
mod tests;
