// Copyright 2025 Muvon Un Limited
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

// Smart text summarization for context management

use crate::session::Message;
use anyhow::Result;

/// Smart summarizer for conversation context
pub struct SmartSummarizer;

impl SmartSummarizer {
	/// Create a new smart summarizer
	pub fn new() -> Self {
		Self
	}

	/// Summarize a list of messages intelligently
	/// Preserves technical context, file modifications, and key decisions
	pub fn summarize_messages(&self, messages: &[Message]) -> Result<String> {
		if messages.is_empty() {
			return Ok("No messages to summarize.".to_string());
		}

		// Extract and categorize content from messages
		let mut conversation_flow = Vec::new();
		let mut technical_context = Vec::new();
		let mut file_modifications = Vec::new();
		let mut tool_usage = Vec::new();
		let mut key_decisions = Vec::new();

		for msg in messages {
			match msg.role.as_str() {
				"system" => {
					// Skip system messages - they're preserved separately
					continue;
				}
				"user" => {
					conversation_flow
						.push(format!("User: {}", self.extract_key_points(&msg.content)));

					// Extract technical keywords and context
					if self.contains_technical_content(&msg.content) {
						technical_context.push(self.extract_technical_info(&msg.content));
					}
				}
				"assistant" => {
					conversation_flow.push(format!(
						"Assistant: {}",
						self.extract_key_points(&msg.content)
					));

					// Extract file modification mentions
					if self.contains_file_modifications(&msg.content) {
						file_modifications.push(self.extract_file_info(&msg.content));
					}

					// Extract decisions and solutions
					if self.contains_decisions(&msg.content) {
						key_decisions.push(self.extract_decisions(&msg.content));
					}
				}
				"tool" => {
					// Preserve tool results as they contain important context
					tool_usage.push(self.extract_tool_summary(&msg.content));
				}
				_ => {
					conversation_flow.push(format!(
						"{}: {}",
						msg.role,
						self.extract_key_points(&msg.content)
					));
				}
			}
		}

		// Build comprehensive summary
		let mut summary_parts = Vec::new();

		// Add conversation overview
		if !conversation_flow.is_empty() {
			summary_parts.push("Conversation Overview:".to_string());
			// Take key conversation points (first, last, and some middle points)
			let points_to_include = std::cmp::min(5, conversation_flow.len());
			for (i, point) in conversation_flow.iter().take(points_to_include).enumerate() {
				summary_parts.push(format!("{}. {}", i + 1, point));
			}
		}

		// Add technical context
		if !technical_context.is_empty() {
			summary_parts.push("\nTechnical Context:".to_string());
			for (i, context) in technical_context.iter().take(3).enumerate() {
				summary_parts.push(format!("{}. {}", i + 1, context));
			}
		}

		// Add file modifications
		if !file_modifications.is_empty() {
			summary_parts.push("\nFile Modifications:".to_string());
			for (i, modification) in file_modifications.iter().take(3).enumerate() {
				summary_parts.push(format!("{}. {}", i + 1, modification));
			}
		}

		// Add key decisions
		if !key_decisions.is_empty() {
			summary_parts.push("\nKey Decisions:".to_string());
			for (i, decision) in key_decisions.iter().take(3).enumerate() {
				summary_parts.push(format!("{}. {}", i + 1, decision));
			}
		}

		// Add tool usage
		if !tool_usage.is_empty() {
			summary_parts.push("\nTool Usage:".to_string());
			summary_parts.push(format!(
				"Used {} development tools: {}",
				tool_usage.len(),
				tool_usage.join(", ")
			));
		}

		Ok(summary_parts.join("\n"))
	}

	/// Check if content contains technical information
	fn contains_technical_content(&self, content: &str) -> bool {
		let technical_keywords = [
			"function",
			"class",
			"method",
			"variable",
			"import",
			"export",
			"struct",
			"enum",
			"trait",
			"impl",
			"mod",
			"use",
			"pub",
			"async",
			"await",
			"Result",
			"Error",
			"Ok",
			"Err",
			"config",
			"configuration",
			"setup",
			"install",
			"deploy",
			"api",
			"endpoint",
			"request",
			"response",
			"http",
			"json",
			"database",
			"query",
			"sql",
			"table",
			"index",
			"test",
			"testing",
			"unit test",
			"integration",
			"bug",
			"fix",
			"issue",
			"error",
			"exception",
			"refactor",
			"optimize",
			"performance",
			"memory",
			"security",
			"authentication",
			"authorization",
			"docker",
			"kubernetes",
			"deployment",
			"ci/cd",
		];

		let content_lower = content.to_lowercase();
		technical_keywords
			.iter()
			.any(|keyword| content_lower.contains(keyword))
	}

	/// Check if content contains file modification information
	fn contains_file_modifications(&self, content: &str) -> bool {
		let file_keywords = [
			"created",
			"modified",
			"updated",
			"changed",
			"edited",
			"added",
			"removed",
			"deleted",
			"renamed",
			"moved",
			"file",
			"directory",
			"folder",
			"path",
			".rs",
			".toml",
			".json",
			".yaml",
			".md",
			".txt",
			"src/",
			"tests/",
			"docs/",
			"examples/",
		];

		let content_lower = content.to_lowercase();
		file_keywords
			.iter()
			.any(|keyword| content_lower.contains(keyword))
	}

	/// Check if content contains decisions or solutions
	fn contains_decisions(&self, content: &str) -> bool {
		let decision_keywords = [
			"decided",
			"choose",
			"selected",
			"option",
			"approach",
			"solution",
			"resolved",
			"implemented",
			"strategy",
			"recommend",
			"suggest",
			"best practice",
			"should",
			"will use",
			"going with",
			"final",
			"conclusion",
		];

		let content_lower = content.to_lowercase();
		decision_keywords
			.iter()
			.any(|keyword| content_lower.contains(keyword))
	}

	/// Extract key points from content (first sentence or up to 150 characters)
	fn extract_key_points(&self, content: &str) -> String {
		let sentences: Vec<&str> = content.split('.').collect();
		if let Some(first_sentence) = sentences.first() {
			if first_sentence.len() <= 150 {
				first_sentence.trim().to_string()
			} else {
				format!("{}...", &first_sentence[..147].trim())
			}
		} else if content.len() <= 150 {
			content.trim().to_string()
		} else {
			format!("{}...", &content[..147].trim())
		}
	}

	/// Extract technical information from content
	fn extract_technical_info(&self, content: &str) -> String {
		// Look for code-related patterns and technical terms
		let lines: Vec<&str> = content.lines().collect();
		for line in &lines {
			if line.contains("```")
				|| line.contains("fn ")
				|| line.contains("struct ")
				|| line.contains("impl ")
				|| line.contains("use ")
			{
				return self.extract_key_points(line);
			}
		}
		self.extract_key_points(content)
	}

	/// Extract file information from modification content
	fn extract_file_info(&self, content: &str) -> String {
		// Look for file paths and modification types
		let words: Vec<&str> = content.split_whitespace().collect();
		let mut file_info = Vec::new();

		for window in words.windows(3) {
			if let [action, _, file] = window {
				if ["created", "modified", "updated", "added", "removed"].contains(action)
					&& (file.contains('.') || file.contains('/'))
				{
					file_info.push(format!("{} {}", action, file));
					break;
				}
			}
		}

		if file_info.is_empty() {
			self.extract_key_points(content)
		} else {
			file_info.join(", ")
		}
	}

	/// Extract decisions from content
	fn extract_decisions(&self, content: &str) -> String {
		let sentences: Vec<&str> = content.split('.').collect();
		for sentence in &sentences {
			if self.contains_decisions(sentence) {
				return self.extract_key_points(sentence);
			}
		}
		self.extract_key_points(content)
	}

	/// Extract tool usage summary
	fn extract_tool_summary(&self, content: &str) -> String {
		// Extract tool name or action from tool result
		if content.len() > 50 {
			format!("tool execution ({}...)", &content[..47])
		} else {
			format!("tool execution ({})", content)
		}
	}
}

impl Default for SmartSummarizer {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
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
				timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
			},
			Message {
				role: "assistant".to_string(),
				content: "I'll help you create a JSON parsing function. Let me create a new file for this.".to_string(),
				timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
				cached: false,
				tool_call_id: None,
				name: None,
				tool_calls: None,
			},
		];

		let result = summarizer.summarize_messages(&messages).unwrap();
		assert!(result.contains("function"));
		assert!(result.contains("JSON") || result.contains("json"));
	}
}
