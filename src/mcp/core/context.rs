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

use serde::Deserialize;
use serde_json::json;

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use crate::session::delegation_context::{self, Scope, DEFAULT_LIMIT, MAX_LIMIT, MAX_QUERY_BYTES};

pub(crate) const TOOL_NAME: &str = "context";

pub(crate) fn definition() -> McpFunction {
	McpFunction {
		name: TOOL_NAME.to_owned(),
		freeform: None,
		annotations: Some(rmcp::model::ToolAnnotations::new().read_only(true).open_world(false)),
		description: "Search the authorized conversation of the agents that delegated this work. Use it for earlier user decisions, constraints, and evidence omitted from the brief. Excerpts are labeled by source and ancestor depth.".to_owned(),
		parameters: json!({
			"type": "object",
			"properties": {
				"query": {"type": "string", "minLength": 1, "maxLength": MAX_QUERY_BYTES},
				"scope": {"type": "string", "enum": ["ancestors", "parent", "root"], "default": "ancestors"},
				"limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT}
			},
			"required": ["query"],
			"additionalProperties": false
		}),
	}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
	query: String,
	#[serde(default = "ancestors")]
	scope: Scope,
	#[serde(default = "default_limit")]
	limit: usize,
}

fn ancestors() -> Scope {
	Scope::Ancestors
}

fn default_limit() -> usize {
	DEFAULT_LIMIT
}

pub(crate) async fn execute(call: &McpToolCall) -> McpToolResult {
	let error = |message| McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), message);
	let Some(grant) = delegation_context::current_grant() else {
		return error("Authorized delegation context is unavailable.".to_owned());
	};
	let search: Search = match serde_json::from_value(call.parameters.clone()) {
		Ok(search) => search,
		Err(failure) => return error(format!("Invalid context search: {failure}")),
	};
	let query = search.query.trim().to_owned();
	if query.is_empty() || query.len() > MAX_QUERY_BYTES || !(1..=MAX_LIMIT).contains(&search.limit) {
		return error(
			"context requires a non-empty query within its byte limit and a limit within the advertised range."
				.to_owned(),
		);
	}
	match delegation_context::search(grant, query, search.scope, search.limit).await {
		Ok(result) => match serde_json::to_string(&result) {
			Ok(text) => McpToolResult::success(call.tool_name.clone(), call.tool_id.clone(), text),
			Err(failure) => error(format!("Delegation search result could not be encoded: {failure}")),
		},
		Err(failure) => error(format!("Authorized delegation context unavailable: {failure}")),
	}
}
