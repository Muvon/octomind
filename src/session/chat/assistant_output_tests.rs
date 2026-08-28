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
use crate::providers::ThinkingBlock;

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

fn thinking(content: &str) -> Option<ThinkingBlock> {
	Some(ThinkingBlock {
		content: content.to_string(),
		tokens: 0,
	})
}

#[test]
fn strip_system_tags_removes_block_and_surrounding_whitespace() {
	let content = "before\n\n<system>hidden context</system>\n\nafter";
	assert_eq!(strip_system_tags(content), "before\nafter");
}

#[test]
fn strip_system_tags_removes_multiple_blocks() {
	let content = "a<system>one</system>b<system>two</system>c";
	assert_eq!(strip_system_tags(content), "a\nb\nc");
}

#[test]
fn strip_system_tags_leaves_content_without_tags() {
	assert_eq!(strip_system_tags("plain text"), "plain text");
	assert_eq!(strip_system_tags("  trimmed  "), "trimmed");
}

#[test]
fn strip_system_tags_handles_multiline_and_case_insensitive_tags() {
	let content = "Keep\n<SYSTEM>\nline one\nline two\n</SYSTEM>\nTail";
	assert_eq!(strip_system_tags(content), "Keep\nTail");
}

#[test]
fn strip_system_tags_content_of_only_a_block_becomes_empty() {
	assert_eq!(strip_system_tags("<system>everything hidden</system>"), "");
}

#[test]
fn get_content_to_display_without_thinking_returns_content() {
	assert_eq!(get_content_to_display("Hello world", &None), "Hello world");
}

#[test]
fn get_content_to_display_skips_thinking_prefix() {
	let content = "Let me think.\nHere is the answer";
	let block = thinking("Let me think.");
	assert_eq!(
		get_content_to_display(content, &block),
		"Here is the answer"
	);
}

#[test]
fn get_content_to_display_returns_empty_when_content_is_all_thinking() {
	let block = thinking("only thinking");
	assert_eq!(get_content_to_display("only thinking", &block), "");
}

#[test]
fn get_content_to_display_returns_full_content_when_thinking_is_not_a_prefix() {
	let block = thinking("unrelated reasoning");
	assert_eq!(get_content_to_display("The answer", &block), "The answer");
}

#[test]
fn get_content_to_display_strips_supervisor_self_report_token() {
	let content = "Answer here <sup>{\"state\":\"done\"}</sup>";
	assert_eq!(get_content_to_display(content, &None), "Answer here");
}

#[test]
fn get_content_to_display_keeps_legitimate_superscript_markup() {
	let content = "E = mc<sup>2</sup>";
	assert_eq!(get_content_to_display(content, &None), content);
}

#[test]
fn print_assistant_response_returns_early_on_empty_display_content() {
	let config = template_config();
	// Thinking equal to content → nothing left to display → early return.
	let block = thinking("all thinking");
	print_assistant_response("all thinking", &config, "assistant", &block);
}

#[test]
fn print_assistant_response_plain_text_does_not_panic() {
	let mut config = template_config();
	config.enable_markdown_rendering = false;
	print_assistant_response("plain answer", &config, "assistant", &None);
}
