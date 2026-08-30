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
fn recent_tool_call_is_preserved_without_field_vocabulary() {
	let call = serde_json::json!({
		"id": "opaque-call",
		"function": {
			"name": "domain_neutral_tool",
			"arguments": {
				"totally_unknown_coordinate": "opaque-resource-17",
				"nested": {"arbitrary": [1, 2, 3]}
			}
		}
	});
	assert_eq!(
		render_tool_call(&call, true, 8.0),
		serde_json::to_string(&call).unwrap()
	);
}

#[test]
fn recency_window_scales_with_ratio_and_keeps_active_edge() {
	let messages: Vec<crate::session::Message> = (0..12)
		.map(|index| crate::session::Message {
			role: "assistant".into(),
			content: format!("message {index} {}", "x".repeat(200)),
			..Default::default()
		})
		.collect();
	let gentle = recent_suffix_start(&messages, 2.0);
	let aggressive = recent_suffix_start(&messages, 8.0);
	assert!(gentle <= aggressive);
	assert!(aggressive < messages.len());
}

#[test]
fn adaptive_preview_preserves_both_ends_with_unicode_boundaries() {
	let content = format!("BEGIN-{}-END-ทดสอบ", "middle".repeat(1_000));
	let preview = adaptive_preview(&content, 8.0);
	assert!(preview.starts_with("BEGIN-"));
	assert!(preview.ends_with("END-ทดสอบ"));
	assert!(preview.contains("[ratio-compressed]"));
}

#[test]
fn evidence_set_tag_has_no_literal_escape_characters() {
	assert_eq!(EVIDENCE_SET_TAG, "<evidence_set>");
	assert!(!EVIDENCE_SET_TAG.contains('\\'));
}

#[test]
fn legacy_transcript_includes_assistant_thinking() {
	let session = ChatSession::for_tests(Vec::new());
	let message = crate::session::Message {
		role: "assistant".into(),
		content: "I will use the narrow fix.".into(),
		thinking: Some(serde_json::json!({
			"content": "The broad rewrite would violate the user's scope.",
			"tokens": 11
		})),
		..Default::default()
	};

	let (system, transcript) =
		build_compression_prompt_json(&session, &[message], None, false, 2.0);

	assert!(system.contains("[ASSISTANT THINKING]"));
	assert!(system.contains("untrusted assistant self-report"));
	assert!(transcript.contains("[ASSISTANT]: I will use the narrow fix."));
	assert!(transcript
		.contains("[ASSISTANT THINKING]: The broad rewrite would violate the user's scope."));
	assert!(!transcript.contains("\"tokens\":11"));
}
