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

//! Compression ID generation and metrics shared by conversation compression.
//!
//! Plan-driven task/phase/project compression is gone with the removal of the
//! `plan` MCP tool: the external planner owns lifecycle state, and PACT owns
//! context policy. This module retains only the shared compression ID and the
//! `CompressionMetrics` type used by conversation compression and cost display.

use serde::{Deserialize, Serialize};

/// Get the current compression ID for logging/tracking
/// Uses full nanosecond timestamp for uniqueness
pub fn get_compression_id() -> Option<String> {
	let now = std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.unwrap_or_default();

	Some(format!(
		"comp_{}_{}",
		now.as_millis(),
		now.as_nanos() // Full nanoseconds for uniqueness
	))
}

/// Metrics tracking compression effectiveness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionMetrics {
	pub messages_removed: usize,
	pub tokens_saved: u64,
	pub compression_ratio: f64, // Ratio of tokens saved to original tokens
}

impl CompressionMetrics {
	pub fn new(messages_removed: usize, tokens_saved: u64, original_tokens: u64) -> Self {
		let compression_ratio = if original_tokens > 0 {
			tokens_saved as f64 / original_tokens as f64
		} else {
			0.0
		};

		Self {
			messages_removed,
			tokens_saved,
			compression_ratio,
		}
	}
}
