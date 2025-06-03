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

// Message handling module - extracted from response.rs for better modularity

use crate::session::chat::session::ChatSession;
use crate::session::ProviderExchange;
use anyhow::Result;

pub struct MessageHandler;

impl MessageHandler {
    /// Extract original tool calls from provider exchange based on provider format
    pub fn extract_original_tool_calls(exchange: &ProviderExchange) -> Option<serde_json::Value> {
        // First check if there's a stored tool_calls_content (for Anthropic and Google)
        if let Some(content_data) = exchange.response.get("tool_calls_content") {
            return Some(content_data.clone());
        }
        
        // Then check for OpenRouter/OpenAI format
        if let Some(tool_calls) = exchange.response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
        {
            return Some(tool_calls.clone());
        }
        
        None
    }

    /// Add assistant message with tool calls preserved
    pub fn add_assistant_message_with_tool_calls(
        chat_session: &mut ChatSession,
        content: &str,
        exchange: &ProviderExchange,
    ) -> Result<()> {
        // Extract the original tool_calls from the exchange response based on provider
        let original_tool_calls = Self::extract_original_tool_calls(exchange);

        // Create the assistant message directly with tool_calls preserved from the exchange
        let assistant_message = crate::session::Message {
            role: "assistant".to_string(),
            content: content.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            cached: false,
            tool_call_id: None,
            name: None,
            tool_calls: original_tool_calls, // Store the original tool_calls for proper reconstruction
        };

        // Add the assistant message to the session
        chat_session.session.messages.push(assistant_message);
        
        // Update last response
        chat_session.last_response = content.to_string();

        Ok(())
    }

    /// Log assistant response and exchange data
    pub fn log_response_data(
        session_name: &str,
        content: &str,
        exchange: &ProviderExchange,
    ) -> Result<()> {
        let _ = crate::session::logger::log_assistant_response(session_name, content);
        let _ = crate::session::logger::log_raw_exchange(exchange);
        Ok(())
    }
}