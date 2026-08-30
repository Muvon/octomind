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

fn role_message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[test]
fn xml_mode_embeds_output_spec_and_raw_xml_directive() {
	let session = ChatSession::for_tests(Vec::new());
	let messages = vec![role_message("user", "fix the parser")];
	let (system, user) = build_compression_prompt_xml(&session, &messages, None, false, 2.0);
	assert!(system.contains("<output_format>"));
	assert!(user.contains("Output ONLY raw XML"));
}

#[test]
fn json_mode_cites_attached_schema_instead_of_xml_spec() {
	let session = ChatSession::for_tests(Vec::new());
	let messages = vec![role_message("user", "fix the parser")];
	let (system, user) = build_compression_prompt_json(&session, &messages, None, false, 2.0);
	assert!(!system.contains("<output_format>"));
	assert!(user.contains("structured-output schema attached to this request"));
}

#[test]
fn force_directive_is_emitted_only_for_forced_calls() {
	let session = ChatSession::for_tests(Vec::new());
	let messages = vec![role_message("user", "fix the parser")];
	let (forced_system, _) = build_compression_prompt_json(&session, &messages, None, true, 2.0);
	assert!(forced_system.contains("<forced>"));
	let (plain_system, _) = build_compression_prompt_json(&session, &messages, None, false, 2.0);
	assert!(!plain_system.contains("<forced>"));
}

#[test]
fn prior_knowledge_block_numbers_retained_entries() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.critical_knowledge = vec![
		"never delete migrations".to_string(),
		"deploy via trunk only".to_string(),
	];
	let (_, user) = build_compression_prompt_json(&session, &[], None, false, 2.0);
	assert!(user.contains("<prior_knowledge>"));
	assert!(user.contains("1. never delete migrations"));
	assert!(user.contains("2. deploy via trunk only"));
}

#[test]
fn agent_state_hint_renders_handoff_fields_and_reason_fallback() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.last_self_report = Some(crate::supervisor::detect::SelfReport::Progressing);
	session.last_self_report_handoff = Some(crate::supervisor::detect::SelfReportHandoff {
		focus: "stabilising the flaky retry test".to_string(),
		next: "re-run the suite once".to_string(),
		carry: vec!["retry count lives in config".to_string()],
	});
	let (_, user) = build_compression_prompt_json(&session, &[], None, false, 2.0);
	assert!(user.contains("<agent_state_hint>"));
	assert!(user.contains("state: progressing"));
	assert!(user.contains("focus: stabilising the flaky retry test"));
	assert!(user.contains("next: re-run the suite once"));
	assert!(user.contains("carry: retry count lives in config"));

	// Legacy one-line reports fall back to the trimmed reason string.
	session.last_self_report_handoff = None;
	session.last_self_report_reason = Some("  mid-refactor on the uploader  ".to_string());
	let (_, user) = build_compression_prompt_json(&session, &[], None, false, 2.0);
	assert!(user.contains("focus: mid-refactor on the uploader"));
}

#[test]
fn transcript_renders_user_thinking_and_tool_roles_with_recency() {
	let session = ChatSession::for_tests(Vec::new());
	let mut thinking = role_message("assistant", "working on it");
	thinking.thinking = Some(serde_json::json!({"content": "plan the narrow fix", "tokens": 5}));
	let mut tool = role_message("tool", "tool body");
	tool.name = Some("search".to_string());
	let messages = vec![
		role_message("user", "please fix the parser"),
		thinking,
		tool,
	];
	// Ratio 1.0 keeps the whole transcript inside the recent budget.
	let (_, user) = build_compression_prompt_json(&session, &messages, None, false, 1.0);
	assert!(user.contains("[RECENT] [USER]: please fix the parser"));
	assert!(user.contains("[RECENT] [ASSISTANT THINKING]: plan the narrow fix"));
	assert!(user.contains("[RECENT] [TOOL RESULT: search]: tool body"));
}

#[test]
fn prior_summary_regrown_sections_are_stripped_before_refeeding() {
	let session = ChatSession::for_tests(Vec::new());
	let prior = role_message(
		"assistant",
		"<conversation_summary id=\"c1\">\n<progress>real narrative</progress>\n<file_context>\nSTALE FILE BYTES\n</file_context>\n<recall_index>\nb:gone L1-2 — stale\n</recall_index>\n</conversation_summary>",
	);
	let (_, user) = build_compression_prompt_json(&session, &[prior], None, false, 2.0);
	assert!(user.contains("real narrative"));
	assert!(!user.contains("STALE FILE BYTES"));
	assert!(!user.contains("recall_index"));
}

#[test]
fn old_tool_call_is_reduced_by_ratio_while_recent_stays_exact() {
	let call = serde_json::json!({
		"id": "call-1",
		"function": {
			"name": "view",
			"arguments": {"path": "src/main.rs", "blob": "x".repeat(4000)}
		}
	});
	let old = render_tool_call(&call, false, 8.0);
	assert!(old.contains("[ratio-compressed]"));
	assert!(old.contains("view"));
	assert_eq!(
		render_tool_call(&call, true, 8.0),
		serde_json::to_string(&call).unwrap()
	);
}

#[test]
fn adaptive_preview_returns_short_content_unchanged() {
	assert_eq!(adaptive_preview("tiny payload", 8.0), "tiny payload");
}

#[test]
fn suffix_to_tokens_handles_zero_budget_and_empty_content() {
	assert_eq!(suffix_to_tokens("abc", 0), "");
	assert_eq!(suffix_to_tokens("", 10), "");
	assert_eq!(suffix_to_tokens("tail only", 100), "tail only");
}

#[test]
fn recent_suffix_start_covers_empty_and_single_message_transcripts() {
	assert_eq!(recent_suffix_start(&[], 4.0), 0);
	let single = vec![role_message("user", "hi")];
	assert_eq!(recent_suffix_start(&single, 4.0), 0);
}

#[test]
fn file_references_block_lists_paths_from_transcript_tool_calls() {
	let session = ChatSession::for_tests(Vec::new());
	let mut assistant = role_message("assistant", "");
	assistant.tool_calls = Some(serde_json::json!([
		{"id": "call-1", "function": {"name": "view", "arguments": {"path": "src/main.rs"}}}
	]));
	let (_, user) = build_compression_prompt_json(&session, &[assistant], None, false, 2.0);
	assert!(user.contains("<file_references>"));
	assert!(user.contains("- src/main.rs"));
}

#[test]
fn compressor_instructions_scale_aggressiveness_with_ratio() {
	let session = ChatSession::for_tests(Vec::new());
	let (_, gentle) = build_compression_prompt_json(&session, &[], None, false, 1.5);
	assert!(gentle.contains("gentle"));
	let (_, selective) = build_compression_prompt_json(&session, &[], None, false, 2.0);
	assert!(selective.contains("selective"));
	assert!(selective.contains("50%"));
	let (_, aggressive) = build_compression_prompt_json(&session, &[], None, false, 5.0);
	assert!(aggressive.contains("very aggressive"));
	assert!(aggressive.contains("80%"));
}

#[test]
fn collect_file_refs_supports_structured_and_flat_call_shapes() {
	let mut structured = role_message("assistant", "");
	structured.tool_calls = Some(serde_json::json!([
		{"id": "call-1", "function": {"name": "view", "arguments": {"path": "a.rs"}}}
	]));
	let mut flat = role_message("assistant", "");
	flat.tool_calls = Some(serde_json::json!([
		{"name": "view", "args": {"path": "b.rs"}}
	]));
	let mut ignored = role_message("assistant", "");
	ignored.tool_calls = Some(serde_json::json!([
		{"id": "call-2", "function": {"name": "shell", "arguments": {"path": "c.rs"}}}
	]));
	let mut refs = Vec::new();
	collect_file_refs(&structured, &mut refs);
	collect_file_refs(&flat, &mut refs);
	collect_file_refs(&ignored, &mut refs);
	collect_file_refs(&role_message("user", "no calls"), &mut refs);
	assert!(refs.contains(&"a.rs".to_string()));
	assert!(refs.contains(&"b.rs".to_string()));
	assert!(!refs.iter().any(|r| r.contains("c.rs")));
}

#[tokio::test]
async fn pact_mode_swaps_transcript_for_evidence_set_and_durable_rule() {
	let messages = vec![
		role_message("system", "system prompt"),
		role_message("user", "stabilise the deploy pipeline"),
		role_message("assistant", "investigating flakiness"),
	];
	let mut session = ChatSession::for_tests(messages);
	session.session.info.name = "prompt-pact-unit".to_string();
	session.last_self_report = Some(crate::supervisor::detect::SelfReport::Progressing);
	let pact = super::super::attention::build(&session, 1, 2, 2.0, true, false)
		.await
		.expect("pact context builds");
	let (system, user) = build_compression_prompt_json(
		&session,
		&session.session.messages[1..=2],
		Some(&pact),
		false,
		2.0,
	);
	assert!(user.contains("<evidence_set>"));
	assert!(user.contains("</evidence_set>"));
	assert!(user.contains("<pinned_state>"));
	assert!(!user.contains("<transcript>\n"));
	// The untrusted self-report hint is PACT-grounded upstream, never re-fed raw.
	assert!(!user.contains("<agent_state_hint>"));
	assert!(system.contains("Preserve durable protocol as attributed folded_units"));
}

#[tokio::test]
async fn legacy_mode_keeps_transcript_and_critical_knowledge_rule() {
	let messages = vec![role_message("user", "fix the parser")];
	let session = ChatSession::for_tests(messages);
	let (system, user) = build_compression_prompt_json(
		&session,
		&session.session.messages.clone(),
		None,
		false,
		2.0,
	);
	assert!(user.contains("<transcript>\n"));
	assert!(user.contains("[USER]: fix the parser"));
	assert!(system.contains("Preserve durable protocol in critical_knowledge"));
}
