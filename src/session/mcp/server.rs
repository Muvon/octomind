// External MCP server provider

use std::collections::HashMap;
use serde_json::{json, Value};
use anyhow::Result;
use reqwest::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use super::{McpToolCall, McpToolResult, McpFunction};
use crate::config::{Config, McpServerConfig};
use super::process;

// Define MCP server function definitions
pub async fn get_server_functions(server: &McpServerConfig) -> Result<Vec<McpFunction>> {
    if !server.enabled {
        return Ok(Vec::new());
    }

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
    
    // Get schema URL
    let schema_url = format!("{}/schema", server_url);
    
    // Make request to get schema
    let response = client.get(&schema_url)
        .headers(headers)
        .send()
        .await?;
    
    // Check if request was successful
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Failed to get schema from MCP server: {}", response.status()));
    }
    
    // Parse response
    let schema: Value = response.json().await?;
    
    // Extract functions
    let mut functions = Vec::new();
    
    if let Some(schema_functions) = schema.get("functions").and_then(|f| f.as_array()) {
        for func in schema_functions {
            if let (Some(name), Some(description)) = (func.get("name").and_then(|n| n.as_str()), 
                                                   func.get("description").and_then(|d| d.as_str())) {
                // Check if this tool is enabled
                if server.tools.is_empty() || server.tools.contains(&name.to_string()) {
                    let parameters = func.get("parameters").cloned().unwrap_or(json!({}));
                    
                    functions.push(McpFunction {
                        name: name.to_string(),
                        description: description.to_string(),
                        parameters,
                    });
                }
            }
        }
    }
    
    Ok(functions)
}

// Execute tool call on MCP server (either local or remote)
pub async fn execute_tool_call(call: &McpToolCall, server: &McpServerConfig) -> Result<McpToolResult> {
    if !server.enabled {
        return Err(anyhow::anyhow!("Server is not enabled"));
    }
    
    // Extract tool name and parameters
    let tool_name = &call.tool_name;
    let parameters = &call.parameters;

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
    
    // Get execution URL
    let execute_url = format!("{}/execute", server_url);
    
    // Prepare request body
    let request_body = json!({
        "name": tool_name,
        "arguments": parameters
    });
    
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
    let output = if let Some(error) = result.get("error") {
        json!({
            "error": error,
            "success": false,
            "message": result.get("message").and_then(|m| m.as_str()).unwrap_or("Server error")
        })
    } else {
        result.get("result").cloned().unwrap_or(json!("No result"))
    };

    // Create tool result
    let tool_result = McpToolResult {
        tool_name: tool_name.clone(),
        result: json!({
            "output": output,
            "parameters": parameters
        }),
    };
    
    Ok(tool_result)
}

// Get the base URL for a server, starting it if necessary for local servers
async fn get_server_base_url(server: &McpServerConfig) -> Result<String> {
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
}

// Get all available functions from all configured servers
pub async fn get_all_server_functions(config: &Config) -> Result<HashMap<String, (McpFunction, McpServerConfig)>> {
    let mut functions = HashMap::new();
    
    // Only proceed if MCP is enabled
    if !config.mcp.enabled {
        return Ok(functions);
    }
    
    // Check each server
    for server in &config.mcp.servers {
        if server.enabled {
            let server_functions = get_server_functions(server).await?;
            
            for func in server_functions {
                functions.insert(func.name.clone(), (func, server.clone()));
            }
        }
    }
    
    Ok(functions)
}

// Clean up any running server processes when the program exits
pub fn cleanup_servers() -> Result<()> {
    process::stop_all_servers()
}