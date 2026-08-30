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
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn test_summarize_empty_messages() {
	let summarizer = SmartSummarizer::new();
	let result = summarizer.summarize_messages(&[]).unwrap();
	assert_eq!(result, "No messages to summarize.");
}

#[test]
fn test_contains_technical_content() {
	let summarizer = SmartSummarizer::new();

	assert!(summarizer.contains_technical_content("Let's create a new function"));
	assert!(summarizer.contains_technical_content("Update the config file"));
	assert!(summarizer.contains_technical_content("Fix the API endpoint"));
	assert!(!summarizer.contains_technical_content("Hello, how are you?"));
}

#[test]
fn test_contains_file_modifications() {
	let summarizer = SmartSummarizer::new();

	assert!(summarizer.contains_file_modifications("I created a new file"));
	assert!(summarizer.contains_file_modifications("Modified src/main.rs"));
	assert!(summarizer.contains_file_modifications("Updated the .toml configuration"));
	assert!(!summarizer.contains_file_modifications("Just talking about code"));
}

#[test]
fn test_summarize_simple_conversation() {
	let summarizer = SmartSummarizer::new();

	let messages = vec![
		Message {
			role: "user".to_string(),
			content: "Can you help me create a function to parse JSON?".to_string(),
			timestamp: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_secs(),
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
		Message {
			role: "assistant".to_string(),
			content:
				"I'll help you create a JSON parsing function. Let me create a new file for this."
					.to_string(),
			timestamp: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.unwrap()
				.as_secs(),
			cached: false,
			cache_ttl: None,
			tool_call_id: None,
			name: None,
			tool_calls: None,
			images: None,
			videos: None,
			thinking: None,
			id: None,
		},
	];

	let result = summarizer.summarize_messages(&messages).unwrap();
	assert!(result.contains("function"));
	assert!(result.contains("JSON") || result.contains("json"));
}
