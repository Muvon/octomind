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

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn agents_file_constant_names_the_standard_instructions_file() {
	assert_eq!(AGENTS_FILE, "AGENTS.md");
}

#[test]
fn format_number_zero_and_small_values_stay_plain() {
	assert_eq!(format_number(0), "0");
	assert_eq!(format_number(1), "1");
	assert_eq!(format_number(999), "999");
}

#[test]
fn format_number_low_thousands_use_one_decimal() {
	assert_eq!(format_number(1_000), "1K");
	assert_eq!(format_number(1_500), "1.5K");
	assert_eq!(format_number(2_000), "2K");
	assert_eq!(format_number(9_500), "9.5K");
}

#[test]
fn format_number_high_thousands_use_whole_k() {
	assert_eq!(format_number(10_000), "10K");
	assert_eq!(format_number(999_999), "999K");
}

#[test]
fn format_number_millions_use_one_decimal_then_whole() {
	assert_eq!(format_number(1_000_000), "1M");
	assert_eq!(format_number(1_500_000), "1.5M");
	assert_eq!(format_number(2_000_000), "2M");
	assert_eq!(format_number(10_000_000), "10M");
	assert_eq!(format_number(999_999_999), "999M");
}

#[test]
fn format_number_billions_use_one_decimal() {
	assert_eq!(format_number(1_000_000_000), "1B");
	assert_eq!(format_number(1_500_000_000), "1.5B");
	assert_eq!(format_number(2_500_000_000), "2.5B");
}

#[tokio::test]
async fn get_initial_messages_without_agents_file_returns_only_welcome() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");

	let messages = get_initial_messages(&config, "assistant", dir.path())
		.await
		.expect("initial messages");

	assert_eq!(messages.len(), 1);
	assert_eq!(messages[0].role, "assistant");
	assert!(
		messages[0].content.starts_with("Hello! Ready to code"),
		"unexpected welcome: {}",
		messages[0].content
	);
	assert!(!messages[0].cached);
}

#[tokio::test]
async fn get_initial_messages_wraps_agents_file_as_user_instructions() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	std::fs::write(dir.path().join(AGENTS_FILE), "Do things carefully.").expect("write AGENTS.md");

	let messages = get_initial_messages(&config, "assistant", dir.path())
		.await
		.expect("initial messages");

	assert_eq!(messages.len(), 2);
	assert_eq!(messages[0].role, "assistant");
	assert_eq!(messages[1].role, "user");
	assert!(
		messages[1].content.starts_with("<instructions>\n"),
		"not wrapped: {}",
		messages[1].content
	);
	assert!(messages[1].content.ends_with("\n</instructions>"));
	assert!(messages[1].content.contains("Do things carefully."));
}

#[tokio::test]
async fn get_initial_messages_skips_whitespace_only_agents_file() {
	let config = template_config();
	let dir = tempfile::tempdir().expect("tempdir");
	std::fs::write(dir.path().join(AGENTS_FILE), "   \n").expect("write AGENTS.md");

	let messages = get_initial_messages(&config, "assistant", dir.path())
		.await
		.expect("initial messages");

	assert_eq!(messages.len(), 1, "empty instructions must be skipped");
	assert_eq!(messages[0].role, "assistant");
}
