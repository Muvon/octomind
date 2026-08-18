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

//! ChatSession surface methods: attachments, message-range removal,
//! compressed-knowledge insertion, compression hints, builder wiring.

use super::*;

fn test_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[test]
fn test_init_params_builder_wiring() {
	let config = test_config();
	let params = SessionInitParams::new(&config, "assistant")
		.with_name("build-test".to_string())
		.with_model("ollama:fake".to_string())
		.with_temperature(0.1)
		.with_max_tokens(2048)
		.with_max_retries(5)
		.with_output_mode("plain".to_string())
		.with_schema(serde_json::json!({"type": "object"}));
	assert_eq!(params.name.as_deref(), Some("build-test"));
	assert_eq!(params.model.as_deref(), Some("ollama:fake"));
	assert_eq!(params.max_retries, Some(5));
	assert!(params.schema.is_some());
}

#[test]
fn test_effective_model_and_counts() {
	let mut session = ChatSession::for_tests(vec![
		message("user", "q1"),
		message("assistant", "a1"),
		message("user", "q2"),
	]);
	assert_eq!(session.get_effective_model(), "anthropic/claude-3-5-sonnet");
	assert_eq!(session.get_message_count(), 3);
	session.invalidate_tool_cache();
}

#[test]
fn test_pending_attachment_take_semantics() {
	let mut session = ChatSession::for_tests(Vec::new());
	assert!(!session.has_pending_image());
	assert!(session.take_pending_image().is_none());
	assert!(!session.has_pending_video());
	assert!(session.take_pending_video().is_none());
}

#[tokio::test]
async fn test_attach_image_from_missing_path_errors() {
	let mut session = ChatSession::for_tests(Vec::new());
	assert!(session
		.attach_image_from_path("/definitely/not/here.png")
		.await
		.is_err());
	assert!(!session.has_pending_image());
	assert!(session
		.attach_video_from_path("/definitely/not/here.mp4")
		.await
		.is_err());
}

#[test]
fn test_remove_messages_in_range() {
	let mut session = ChatSession::for_tests(vec![
		message("user", "m0"),
		message("assistant", "m1"),
		message("user", "m2"),
		message("assistant", "m3"),
	]);
	// Removes start+1..=end: the range anchor at index 0 survives
	let (removed, had_cached) = session
		.remove_messages_in_range(0, 2)
		.expect("range removal");
	assert_eq!(removed, 2);
	assert!(!had_cached);
	assert_eq!(session.get_message_count(), 2);
	assert_eq!(session.session.messages[0].content, "m0");
	assert_eq!(session.session.messages[1].content, "m3");

	// Out-of-bounds and inverted ranges fail instead of silently truncating
	assert!(session.remove_messages_in_range(5, 9).is_err());
	assert!(session.remove_messages_in_range(1, 1).is_err());
}

#[test]
fn test_insert_compressed_knowledge() {
	let mut session = ChatSession::for_tests(vec![message("user", "task")]);
	session
		.insert_compressed_knowledge(0, "critical: build on the box".to_string())
		.expect("insert knowledge");
	assert!(session.get_message_count() >= 2);
}

#[test]
fn test_compression_hint_gating() {
	let mut config = test_config();
	let mut session = ChatSession::for_tests(Vec::new());

	// Hints disabled → never shown
	config.compression.hints_enabled = false;
	assert!(!session.should_show_compression_hint(&config));
	assert!(session.get_compression_hint(&config).is_none());

	// Hints enabled but an empty session is under any pressure threshold
	config.compression.hints_enabled = true;
	assert!(!session.should_show_compression_hint(&config));
}
