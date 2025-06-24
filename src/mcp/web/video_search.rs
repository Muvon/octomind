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

// Video search functionality

use super::super::{McpFunction, McpToolCall, McpToolResult};
use super::api_client::{
	create_api_error_result, extract_and_validate_query, make_brave_api_request,
};
use super::formatters::format_video_results;
use anyhow::Result;
use serde_json::json;

// Define the video_search function for the MCP protocol
pub fn get_video_search_function() -> McpFunction {
	McpFunction {
		name: "video_search".to_string(),
		description: "Search for videos using Brave Search API.

Returns video search results in a token-efficient text format with titles, URLs, descriptions, duration, and view counts.
Requires BRAVE_API_KEY environment variable to be set.

Results format:
Each result is on a separate line with: [Rank] Title | URL | Description | Duration: X | Views: Y

Best Practices:
- Use descriptive search queries for video content
- Use quotes for exact phrase matching: \"how to cook pasta\"
- Be specific about the type of video: \"tutorial\", \"review\", \"documentary\"
- Keep queries focused to get relevant video results

Examples:
- `{\"query\": \"python programming tutorial\"}`
- `{\"query\": \"guitar lessons for beginners\"}`
- `{\"query\": \"documentary about climate change\"}`
"
		.to_string(),
		parameters: json!({
			"type": "object",
			"properties": {
				"query": {
					"type": "string",
					"description": "The search query to execute"
				},
				"count": {
					"type": "integer",
					"description": "Number of results to return (default: 20, max: 20)",
					"minimum": 1,
					"maximum": 20,
					"default": 20
				},
				"offset": {
					"type": "integer",
					"description": "Number of results to skip for pagination (default: 0, max: 9)",
					"minimum": 0,
					"maximum": 9,
					"default": 0
				},
				"country": {
					"type": "string",
					"description": "Country code for localized results (e.g., 'US', 'GB', 'DE')",
					"default": "US"
				},
				"search_lang": {
					"type": "string",
					"description": "Language for search results (e.g., 'en', 'es', 'fr')",
					"default": "en"
				},
				"ui_lang": {
					"type": "string",
					"description": "Language for UI elements (e.g., 'en-US', 'es-ES', 'fr-FR')",
					"default": "en-US"
				},
				"safesearch": {
					"type": "string",
					"description": "Safe search setting: 'strict', 'moderate', or 'off'",
					"enum": ["strict", "moderate", "off"],
					"default": "moderate"
				},
				"freshness": {
					"type": "string",
					"description": "Time filter for results: 'pd' (past day), 'pw' (past week), 'pm' (past month), 'py' (past year)",
					"enum": ["pd", "pw", "pm", "py"]
				}
			},
			"required": ["query"]
		}),
	}
}

// Execute a video search using Brave Search API
pub async fn execute_video_search(
	call: &McpToolCall,
	_cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	// Extract and validate query
	let query = match extract_and_validate_query(call) {
		Ok(q) => q,
		Err(error_result) => return Ok(error_result),
	};

	// Get API key from environment
	let api_key = match std::env::var("BRAVE_API_KEY") {
		Ok(key) => key,
		Err(_) => {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"BRAVE_API_KEY environment variable is not set".to_string(),
			));
		}
	};

	// Extract optional parameters with defaults
	let count = call
		.parameters
		.get("count")
		.and_then(|v| v.as_u64())
		.unwrap_or(20) as u32;
	let offset = call
		.parameters
		.get("offset")
		.and_then(|v| v.as_u64())
		.unwrap_or(0) as u32;
	let country = call
		.parameters
		.get("country")
		.and_then(|v| v.as_str())
		.unwrap_or("US");
	let search_lang = call
		.parameters
		.get("search_lang")
		.and_then(|v| v.as_str())
		.unwrap_or("en");
	let ui_lang = call
		.parameters
		.get("ui_lang")
		.and_then(|v| v.as_str())
		.unwrap_or("en-US");
	let safesearch = call
		.parameters
		.get("safesearch")
		.and_then(|v| v.as_str())
		.unwrap_or("moderate");

	// Build the API URL
	let mut url = format!(
		"https://api.search.brave.com/res/v1/videos/search?q={}&count={}&offset={}&country={}&search_lang={}&ui_lang={}&safesearch={}",
		urlencoding::encode(&query),
		count,
		offset,
		country,
		search_lang,
		ui_lang,
		safesearch
	);

	// Add freshness filter if specified
	if let Some(freshness) = call.parameters.get("freshness").and_then(|v| v.as_str()) {
		url.push_str(&format!("&freshness={}", freshness));
	}

	// Create HTTP client
	let client = reqwest::Client::new();

	// Make the API request
	let search_result = match make_brave_api_request(&client, &url, &api_key, "video").await {
		Ok(result) => result,
		Err(e) => {
			return Ok(create_api_error_result(
				e,
				"video",
				"video_search",
				&call.tool_id,
			))
		}
	};

	// Format the results
	let formatted_results = match format_video_results(&search_result, &query) {
		Ok(results) => results,
		Err(e) => {
			return Ok(create_api_error_result(
				e,
				"video",
				"video_search",
				&call.tool_id,
			))
		}
	};

	Ok(McpToolResult::success(
		"video_search".to_string(),
		call.tool_id.clone(),
		formatted_results,
	))
}
