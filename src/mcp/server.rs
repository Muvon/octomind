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

// External MCP server provider

use std::collections::HashMap;
use serde_json::{json, Value};
use anyhow::Result;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use super::{McpToolCall, McpToolResult, McpFunction};
use crate::config::{Config, McpServerConfig, McpServerMode};
use super::process;

// Define MCP server function definitions
pub async fn get_server_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
	// Note: enabled check is now handled at the role level via server_refs
	// All servers in the registry are considered available

	// Handle different server modes
	match server.mode {
		McpServerMode::Http => {
			// Handle local vs remote servers
			let server_url = get_server_base_url(server).await?;

			// Create a client
			let client = Client::new();

			// Prepare headers
			let mut headers = HeaderMap::new();
			headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

			// Add auth token if present
			if let Some(token) = &server.auth_token {
				headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token))?);
			}

			// Get schema URL - should be schema endpoint
			let schema_url = format!("{}/tools/list", server_url); // Correct endpoint

			// Make request to get schema
			let response = client.get(&schema_url)
				.headers(headers)
				.send()
			.await?;

			// Debug output
			// println!("Schema response from HTTP server: {}", response.status());

			// Check if request was successful
			if !response.status().is_success() {
				return Err(anyhow::anyhow!("Failed to get schema from MCP server: {}", response.status()));
			}

			// Parse response
			let schema: Value = response.json().await?;

			// Debug output
			// println!("Schema response body: {}", schema);

			// Extract functions
			let mut functions = Vec::new();

			// Extract tools from result.tools if available
			if let Some(result) = schema.get("result") {
				if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
					for func in tools {
						if let (Some(name), Some(description)) = (func.get("name").and_then(|n| n.as_str()),
							func.get("description").and_then(|d| d.as_str())) {
							// Check if this tool is enabled
							if server.tools.is_empty() || server.tools.contains(&name.to_string()) {
								// Get the parameters from the inputSchema field if available
								let parameters = func.get("inputSchema").cloned().unwrap_or(json!({}));

								functions.push(McpFunction {
									name: name.to_string(),
									description: description.to_string(),
									parameters,
								});
							}
						}
					}
				}
			} else {
				// Legacy support for functions directly in schema
				if let Some(schema_functions) = schema.get("functions").and_then(|f| f.as_array()) {
					for func in schema_functions {
						if let (Some(name), Some(description)) = (func.get("name").and_then(|n| n.as_str()),
							func.get("description").and_then(|d| d.as_str())) {
							// Check if this tool is enabled
							if server.tools.is_empty() || server.tools.contains(&name.to_string()) {
								// Get the parameters from the inputSchema field if available
								let parameters = func.get("inputSchema").cloned().unwrap_or(json!({}));

								functions.push(McpFunction {
									name: name.to_string(),
									description: description.to_string(),
									parameters,
								});
							}
						}
					}
				}
			}

			Ok(functions)
		},
		McpServerMode::Stdin => {
			// For stdin-based servers, ensure the server is running and get functions
			process::ensure_server_running(server).await?;
			process::get_stdin_server_functions(server).await
		}
	}
}

// Execute tool call on MCP server (either local or remote)
pub async fn execute_tool_call(call: &McpToolCall, server: &McpServerConfig) -> Result<McpToolResult> {
	execute_tool_call_with_cancellation(call, server, None).await
}

// Execute tool call on MCP server with cancellation support
pub async fn execute_tool_call_with_cancellation(
	call: &McpToolCall,
	server: &McpServerConfig,
	cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>
) -> Result<McpToolResult> {
	use std::sync::atomic::Ordering;

	// Check for cancellation before starting
	if let Some(ref token) = cancellation_token {
		if token.load(Ordering::SeqCst) {
			return Err(anyhow::anyhow!("External tool execution cancelled"));
		}
	}

	// Note: enabled check is now handled at the role level via server_refs
	// All servers in the registry are considered available for execution

	// Extract tool name and parameters
	let tool_name = &call.tool_name;
	let parameters = &call.parameters;

	// Tool execution display is now handled in response.rs to avoid duplication

	// Handle different server modes
	match server.mode {
		McpServerMode::Http => {
			// Check for cancellation before HTTP request
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("External tool execution cancelled"));
				}
			}

			// Handle local vs remote servers for HTTP mode
			let server_url = get_server_base_url(server).await?;

			// Create a client with configured timeout
			let client = Client::builder()
				.timeout(std::time::Duration::from_secs(server.timeout_seconds))
				.build()
				.unwrap_or_else(|_| Client::new());

			// Prepare headers
			let mut headers = HeaderMap::new();
			headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

			// Add auth token if present
			if let Some(token) = &server.auth_token {
				headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token))?);
			}

			// Get execution URL
			let execute_url = format!("{}/tools/call", server_url);

			// Prepare request body
			let request_body = json!({
				"name": tool_name,
				"arguments": parameters
			});

			// Check for cancellation one more time before sending request
			if let Some(ref token) = cancellation_token {
				if token.load(Ordering::SeqCst) {
					return Err(anyhow::anyhow!("External tool execution cancelled"));
				}
			}

			// Make request to execute tool
			let response = client.post(&execute_url)
				.headers(headers)
				.json(&request_body)
				.send()
			.await?;

			// Check if request was successful
			if !response.status().is_success() {
				// Save the status before consuming the response with text()
				let status = response.status();
				let error_text = response.text().await?;
				return Err(anyhow::anyhow!("Failed to execute tool on MCP server: {}, {}", status, error_text));
			}

			// Parse response
			let result: Value = response.json().await?;

			// Extract result or error from the response
			let output = if let Some(_error) = result.get("error") {
				json!({
					"error": true,
					"success": false,
					"message": result.get("message").and_then(|m| m.as_str()).unwrap_or("Server error")
				})
			} else {
				result.get("result").cloned().unwrap_or(json!("No result"))
			};

			// Create tool result
			let tool_result = McpToolResult {
				tool_name: tool_name.clone(),
				tool_id: call.tool_id.clone(),
				result: json!({
					"output": output,
					"parameters": parameters
				}),
			};

			Ok(tool_result)
		},
		McpServerMode::Stdin => {
			// For stdin-based servers, use the stdin communication channel
			// Note: stdin servers don't currently support cancellation mid-execution
			process::execute_stdin_tool_call(call, server).await
		}
	}
}

// Get the base URL for a server, starting it if necessary for local servers
async fn get_server_base_url(server: &McpServerConfig) -> Result<String> {
	match server.mode {
		McpServerMode::Http => {
			// Check if this is a local server that needs to be started
			if server.command.is_some() {
				// This is a local server, ensure it's running
				process::ensure_server_running(server).await
			} else if let Some(url) = &server.url {
				// This is a remote server with a URL
				Ok(url.trim_end_matches("/").to_string())
			} else {
				// Neither remote nor local configuration
				Err(anyhow::anyhow!("Invalid server configuration: neither URL nor command specified for server '{}'", server.name))
			}
		},
		McpServerMode::Stdin => {
			// For stdin-based servers, return a pseudo-URL
			if server.command.is_some() {
				// Ensure the stdin server is running
				process::ensure_server_running(server).await
			} else {
				Err(anyhow::anyhow!("Invalid server configuration: command not specified for stdin-based server '{}'", server.name))
			}
		}
	}
}

// Get all available functions from all configured servers
pub async fn get_all_server_functions(config: &Config) -> Result<HashMap<String, (McpFunction, McpServerConfig)>> {
	let mut functions = HashMap::new();

	// Only proceed if MCP has any servers configured
	if config.mcp.servers.is_empty() {
		return Ok(functions);
	}

	// Get available servers from merged config (which should already be filtered by server_refs)
	let servers: Vec<crate::config::McpServerConfig> = config.mcp.servers.values().cloned().collect();

	// Check each server
	for server in &servers {
		let server_functions = get_server_functions(server).await?;

		for func in server_functions {
			functions.insert(func.name.clone(), (func, server.clone()));
		}
	}

	Ok(functions)
}

// Clean up any running server processes when the program exits
pub fn cleanup_servers() -> Result<()> {
	process::stop_all_servers()
}
