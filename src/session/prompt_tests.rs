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
use crate::session::{CompressionKind, CompressionStats};

#[test]
fn zero_compressions_leaves_prompt_unchanged() {
	let original = "You are a helpful assistant.".to_string();
	let mut prompt = original.clone();
	add_compression_hints_to_prompt(&mut prompt, &CompressionStats::default());
	assert_eq!(prompt, original);
}

#[test]
fn compression_hint_appends_xml_block() {
	let mut stats = CompressionStats::default();
	stats.add_compression(CompressionKind::Conversation, 10, 5_000);
	stats.add_compression(CompressionKind::Task, 4, 2_000);

	let mut prompt = "base prompt".to_string();
	add_compression_hints_to_prompt(&mut prompt, &stats);

	assert!(prompt.starts_with("base prompt"));
	assert!(prompt.contains("<context_compression>"));
	assert!(prompt.ends_with("</context_compression>"));
}

#[test]
fn compression_hint_is_byte_stable_across_resumes() {
	let mut stats = CompressionStats::default();
	stats.add_compression(CompressionKind::Phase, 5, 10_000);
	let mut prompt = "base prompt".to_string();
	add_compression_hints_to_prompt(&mut prompt, &stats);
	let first = prompt.clone();

	// A later resume after more compressions must not touch the cached prefix.
	stats.add_compression(CompressionKind::Conversation, 8, 3_000);
	add_compression_hints_to_prompt(&mut prompt, &stats);
	add_compression_hints_to_prompt(&mut prompt, &stats);

	assert_eq!(prompt, first);
	assert_eq!(prompt.matches("<context_compression").count(), 1);
}

#[test]
fn compression_hint_replaces_stacked_legacy_blocks() {
	let mut stats = CompressionStats::default();
	stats.add_compression(CompressionKind::Conversation, 10, 5_000);
	let legacy = |n: u32| {
		format!("\n\n<context_compression status=\"active\" compressions=\"{n}\" tokens_saved=\"1\" reduction=\"1.0%\">\nold\n</context_compression>")
	};
	let mut prompt = format!("base prompt{}{}{}", legacy(1), legacy(2), legacy(2));

	add_compression_hints_to_prompt(&mut prompt, &stats);

	let mut expected = "base prompt".to_string();
	add_compression_hints_to_prompt(&mut expected, &stats);
	assert_eq!(prompt, expected);
}

fn template_config() -> crate::config::Config {
	let mut config: crate::config::Config =
		toml::from_str(include_str!("../../config-templates/default.toml")).expect("parse default config template");
	config.build_role_map();
	config
}

#[tokio::test]
async fn commit_attribution_is_appended_only_when_configured() {
	let dir = tempfile::tempdir().expect("tempdir");
	let mut config = template_config();
	// `task_refiner` has no configured tools.
	let prompt = create_system_prompt(dir.path(), &config, "task_refiner").await;
	assert!(prompt.contains(&format!("{}\n</commit-attribution>", config.commit_attribution)));

	config.commit_attribution = "  ".to_string();
	let prompt = create_system_prompt(dir.path(), &config, "task_refiner").await;
	assert!(!prompt.contains("<commit-attribution>"));
}

#[tokio::test]
async fn scheduling_guidance_matches_enabled_tools_and_focuses_on_tap() {
	let dir = tempfile::tempdir().expect("tempdir");
	for tools in [vec![], vec!["tap"], vec!["schedule"], vec!["schedule", "tap"]] {
		let mut config = template_config();
		let role = "schedule-prompt-test";
		config.roles.push(crate::config::Role {
			name: role.to_string(),
			config: config.get_role_config_struct("task_refiner").clone(),
			mcp: crate::config::RoleMcpConfig::with_server_refs(vec!["orchestration".to_string()]),
		});
		config.mcp.servers = if tools.is_empty() {
			Vec::new()
		} else {
			vec![crate::config::McpServerConfig::builtin(
				"orchestration",
				30,
				tools.iter().map(|name| name.to_string()).collect(),
			)]
		};
		config.build_role_map();
		let id = format!("schedule-prompt-{}", uuid::Uuid::new_v4());
		let prompt =
			crate::session::context::with_session_id(id.clone(), create_system_prompt(dir.path(), &config, role)).await;
		assert_eq!(
			prompt.contains("<scheduling>"),
			tools.contains(&"schedule"),
			"{tools:?}"
		);
		assert_eq!(
			prompt.contains("schedule the tap call itself"),
			tools.contains(&"schedule") && tools.contains(&"tap"),
			"{tools:?}"
		);
		if tools.contains(&"schedule") {
			assert!(prompt.contains("exactly one of message or tool_call"));
			assert!(prompt.contains("normal authorization and guardrails apply"));
			assert!(prompt.contains("Results/errors return to your inbox"));
		}
		if tools.contains(&"schedule") && tools.contains(&"tap") {
			assert!(prompt.contains("Discover the role before scheduling"));
			assert!(prompt.contains("Include session to resume"));
			assert!(prompt.contains("do not poll it"));
		}
		crate::session::context::cleanup_session(&id);
	}
}
