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

use anyhow::Result;
use regex::Regex;

/// Parser for extracting data from layer outputs using regex patterns
pub struct PatternParser;

impl PatternParser {
	/// Parse items from text using a regex pattern
	/// Returns vector of captured groups
	pub fn parse_items(text: &str, pattern: &str) -> Result<Vec<String>> {
		let regex = Regex::new(pattern)?;
		let mut items = Vec::new();

		for cap in regex.captures_iter(text) {
			// Get first capture group (or full match if no groups)
			if let Some(matched) = cap.get(1).or_else(|| cap.get(0)) {
				items.push(matched.as_str().to_string());
			}
		}

		Ok(items)
	}

	/// Check if text matches a pattern
	pub fn matches(text: &str, pattern: &str) -> Result<bool> {
		let regex = Regex::new(pattern)?;
		Ok(regex.is_match(text))
	}

	/// Extract first match from text
	pub fn extract_first(text: &str, pattern: &str) -> Result<Option<String>> {
		let regex = Regex::new(pattern)?;
		Ok(regex
			.captures(text)
			.and_then(|cap| cap.get(1).or_else(|| cap.get(0)))
			.map(|m| m.as_str().to_string()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_items() {
		let text = "SUBGOAL 1: Add dependency\nSUBGOAL 2: Create module";
		let pattern = r"SUBGOAL \d+: (.*)";
		let items = PatternParser::parse_items(text, pattern).unwrap();

		assert_eq!(items.len(), 2);
		assert_eq!(items[0], "Add dependency");
		assert_eq!(items[1], "Create module");
	}

	#[test]
	fn test_matches() {
		let text = "VALID: All checks passed";
		assert!(PatternParser::matches(text, r"VALID").unwrap());
		assert!(!PatternParser::matches(text, r"INVALID").unwrap());
	}

	#[test]
	fn test_extract_first() {
		let text = "Score: 8.5 out of 10";
		let result = PatternParser::extract_first(text, r"Score: ([\d.]+)").unwrap();
		assert_eq!(result, Some("8.5".to_string()));
	}
}
