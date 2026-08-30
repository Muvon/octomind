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

fn cache_message(role: &str, content: &str, cached: bool) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		cached,
		cache_ttl: cached.then(|| "stale".to_string()),
		..Default::default()
	}
}

fn content_marker_indices(messages: &[crate::session::Message]) -> Vec<usize> {
	messages
		.iter()
		.enumerate()
		.filter(|(_, message)| message.role != "system" && message.cached)
		.map(|(index, _)| index)
		.collect()
}

#[test]
fn compression_markers_keep_anchor_and_end_after_skill_and_note_reinjection() {
	let mut messages = vec![
		cache_message("system", "system", true),
		cache_message("assistant", "unchanged welcome anchor", false),
		cache_message("user", "<skill name=\"rust\">rules</skill>", true),
		cache_message("assistant", "compressed summary", true),
		cache_message("user", "<continuation>resume</continuation>", true),
		cache_message("user", "<pay-attention>re-anchor</pay-attention>", false),
	];

	align_compression_cache_markers(&mut messages, 1, 3, true);

	assert_eq!(content_marker_indices(&messages), vec![1, 5]);
	assert_eq!(messages[1].cache_ttl.as_deref(), Some("1h"));
	assert!(
		!messages[2].cached,
		"re-injected skill is between boundaries"
	);
	assert!(
		!messages[3].cached,
		"summary is covered by the final boundary"
	);
	assert!(
		!messages[4].cached,
		"stale pre-reinjection end marker is cleared"
	);
	assert!(messages[5].cached, "final current state gets marker #2");
}

#[test]
fn compression_with_system_anchor_uses_both_content_marker_slots() {
	let mut messages = vec![
		cache_message("system", "system anchor", true),
		cache_message("assistant", "compressed summary", false),
		cache_message("user", "<continuation>resume</continuation>", false),
	];

	align_compression_cache_markers(&mut messages, 0, 1, true);

	assert!(messages[0].cached, "system cache marker remains intact");
	assert_eq!(content_marker_indices(&messages), vec![1, 2]);
	assert_eq!(messages[1].cache_ttl, None, "new summary uses normal TTL");
}

#[test]
fn compression_clears_content_markers_for_non_caching_models() {
	let mut messages = vec![
		cache_message("system", "system", true),
		cache_message("assistant", "anchor", true),
		cache_message("assistant", "summary", true),
		cache_message("user", "continuation", true),
	];

	align_compression_cache_markers(&mut messages, 1, 2, false);

	assert!(content_marker_indices(&messages).is_empty());
	assert!(messages[0].cached, "system marker is managed separately");
}

#[test]
fn continuation_detection_ignores_ordinary_messages() {
	assert!(!is_continuation_message("fix the parser"));
	assert!(!is_continuation_message(""));
	// A mention of the tag mid-message is not a wrapper.
	assert!(!is_continuation_message("talk about <continuation> tags"));

	assert!(is_continuation_message("<continuation>\nbody"));
	// Leading whitespace/newlines still count — the wrapper may be re-indented.
	assert!(is_continuation_message("\n  <continuation>\nbody"));
}

#[test]
fn built_wrapper_round_trips_through_the_extractor() {
	let intent = "add retry logic to the uploader";
	let wrapper = build_continuation_content(None, Some(intent), None, false);
	assert!(is_continuation_message(&wrapper));
	assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
	assert!(!wrapper.contains("execution plan is already active"));

	// With an active plan the wrapper gains the continue-the-plan note and
	// the task must still round-trip through the extractor.
	let wrapper = build_continuation_content(None, Some(intent), None, true);
	assert!(wrapper.contains("execution plan is already active"));
	assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
}

#[test]
fn pact_continuation_separates_contextual_request_from_validated_frontier() {
	let summary = CompressionSummary {
		folded_units: vec![super::super::schema::FoldedUnit {
				text: "Continue monitoring the 50-case benchmark; monitor mon-debabfb8 is already running."
					.to_string(),
				kind: "next_action".to_string(),
				status: "tentative".to_string(),
				refs: vec!["b:frontier".to_string()],
			}],
		..Default::default()
	};
	let action = select_continuation_action(&summary, true);
	let wrapper =
		build_continuation_content(None, Some("Should work now"), action.as_deref(), false);

	assert_eq!(
		extract_continuation_task(&wrapper).as_deref(),
		Some("Should work now"),
		"runtime task identity must remain the exact user request"
	);
	assert!(wrapper.contains(
			"<task>\nContinue monitoring the 50-case benchmark; monitor mon-debabfb8 is already running.\n</task>"
		));
	assert!(!wrapper.contains("<task>\nShould work now\n</task>"));
}

#[test]
fn pact_continuation_falls_back_to_pending_open_loop_over_completed_request() {
	let summary = CompressionSummary {
		folded_units: vec![
			super::super::schema::FoldedUnit {
				text: "Model swap completed; config verified on box.".to_string(),
				kind: "outcome".to_string(),
				status: "established".to_string(),
				refs: vec!["b:done".to_string()],
			},
			super::super::schema::FoldedUnit {
				text: "Proposed fix pending approval: catch the validation error.".to_string(),
				kind: "open_loop".to_string(),
				status: "pending".to_string(),
				refs: vec!["b:loop".to_string()],
			},
		],
		..Default::default()
	};
	let action = select_continuation_action(&summary, true);
	let wrapper =
		build_continuation_content(None, Some("disable plan also"), action.as_deref(), false);

	assert!(wrapper
		.contains("<task>\nProposed fix pending approval: catch the validation error.\n</task>"));
	assert!(!wrapper.contains("<task>\ndisable plan also\n</task>"));
}

#[test]
fn fallback_wrapper_carries_no_extractable_intent() {
	// Without a real user ask the wrapper holds only the placeholder, which
	// must not propagate as if it were the active task.
	let wrapper = build_continuation_content(None, None, None, false);
	assert!(wrapper.contains(CONTINUATION_FALLBACK_INTENT));
	assert_eq!(extract_continuation_task(&wrapper), None);
}

#[test]
fn extract_returns_none_for_non_wrappers_and_malformed_tags() {
	assert_eq!(extract_continuation_task("plain user message"), None);
	// Wrapper without a task block.
	assert_eq!(extract_continuation_task("<continuation>\nno task"), None);
	// Unclosed task block.
	assert_eq!(
		extract_continuation_task("<continuation>\n<task>\nhalf"),
		None
	);
	// Empty task block.
	assert_eq!(
		extract_continuation_task("<continuation>\n<task></task>"),
		None
	);
}

#[test]
fn extract_trims_and_keeps_multiline_intent() {
	let wrapper = "<continuation>\n<task>\n  first line\n  second line  \n</task>\n</continuation>";
	assert_eq!(
		extract_continuation_task(wrapper).as_deref(),
		Some("first line\n  second line")
	);
}

#[test]
fn extract_handles_multibyte_intent_without_panicking() {
	let intent = "почини парсер 日本語";
	let wrapper = build_continuation_content(None, Some(intent), None, false);
	assert_eq!(extract_continuation_task(&wrapper).as_deref(), Some(intent));
}

#[test]
fn continuation_round_trips_exact_previous_assistant_response() {
	let previous = "  Exact answer\nwith formatting and trailing space ";
	let request = "  exact follow-up\nwith trailing space ";
	let wrapper = build_continuation_content(Some(previous), Some(request), None, false);
	assert_eq!(
		extract_previous_assistant_response(&wrapper).as_deref(),
		Some(previous)
	);
	assert!(wrapper.contains(&format!("<request>{request}</request>")));
}
