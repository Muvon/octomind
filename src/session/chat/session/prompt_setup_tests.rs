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

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::CompressionKind;

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

fn system_message(content: &str) -> crate::session::Message {
	crate::session::Message {
		role: "system".to_string(),
		content: content.to_string(),
		..Default::default()
	}
}

#[tokio::test]
async fn resumed_session_with_compressions_gets_hint_appended() {
	let config = template_config();
	let mut session = ChatSession::for_tests(vec![system_message("base prompt")]);
	session.session.info.compression_stats.add_compression(
		CompressionKind::Conversation,
		10,
		5_000,
	);

	setup_system_prompt_and_cache(&mut session, &config, "assistant", false)
		.await
		.expect("setup");

	let first = &session.session.messages[0];
	assert_eq!(first.role, "system");
	assert!(first.content.starts_with("base prompt"));
	assert!(first.content.contains("<context_compression"));
	assert!(first.content.contains("compressions=\"1\""));
}

#[tokio::test]
async fn resumed_session_without_compressions_leaves_prompt_unchanged() {
	let config = template_config();
	let mut session = ChatSession::for_tests(vec![system_message("base prompt")]);

	setup_system_prompt_and_cache(&mut session, &config, "assistant", false)
		.await
		.expect("setup");

	assert_eq!(session.session.messages[0].content, "base prompt");
}

#[tokio::test]
async fn resumed_session_with_non_system_first_message_is_untouched() {
	let config = template_config();
	let mut session = ChatSession::for_tests(vec![crate::session::Message {
		role: "user".to_string(),
		content: "hello".to_string(),
		..Default::default()
	}]);
	session
		.session
		.info
		.compression_stats
		.add_compression(CompressionKind::Task, 4, 2_000);

	setup_system_prompt_and_cache(&mut session, &config, "assistant", false)
		.await
		.expect("setup");

	assert_eq!(session.session.messages[0].content, "hello");
}

#[tokio::test]
async fn fresh_session_non_interactive_builds_cached_system_prompt_and_welcome() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	crate::mcp::set_thread_working_directory(dir.path().to_path_buf());
	let mut session = ChatSession::for_tests(Vec::new());

	setup_system_prompt_and_cache(&mut session, &config, "assistant", false)
		.await
		.expect("setup");

	assert!(session.session.messages.len() >= 2);
	let system = &session.session.messages[0];
	assert_eq!(system.role, "system");
	assert!(!system.content.is_empty());
	assert!(system
		.content
		.contains("You have access to the following tools:"));
	assert!(
		system.cached,
		"anthropic model supports caching → system prompt must be marked cached"
	);
	let welcome = &session.session.messages[1];
	assert_eq!(welcome.role, "assistant");
	assert!(
		welcome.content.starts_with("Hello! Ready to code"),
		"unexpected welcome: {}",
		welcome.content
	);
}

#[tokio::test]
async fn fresh_session_non_interactive_loads_agents_file_as_instructions() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	std::fs::write(dir.path().join("AGENTS.md"), "Project rule: be terse.")
		.expect("write AGENTS.md");
	crate::mcp::set_session_working_directory(dir.path().to_path_buf());
	let mut session = ChatSession::for_tests(Vec::new());

	setup_system_prompt_and_cache(&mut session, &config, "assistant", false)
		.await
		.expect("setup");

	assert_eq!(session.session.messages.len(), 3);
	let instructions = session
		.session
		.messages
		.last()
		.expect("instructions message");
	assert_eq!(instructions.role, "user");
	assert!(
		instructions.content.starts_with("<instructions>\n"),
		"not wrapped: {}",
		instructions.content
	);
	assert!(instructions.content.contains("Project rule: be terse."));
}

#[tokio::test]
async fn fresh_session_interactive_adds_welcome_and_instructions() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	std::fs::write(dir.path().join("AGENTS.md"), "Project rule: be terse.")
		.expect("write AGENTS.md");
	crate::mcp::set_session_working_directory(dir.path().to_path_buf());
	let mut session = ChatSession::for_tests(Vec::new());

	setup_system_prompt_and_cache(&mut session, &config, "assistant", true)
		.await
		.expect("setup");

	assert_eq!(session.session.messages.len(), 3);
	assert_eq!(session.session.messages[0].role, "system");
	assert_eq!(session.session.messages[1].role, "assistant");
	assert!(session.session.messages[1]
		.content
		.starts_with("Hello! Ready to code"));
	let instructions = session
		.session
		.messages
		.last()
		.expect("instructions message");
	assert_eq!(instructions.role, "user");
	assert!(instructions.content.contains("Project rule: be terse."));
}
