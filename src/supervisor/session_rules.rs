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

//! Standing user rules for one session.
//!
//! Per-turn constraints come from the latest request only, so a rule the user
//! set earlier ("from now on, never push to main") stops being recited the
//! moment the next request arrives, and survives a compaction only if the
//! summarizer happens to keep it. Summarizers routinely drop such side
//! constraints. These rules are verbatim user excerpts held in `SessionInfo`
//! instead: recited every turn and pinned through every fold until the user
//! revokes them. The resolver's classifier proposes the delta; this module only
//! admits what the user's own words ground.

use serde::Deserialize;

/// Standing rules held at once. Each one is recited on every turn, so the
/// ledger stays small enough not to dilute the recency slot it occupies.
pub const SESSION_RULES_MAX: usize = 8;
/// A standing rule is a short instruction; a longer excerpt is task prose.
pub const SESSION_RULE_CHARS: usize = 200;
/// New rules admitted from one turn.
const SESSION_RULES_PER_TURN: usize = 4;

/// One proposed revocation: the standing rule exactly as held, plus the user's
/// own words in the current turn that cancel it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Revocation {
	#[serde(default)]
	pub rule: String,
	#[serde(default)]
	pub evidence: String,
}

/// Delta to the session's standing rules, produced from one genuine user turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRulesUpdate {
	pub add: Vec<String>,
	pub revoke: Vec<Revocation>,
}

impl SessionRulesUpdate {
	/// Keep only what the user's words ground: an addition must occur verbatim
	/// in the current request; a revocation must name a standing rule exactly
	/// and quote the current request cancelling it. A model-authored paraphrase
	/// can neither create a rule nor cancel one, and a classifier misfire on a
	/// turn that never mentions a rule cannot drop it.
	pub fn grounded(self, request: &str, standing: &[String]) -> Self {
		let mut add: Vec<String> = Vec::new();
		for candidate in self.add {
			let excerpt = candidate.trim();
			if !excerpt.is_empty()
				&& excerpt.chars().count() <= SESSION_RULE_CHARS
				&& request.contains(excerpt)
				&& !add.iter().any(|existing| existing == excerpt)
			{
				add.push(excerpt.to_string());
				if add.len() == SESSION_RULES_PER_TURN {
					break;
				}
			}
		}
		let mut revoke: Vec<Revocation> = Vec::new();
		for candidate in self.revoke {
			let rule = candidate.rule.trim();
			let evidence = candidate.evidence.trim();
			if standing.iter().any(|existing| existing == rule)
				&& !evidence.is_empty()
				&& evidence.chars().count() <= SESSION_RULE_CHARS
				&& request.contains(evidence)
				&& !revoke.iter().any(|existing| existing.rule == rule)
			{
				revoke.push(Revocation {
					rule: rule.to_string(),
					evidence: evidence.to_string(),
				});
			}
		}
		Self { add, revoke }
	}

	/// Apply to the persisted rules. Revocations run first, so a turn that
	/// replaces a rule lands the replacement. Returns whether the rules changed.
	pub fn apply(&self, rules: &mut Vec<String>) -> bool {
		let previous = rules.clone();
		rules.retain(|rule| {
			!self
				.revoke
				.iter()
				.any(|revocation| &revocation.rule == rule)
		});
		for rule in &self.add {
			if !rules.contains(rule) {
				rules.push(rule.clone());
			}
		}
		if rules.len() > SESSION_RULES_MAX {
			let evicted = rules.len() - SESSION_RULES_MAX;
			crate::log_info!(
				"Session rules over cap {}: evicting {} oldest",
				SESSION_RULES_MAX,
				evicted
			);
			rules.drain(..evicted);
		}
		*rules != previous
	}
}

#[cfg(test)]
#[path = "session_rules_tests.rs"]
mod tests;
