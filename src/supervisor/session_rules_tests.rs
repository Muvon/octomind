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

use super::*;

fn strings(values: &[&str]) -> Vec<String> {
	values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn additions_must_be_verbatim_user_excerpts() {
	let update = SessionRulesUpdate {
		add: strings(&[
			"never run cargo locally",
			"always ask before pushing",
			"  never run cargo locally  ",
			"",
		]),
		revoke: Vec::new(),
	}
	.grounded("Fix the parser. From now on, never run cargo locally.", &[]);
	assert_eq!(update.add, strings(&["never run cargo locally"]));
}

#[test]
fn additions_are_bounded_in_length_and_count() {
	let long = "x".repeat(SESSION_RULE_CHARS + 1);
	let request = format!("{long} a b c d e");
	let update = SessionRulesUpdate {
		add: vec![long.clone(), "a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
		revoke: Vec::new(),
	}
	.grounded(&request, &[]);
	assert_eq!(update.add, strings(&["a", "b", "c", "d"]));
}

fn revocation(rule: &str, evidence: &str) -> Revocation {
	Revocation {
		rule: rule.to_string(),
		evidence: evidence.to_string(),
	}
}

#[test]
fn revocations_need_a_held_rule_and_user_evidence() {
	let standing = strings(&["never run cargo locally", "never push to main"]);
	let update = SessionRulesUpdate {
		add: Vec::new(),
		revoke: vec![
			revocation("never run cargo locally", "You can run cargo now"),
			revocation("a rule that never existed", "You can run cargo now"),
			// The misfire this guards: a turn that never mentions the rule.
			revocation("never push to main", "you may push to main"),
			revocation("never push to main", ""),
		],
	}
	.grounded("You can run cargo now.", &standing);
	assert_eq!(
		update.revoke,
		vec![revocation("never run cargo locally", "You can run cargo now")]
	);
}

#[test]
fn apply_revokes_before_adding_and_reports_change() {
	let mut rules = strings(&["never run cargo locally"]);
	let replace = SessionRulesUpdate {
		add: strings(&["run cargo only on the dev box"]),
		revoke: vec![revocation("never run cargo locally", "run cargo only on the dev box")],
	};
	assert!(replace.apply(&mut rules));
	assert_eq!(rules, strings(&["run cargo only on the dev box"]));

	let repeat = SessionRulesUpdate {
		add: strings(&["run cargo only on the dev box"]),
		revoke: Vec::new(),
	};
	assert!(!repeat.apply(&mut rules), "re-adding a held rule is a no-op");
	assert!(!SessionRulesUpdate::default().apply(&mut rules));
}

#[test]
fn apply_evicts_oldest_over_cap() {
	let mut rules: Vec<String> = (0..SESSION_RULES_MAX).map(|i| format!("rule {i}")).collect();
	let update = SessionRulesUpdate {
		add: strings(&["newest rule"]),
		revoke: Vec::new(),
	};
	assert!(update.apply(&mut rules));
	assert_eq!(rules.len(), SESSION_RULES_MAX);
	assert_eq!(rules.first().map(String::as_str), Some("rule 1"));
	assert_eq!(rules.last().map(String::as_str), Some("newest rule"));
}
