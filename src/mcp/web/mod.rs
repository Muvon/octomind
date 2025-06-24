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

// Handles web search operations using Brave Search API and HTML conversion

use super::{McpToolCall, McpToolResult};
use anyhow::Result;

pub mod functions;
pub mod html_converter;
pub mod search;

// Individual search modules
pub mod api_client;
pub mod formatters;
pub mod image_search;
pub mod news_search;
pub mod video_search;
pub mod web_search;

pub use functions::get_all_functions;
pub use search::{
	execute_image_search, execute_news_search, execute_video_search, execute_web_search,
};

// Execute HTML to Markdown conversion with cancellation support
pub async fn execute_read_html(
	call: &McpToolCall,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Ok(McpToolResult::error(
				call.tool_name.clone(),
				call.tool_id.clone(),
				"HTML to Markdown conversion cancelled".to_string(),
			));
		}
	}

	html_converter::execute_read_html(call).await
}
