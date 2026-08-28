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
fn compression_hint_appends_xml_block_with_count() {
	let mut stats = CompressionStats::default();
	stats.add_compression(CompressionKind::Conversation, 10, 5_000);
	stats.add_compression(CompressionKind::Task, 4, 2_000);

	let mut prompt = "base prompt".to_string();
	add_compression_hints_to_prompt(&mut prompt, &stats);

	assert!(prompt.starts_with("base prompt"));
	assert!(prompt.contains("<context_compression"));
	assert!(prompt.contains("</context_compression>"));
	assert!(prompt.contains("status=\"active\""));
	assert!(prompt.contains("compressions=\"2\""));
	assert!(prompt.contains("tokens_saved=\"7000\""));
}

#[test]
fn compression_hint_reports_reduction_percentage() {
	let mut stats = CompressionStats::default();
	stats.add_compression(CompressionKind::Phase, 5, 10_000);
	// avg_compression_ratio = 10_000 / (10_000 + 10_000) = 0.5 → "50.0%"
	let mut prompt = String::new();
	add_compression_hints_to_prompt(&mut prompt, &stats);
	assert!(prompt.contains("reduction=\"50.0%\""));
}
