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

// Shared Brave API client functionality

use super::super::{McpToolCall, McpToolResult};
use anyhow::{anyhow, Result};
use serde_json::Value;

// Helper function to extract and validate query parameter - returns MCP-compliant results
pub fn extract_and_validate_query(call: &McpToolCall) -> Result<String, McpToolResult> {
	let query = match call.parameters.get("query") {
		Some(Value::String(q)) => {
			if q.trim().is_empty() {
				return Err(McpToolResult::error(
					call.tool_name.clone(),
					call.tool_id.clone(),
					"Query parameter cannot be empty".to_string(),
				));
			}
			q.clone()
		}
		Some(other) => {
			return Err(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				format!("Query parameter must be a string, got: {}", other),
			));
		}
		None => {
			return Err(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"Missing required parameter: query".to_string(),
			));
		}
	};

	Ok(query)
}

// Helper function to make Brave API requests
pub async fn make_brave_api_request(
	client: &reqwest::Client,
	url: &str,
	api_key: &str,
	search_type: &str,
) -> Result<Value> {
	let request = client
		.get(url)
		.header("Accept", "application/json")
		.header("Accept-Encoding", "gzip")
		.header("X-Subscription-Token", api_key);

	make_brave_api_request_with_builder(request, search_type).await
}

// Helper function to make Brave API requests with a pre-built request
pub async fn make_brave_api_request_with_builder(
	request: reqwest::RequestBuilder,
	search_type: &str,
) -> Result<Value> {
	let response = request.send().await.map_err(|e| {
		anyhow!(
			"Failed to send {} request to Brave Search API: {}",
			search_type,
			e
		)
	})?;

	handle_brave_api_response(response, search_type).await
}

// Helper function to handle Brave API responses
pub async fn handle_brave_api_response(
	response: reqwest::Response,
	search_type: &str,
) -> Result<Value> {
	if !response.status().is_success() {
		let status = response.status();
		let error_text = response
			.text()
			.await
			.unwrap_or_else(|_| "Unknown error".to_string());

		return Err(anyhow!(
			"Brave Search API {} request failed with status {}: {}",
			search_type,
			status,
			error_text
		));
	}

	let search_result: Value = response.json().await.map_err(|e| {
		anyhow!(
			"Failed to parse {} response from Brave Search API: {}",
			search_type,
			e
		)
	})?;

	Ok(search_result)
}

// Create a common API error result
pub fn create_api_error_result(
	error: anyhow::Error,
	search_type: &str,
	tool_name: &str,
	tool_id: &str,
) -> McpToolResult {
	McpToolResult::error(
		tool_name.to_string(),
		tool_id.to_string(),
		format!("Failed to execute {} search: {}", search_type, error),
	)
}
