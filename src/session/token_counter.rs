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

// Token counting utilities

use tiktoken_rs::{cl100k_base, CoreBPE};
use std::sync::OnceLock;

// Global tokenizer instance - created once and reused
static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();

// Get or initialize the global tokenizer instance
fn get_tokenizer() -> &'static CoreBPE {
	TOKENIZER.get_or_init(|| {
		cl100k_base().unwrap_or_else(|_| {
			// Fallback - this shouldn't happen in practice
			panic!("Failed to initialize tokenizer")
		})
	})
}

// Simple token counter that uses tiktoken to estimate token counts
pub fn estimate_tokens(text: &str) -> usize {
	// Use the cached global tokenizer
	let tokenizer = get_tokenizer();
	let tokens = tokenizer.encode_ordinary(text);
	tokens.len()
}

// Estimate tokens for a full message list
pub fn estimate_message_tokens(messages: &[crate::session::Message]) -> usize {
	let mut total = 0;

	for msg in messages {
		// Add ~4 tokens for role
		total += 4;

		// Add content tokens
		total += estimate_tokens(&msg.content);
	}

	// Add some overhead for message formatting
	total += messages.len() * 2;

	total
}
