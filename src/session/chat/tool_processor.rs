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

// Tool call processing module - extracted from response.rs for better modularity

use crate::config::Config;
use crate::session::chat::session::ChatSession;
use crate::session::chat::tool_error_tracker::ToolErrorTracker;
use crate::log_debug;
use anyhow::Result;
use colored::Colorize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct ToolProcessor {
    pub error_tracker: ToolErrorTracker,
}

impl ToolProcessor {
    pub fn new() -> Self {
        Self {
            error_tracker: ToolErrorTracker::new(3),
        }
    }

    /// Execute tool calls in parallel and handle their results
    pub async fn execute_tool_calls(
        &mut self,
        tool_calls: Vec<crate::mcp::McpToolCall>,
        chat_session: &mut ChatSession,
        config: &Config,
        operation_cancelled: Arc<AtomicBool>,
    ) -> Result<(Vec<String>, bool)> {
        let mut tool_tasks = Vec::new();
        let mut tool_results = Vec::new();

        // Execute all tool calls in parallel
        for tool_call in tool_calls.clone() {
            if operation_cancelled.load(Ordering::SeqCst) {
                return Ok((tool_results, false));
            }

            let operation_cancelled_clone = operation_cancelled.clone();
            let config_clone = config.clone();
            let task = tokio::spawn(async move {
                crate::mcp::execute_tool_call_with_cancellation(&tool_call, &config_clone, Some(operation_cancelled_clone)).await
            });
            tool_tasks.push(task);
        }

        // Collect all results and display them cleanly with real-time cancellation feedback
        for (i, task) in tool_tasks.into_iter().enumerate() {
            if operation_cancelled.load(Ordering::SeqCst) {
                return Ok((tool_results, false));
            }

            let tool_call = &tool_calls[i];
            let result = task.await;

            match result {
                Ok(Ok((tool_result, _duration_ms))) => {
                    // Reset error counter on successful tool execution
                    self.error_tracker.record_success(&tool_call.tool_name);

                    // Format and display the result
                    let result_content = match &tool_result.result {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    
                    let formatted_result = format!(
                        "**{}**: {}",
                        tool_call.tool_name,
                        result_content.trim()
                    );

                    if config.get_log_level().is_debug_enabled() {
                        log_debug!("Tool {} executed successfully", tool_call.tool_name);
                    }

                    tool_results.push(formatted_result.clone());

                    // Create tool message for session
                    let tool_message = crate::session::Message {
                        role: "tool".to_string(),
                        content: result_content,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        cached: false,
                        tool_call_id: Some(tool_call.tool_id.clone()),
                        name: Some(tool_call.tool_name.clone()),
                        tool_calls: None,
                    };

                    chat_session.session.messages.push(tool_message);
                }
                Ok(Err(e)) => {
                    let has_hit_threshold = self.error_tracker.record_error(&tool_call.tool_name);
                    let error_msg = format!("Error executing {}: {}", tool_call.tool_name, e);
                    
                    if config.get_log_level().is_debug_enabled() {
                        log_debug!("{}", error_msg);
                    }

                    // Check if we should stop due to too many consecutive errors
                    if has_hit_threshold {
                        println!(
                            "{}",
                            "Too many consecutive tool errors. Stopping tool execution.".red()
                        );
                        return Ok((tool_results, false));
                    }

                    tool_results.push(error_msg.clone());

                    // Create error tool message for session
                    let tool_message = crate::session::Message {
                        role: "tool".to_string(),
                        content: error_msg,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        cached: false,
                        tool_call_id: Some(tool_call.tool_id.clone()),
                        name: Some(tool_call.tool_name.clone()),
                        tool_calls: None,
                    };

                    chat_session.session.messages.push(tool_message);
                }
                Err(e) => {
                    let has_hit_threshold = self.error_tracker.record_error(&tool_call.tool_name);
                    let error_msg = format!("Task error for {}: {}", tool_call.tool_name, e);
                    
                    if config.get_log_level().is_debug_enabled() {
                        log_debug!("{}", error_msg);
                    }

                    if has_hit_threshold {
                        println!(
                            "{}",
                            "Too many consecutive tool errors. Stopping tool execution.".red()
                        );
                        return Ok((tool_results, false));
                    }

                    tool_results.push(error_msg);
                }
            }
        }

        let should_continue = !tool_results.is_empty();
        Ok((tool_results, should_continue))
    }
}

impl Default for ToolProcessor {
    fn default() -> Self {
        Self::new()
    }
}