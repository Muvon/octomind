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

use crate::providers::ThinkingBlock;

#[test]
fn test_display_thinking_typical_block_does_not_panic() {
	let thinking = ThinkingBlock {
		content: "Considering the options".to_string(),
		tokens: 42,
	};
	display_thinking(&thinking);
}

#[test]
fn test_display_thinking_empty_content_does_not_panic() {
	let thinking = ThinkingBlock {
		content: String::new(),
		tokens: 0,
	};
	display_thinking(&thinking);
}

#[test]
fn test_display_thinking_multiline_content_does_not_panic() {
	let thinking = ThinkingBlock {
		content: "line one\nline two\nline three".to_string(),
		tokens: 128,
	};
	display_thinking(&thinking);
}
