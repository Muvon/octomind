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

#[test]
fn default_anchor_is_empty() {
	let a = Anchor::default();
	assert!(a.is_empty());
	assert_eq!(a.compactions_folded, 0);
	assert_eq!(a.last_compacted_at, 0);
}

#[test]
fn extend_replaces_intent_when_supplied() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			intent: Some("Add feature X".to_string()),
			..Default::default()
		},
		100,
	);
	// No intent in the update — the existing one is kept.
	a.extend(AnchorUpdate::default(), 150);
	assert_eq!(a.intent, "Add feature X");
	// A supplied intent replaces (pivot sanctioned by the supplier).
	a.extend(
		AnchorUpdate {
			intent: Some("Now do feature Y instead".to_string()),
			..Default::default()
		},
		200,
	);
	assert_eq!(a.intent, "Now do feature Y instead");
	assert_eq!(a.compactions_folded, 3);
	assert_eq!(a.last_compacted_at, 200);
}

#[test]
fn extend_dedupes_appendable_lists() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			decisions: vec!["use postgres".to_string(), "use redis".to_string()],
			..Default::default()
		},
		0,
	);
	a.extend(
		AnchorUpdate {
			decisions: vec!["use redis".to_string(), "use kafka".to_string()],
			..Default::default()
		},
		0,
	);
	assert_eq!(a.decisions, vec!["use postgres", "use redis", "use kafka"]);
}

#[test]
fn extend_replaces_next_steps() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			next_steps: vec!["write tests".to_string()],
			..Default::default()
		},
		0,
	);
	a.extend(
		AnchorUpdate {
			next_steps: vec!["ship it".to_string()],
			..Default::default()
		},
		0,
	);
	// Latest wins — old next-steps go stale.
	assert_eq!(a.next_steps, vec!["ship it"]);
}

#[test]
fn extend_skips_empty_strings() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			intent: Some("   ".to_string()),
			changes_made: vec!["".to_string(), "  ".to_string(), "real change".to_string()],
			..Default::default()
		},
		0,
	);
	assert!(a.intent.is_empty());
	assert_eq!(a.changes_made, vec!["real change"]);
}

#[test]
fn to_xml_renders_only_present_sections() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			intent: Some("Refactor auth".to_string()),
			decisions: vec!["use JWT".to_string()],
			..Default::default()
		},
		0,
	);
	let xml = a.to_xml();
	assert!(xml.contains("<intent>Refactor auth</intent>"));
	assert!(xml.contains("<decisions>"));
	assert!(xml.contains("<decision>use JWT</decision>"));
	// Empty sections are skipped.
	assert!(!xml.contains("<errors_seen"));
	assert!(!xml.contains("<next_steps"));
}

#[test]
fn empty_anchor_renders_to_short_string() {
	let a = Anchor::default();
	let xml = a.to_xml();
	assert!(xml.is_empty() || xml.len() < 50);
}

#[test]
fn json_round_trip_preserves_state() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			intent: Some("test intent".to_string()),
			decisions: vec!["d1".to_string()],
			file_refs: vec!["a/b.rs".to_string()],
			..Default::default()
		},
		42,
	);
	let json = serde_json::to_string(&a).expect("serialize");
	let restored: Anchor = serde_json::from_str(&json).expect("deserialize");
	assert_eq!(restored.intent, "test intent");
	assert_eq!(restored.decisions, vec!["d1"]);
	assert_eq!(restored.file_refs, vec!["a/b.rs"]);
	assert_eq!(restored.last_compacted_at, 42);
	assert_eq!(restored.compactions_folded, 1);
}

#[test]
fn task_sig_is_deterministic_and_trim_insensitive() {
	assert_eq!(task_sig("fix the login bug"), task_sig("fix the login bug"));
	assert_eq!(
		task_sig("  fix the login bug  "),
		task_sig("fix the login bug")
	);
	assert_ne!(
		task_sig("fix the login bug"),
		task_sig("fix the logout bug")
	);
	// 0 is reserved for "unknown" — no real input may produce it.
	assert_ne!(task_sig(""), 0);
	assert_ne!(task_sig("a"), 0);
}

#[test]
fn task_sig_pins_the_fnv1a_reference_values() {
	// Anchors are serialized and re-read by later processes, so the hash
	// must never drift. FNV-1a 64 reference values (offset basis for "").
	assert_eq!(task_sig("fix the login bug"), 11_728_636_376_826_184_288);
	assert_eq!(task_sig("a"), 12_638_187_200_555_641_996);
	assert_eq!(task_sig(""), 14_695_981_039_346_656_037);
}

#[test]
fn extend_tracks_intent_task_sig_lifecycle() {
	let mut a = Anchor::default();
	// Intent + signature: the signature is recorded.
	a.extend(
		AnchorUpdate {
			intent: Some("Ship the parser".to_string()),
			intent_task_sig: Some(42),
			..Default::default()
		},
		0,
	);
	assert_eq!(a.intent_task_sig, 42);
	// No intent in the update: the existing signature is untouched.
	a.extend(
		AnchorUpdate {
			intent_task_sig: Some(99),
			..Default::default()
		},
		0,
	);
	assert_eq!(a.intent_task_sig, 42);
	// Intent supplied without a signature: resets to 0 (unknown).
	a.extend(
		AnchorUpdate {
			intent: Some("New goal".to_string()),
			..Default::default()
		},
		0,
	);
	assert_eq!(a.intent_task_sig, 0);
	// Whitespace-only intent is not an intent: goal and signature untouched.
	a.extend(
		AnchorUpdate {
			intent: Some("   ".to_string()),
			intent_task_sig: Some(7),
			..Default::default()
		},
		0,
	);
	assert_eq!(a.intent_task_sig, 0);
	assert_eq!(a.intent, "New goal");
}

#[test]
fn extend_with_empty_update_only_bumps_counters() {
	let mut a = Anchor::default();
	a.extend(AnchorUpdate::default(), 77);
	assert!(a.is_empty());
	assert_eq!(a.compactions_folded, 1);
	assert_eq!(a.last_compacted_at, 77);
}

#[test]
fn compactions_folded_saturates_instead_of_overflowing() {
	let mut a = Anchor {
		compactions_folded: u32::MAX,
		..Default::default()
	};
	a.extend(AnchorUpdate::default(), 0);
	assert_eq!(a.compactions_folded, u32::MAX);
}

#[test]
fn is_empty_turns_false_once_any_field_is_set() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			errors_seen: vec!["boom".to_string()],
			..Default::default()
		},
		0,
	);
	assert!(!a.is_empty());
}

#[test]
fn to_xml_reports_fold_count_once_compacted() {
	assert!(!Anchor::default().to_xml().contains("<folds>"));
	let mut a = Anchor::default();
	a.extend(AnchorUpdate::default(), 0);
	a.extend(AnchorUpdate::default(), 0);
	assert!(a.to_xml().contains("<folds>2</folds>"));
}

#[test]
fn to_xml_trims_whitespace_around_list_items() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			decisions: vec!["  use JWT  ".to_string()],
			..Default::default()
		},
		0,
	);
	assert!(a.to_xml().contains("<decision>use JWT</decision>"));
}

#[test]
fn extend_dedup_trims_before_comparing() {
	let mut a = Anchor::default();
	a.extend(
		AnchorUpdate {
			file_refs: vec!["src/main.rs".to_string()],
			..Default::default()
		},
		0,
	);
	a.extend(
		AnchorUpdate {
			file_refs: vec!["  src/main.rs  ".to_string()],
			..Default::default()
		},
		0,
	);
	assert_eq!(a.file_refs, vec!["src/main.rs"]);
}

#[test]
fn default_anchor_serializes_to_empty_object() {
	let json = serde_json::to_string(&Anchor::default()).expect("serialize");
	assert_eq!(json, "{}");
	let restored: Anchor = serde_json::from_str("{}").expect("deserialize");
	assert!(restored.is_empty());
	assert_eq!(restored.compactions_folded, 0);
	assert_eq!(restored.last_compacted_at, 0);
	assert_eq!(restored.intent_task_sig, 0);
}

#[test]
fn anchor_skips_default_fields_when_serializing() {
	let mut a = Anchor::default();
	a.extend(AnchorUpdate::default(), 12);
	let json = serde_json::to_string(&a).expect("serialize");
	assert!(json.contains("\"compactions_folded\":1"));
	assert!(json.contains("\"last_compacted_at\":12"));
	assert!(!json.contains("intent"));
	assert!(!json.contains("decisions"));
}

#[test]
fn anchor_update_deserializes_missing_fields_to_defaults() {
	let update: AnchorUpdate = serde_json::from_str("{}").expect("deserialize");
	assert!(update.intent.is_none());
	assert!(update.intent_task_sig.is_none());
	assert!(update.changes_made.is_empty());
	assert!(update.decisions.is_empty());
	assert!(update.file_refs.is_empty());
	assert!(update.errors_seen.is_empty());
	assert!(update.next_steps.is_empty());
}

#[test]
fn anchor_update_round_trips_all_fields() {
	let update = AnchorUpdate {
		intent: Some("goal".to_string()),
		changes_made: vec!["c".to_string()],
		decisions: vec!["d".to_string()],
		file_refs: vec!["f".to_string()],
		errors_seen: vec!["e".to_string()],
		next_steps: vec!["n".to_string()],
		intent_task_sig: Some(5),
	};
	let json = serde_json::to_string(&update).expect("serialize");
	let restored: AnchorUpdate = serde_json::from_str(&json).expect("deserialize");
	assert_eq!(restored.intent.as_deref(), Some("goal"));
	assert_eq!(restored.intent_task_sig, Some(5));
	assert_eq!(restored.changes_made, vec!["c"]);
	assert_eq!(restored.decisions, vec!["d"]);
	assert_eq!(restored.file_refs, vec!["f"]);
	assert_eq!(restored.errors_seen, vec!["e"]);
	assert_eq!(restored.next_steps, vec!["n"]);
}
