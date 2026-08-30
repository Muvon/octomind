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

// Bounded recall over the PACT lossless archive (addressable recall).
//
// Compression replaces drained messages with an attributed summary whose
// folded units cite content-addressed block IDs (`b:<hex>`); the raw messages
// live on in a per-session JSONL archive (see conversation_compression::
// archive). This tool dereferences those IDs back into verbatim messages on
// demand, closing the elastic loop: compression narrows the context under
// pressure, recall re-expands exactly the evidence the model needs.
//
// Recalled content returns as a normal tool result — appended at the tail,
// never rewriting history — so the prompt cache stays intact, and the recalled
// text folds back into the next compression cycle automatically once it stops
// being referenced.

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::chat::conversation_compression::archive;
use anyhow::{anyhow, Result};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Per-call bound: recall stays surgical. Larger needs are served by repeated
/// calls, keeping each response inside the tool-output token cap.
const MAX_BLOCKS_PER_CALL: usize = 2;

pub fn get_recall_function() -> McpFunction {
	McpFunction {
		name: "recall".to_string(),
		description: "Retrieve the verbatim archived messages behind compressed-context block IDs.

After conversation compression, the summary's <folded_state> units cite block IDs like b:1a2b3c4d (refs attribute), and archived tool packets are listed with the same IDs. The raw messages are archived losslessly; this tool returns them exactly as they were.

Use when the summary elided something you now need — exact code, error text, tool output, or user wording. Do not guess elided content; recall it. Max 2 block IDs per call; call again for more."
			.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"ids": {
					"type": "array",
					"items": { "type": "string" },
					"minItems": 1,
					"maxItems": MAX_BLOCKS_PER_CALL,
					"description": "Block IDs to dereference (as cited in <folded_state> refs, e.g. \"b:1a2b3c4d\")"
				}
			},
			"required": ["ids"],
			"additionalProperties": false
		}),
	}
}

pub async fn execute_recall(call: &McpToolCall) -> Result<McpToolResult> {
	let ids: Vec<String> = call
		.parameters
		.get("ids")
		.and_then(|v| v.as_array())
		.map(|arr| {
			arr.iter()
				.filter_map(|v| v.as_str())
				.map(str::to_string)
				.collect()
		})
		.unwrap_or_default();

	if ids.is_empty() {
		return Err(anyhow!("'ids' must be a non-empty array of block IDs"));
	}
	if ids.len() > MAX_BLOCKS_PER_CALL {
		return Err(anyhow!(
			"at most {} block IDs per call — call again for the rest",
			MAX_BLOCKS_PER_CALL
		));
	}

	let session_name = crate::session::context::current_session_id()
		.ok_or_else(|| anyhow!("no active session — recall is only available inside a session"))?;
	let registry = archive::read_session_block_registry(&session_name);
	if registry.is_empty() {
		return Err(anyhow!(
			"no compressed blocks archived for this session yet — recall becomes available after the first compression"
		));
	}

	let unknown: Vec<&str> = ids
		.iter()
		.filter(|id| !registry.contains_key(id.as_str()))
		.map(String::as_str)
		.collect();
	if !unknown.is_empty() {
		return Err(anyhow!(
			"unknown block ID(s): {} — valid IDs are cited in <folded_state> refs",
			unknown.join(", ")
		));
	}

	// Blocks may span several compression cycles, each with its own sidecar
	// index; group the dereferences per index file.
	let mut by_index: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
	for id in &ids {
		let entry = &registry[id.as_str()];
		by_index
			.entry(entry.index_path.clone())
			.or_default()
			.push(id.clone());
	}

	let mut output = String::new();
	for (index_path, block_ids) in by_index {
		let messages = archive::read_blocks(&index_path, &block_ids)?;
		output.push_str(&format!("<recall ids=\"{}\">\n", block_ids.join(" ")));
		for message in &messages {
			output.push_str(&format!("[{}] {}\n", message.role, message.content.trim()));
		}
		output.push_str("</recall>\n");
	}

	Ok(McpToolResult::success(
		call.tool_name.clone(),
		call.tool_id.clone(),
		output,
	))
}

#[cfg(test)]
#[path = "recall_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "recall_tests.rs"]
mod recall_tests;
