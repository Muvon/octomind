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

// MCP command handler

use super::utils::get_tool_server_name_async;
use super::{CommandOutput, CommandResult};
use crate::config::{Config, McpConnectionType};
use anyhow::Result;

pub async fn handle_mcp(config: &Config, role: &str, params: &[&str]) -> Result<CommandResult> {
	// Handle /mcp command for showing MCP server status and tools
	// Support subcommands: list, info, full
	let subcommand = if params.is_empty() { "info" } else { params[0] };

	match subcommand {
		"list" => handle_mcp_list(config, role).await,
		"info" => handle_mcp_info(config, role).await,
		"full" => handle_mcp_full(config, role).await,
		"health" => handle_mcp_health(config, role).await,
		"dump" => handle_mcp_dump(config, role).await,
		"validate" => handle_mcp_validate(config, role).await,
		_ => handle_mcp_invalid(),
	}
}

async fn handle_mcp_list(config: &Config, role: &str) -> Result<CommandResult> {
	// Very short list - just tool names
	let config_for_role = config.get_merged_config_for_role(role);
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;

	// Build JSON output with all data needed for display
	let mut servers_json: std::collections::HashMap<String, Vec<String>> =
		std::collections::HashMap::new();

	if !available_functions.is_empty() {
		// Group tools by server name
		for func in &available_functions {
			let server_name = get_tool_server_name_async(&func.name, &config_for_role).await;
			servers_json
				.entry(server_name)
				.or_default()
				.push(func.name.clone());
		}
	}

	let json_output = serde_json::json!({
		"subcommand": "list",
		"servers": servers_json,
		"total_tools": available_functions.len()
	});

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: String::new(),
			data: json_output,
		},
	)))
}

async fn handle_mcp_info(config: &Config, role: &str) -> Result<CommandResult> {
	// Default view - server status + tools with short descriptions
	let config_for_role = config.get_merged_config_for_role(role);

	if config_for_role.mcp.servers.is_empty() {
		let json_output = serde_json::json!({"subcommand": "info", "servers": [], "message": "No MCP servers configured"});
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Mcp {
				mcp_command: String::new(),
				data: json_output,
			},
		)));
	}

	// Collect server status data
	let server_report = crate::mcp::server::get_server_status_report();
	let mut servers_data = Vec::new();
	let mut seen_server_names: std::collections::HashSet<String> = std::collections::HashSet::new();

	for server in &config_for_role.mcp.servers {
		let (health, restart_info) = match server.connection_type() {
			McpConnectionType::Builtin => (
				crate::mcp::process::ServerHealth::Running,
				Default::default(),
			),
			McpConnectionType::Http | McpConnectionType::Stdin => {
				if let Some((h, r)) = server_report.get(server.name()) {
					(*h, r.clone())
				} else {
					let health = check_server_health_on_demand(server).await;
					(health, Default::default())
				}
			}
		};

		let health_str = match health {
			crate::mcp::process::ServerHealth::Running => "running",
			crate::mcp::process::ServerHealth::Dead => "dead",
			crate::mcp::process::ServerHealth::Restarting => "restarting",
			crate::mcp::process::ServerHealth::Failed => "failed",
			crate::mcp::process::ServerHealth::Unreachable => "unreachable",
		};

		seen_server_names.insert(server.name().to_string());
		servers_data.push(serde_json::json!({
			"name": server.name(),
			"health": health_str,
			"connection_type": format!("{:?}", server.connection_type()),
			"tools": server.tools(),
			"restart_count": restart_info.restart_count,
			"consecutive_failures": restart_info.consecutive_failures,
		}));
	}

	// Append dynamically-registered servers (capability activations, runtime
	// `mcp add`) that aren't already in the static config — otherwise they'd
	// show their tools under "Available Tools" but be invisible under
	// "MCP Server Status".
	for (name, tools, enabled) in crate::mcp::core::dynamic::list_servers() {
		if !enabled || seen_server_names.contains(&name) {
			continue;
		}
		let (health, restart_info) = if let Some((h, r)) = server_report.get(&name) {
			(*h, r.clone())
		} else {
			(
				crate::mcp::process::ServerHealth::Running,
				Default::default(),
			)
		};
		let health_str = match health {
			crate::mcp::process::ServerHealth::Running => "running",
			crate::mcp::process::ServerHealth::Dead => "dead",
			crate::mcp::process::ServerHealth::Restarting => "restarting",
			crate::mcp::process::ServerHealth::Failed => "failed",
			crate::mcp::process::ServerHealth::Unreachable => "unreachable",
		};
		servers_data.push(serde_json::json!({
			"name": name,
			"health": health_str,
			"connection_type": "Dynamic",
			"tools": tools,
			"restart_count": restart_info.restart_count,
			"consecutive_failures": restart_info.consecutive_failures,
		}));
	}

	// Collect tools data with short descriptions
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;
	let mut tools_by_server: std::collections::HashMap<String, Vec<serde_json::Value>> =
		std::collections::HashMap::new();

	for func in &available_functions {
		let server_name = get_tool_server_name_async(&func.name, &config_for_role).await;
		let short_desc = if func.description.chars().count() > 60 {
			let truncated: String = func.description.chars().take(57).collect();
			format!("{}...", truncated)
		} else {
			func.description.clone()
		};

		tools_by_server
			.entry(server_name)
			.or_default()
			.push(serde_json::json!({
				"name": func.name,
				"description": short_desc,
			}));
	}

	let json_output = serde_json::json!({
		"subcommand": "info",
		"servers": servers_data,
		"tools": tools_by_server,
		"total_tools": available_functions.len()
	});

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: String::new(),
			data: json_output,
		},
	)))
}

async fn handle_mcp_full(config: &Config, role: &str) -> Result<CommandResult> {
	let config_for_role = config.get_merged_config_for_role(role);

	if config_for_role.mcp.servers.is_empty() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Mcp {
				mcp_command: "full".to_string(),
				data: serde_json::json!({"subcommand": "full", "servers": [], "tools": {}, "message": "No MCP servers configured"}),
			},
		)));
	}

	// Collect server status data
	let server_report = crate::mcp::server::get_server_status_report();
	let mut servers_data = Vec::new();
	let mut seen_server_names: std::collections::HashSet<String> = std::collections::HashSet::new();

	for server in &config_for_role.mcp.servers {
		let (health, restart_info) = match server.connection_type() {
			McpConnectionType::Builtin => (
				crate::mcp::process::ServerHealth::Running,
				Default::default(),
			),
			McpConnectionType::Http | McpConnectionType::Stdin => {
				if let Some((h, r)) = server_report.get(server.name()) {
					(*h, r.clone())
				} else {
					let health = check_server_health_on_demand(server).await;
					(health, Default::default())
				}
			}
		};

		let health_str = match health {
			crate::mcp::process::ServerHealth::Running => "running",
			crate::mcp::process::ServerHealth::Dead => "dead",
			crate::mcp::process::ServerHealth::Restarting => "restarting",
			crate::mcp::process::ServerHealth::Failed => "failed",
			crate::mcp::process::ServerHealth::Unreachable => "unreachable",
		};

		seen_server_names.insert(server.name().to_string());
		servers_data.push(serde_json::json!({
			"name": server.name(),
			"health": health_str,
			"connection_type": format!("{:?}", server.connection_type()),
			"tools": server.tools(),
			"restart_count": restart_info.restart_count,
			"consecutive_failures": restart_info.consecutive_failures,
		}));
	}

	// Append dynamically-registered servers (capability activations, runtime
	// `mcp add`) that aren't already in the static config.
	for (name, tools, enabled) in crate::mcp::core::dynamic::list_servers() {
		if !enabled || seen_server_names.contains(&name) {
			continue;
		}
		let (health, restart_info) = if let Some((h, r)) = server_report.get(&name) {
			(*h, r.clone())
		} else {
			(
				crate::mcp::process::ServerHealth::Running,
				Default::default(),
			)
		};
		let health_str = match health {
			crate::mcp::process::ServerHealth::Running => "running",
			crate::mcp::process::ServerHealth::Dead => "dead",
			crate::mcp::process::ServerHealth::Restarting => "restarting",
			crate::mcp::process::ServerHealth::Failed => "failed",
			crate::mcp::process::ServerHealth::Unreachable => "unreachable",
		};
		servers_data.push(serde_json::json!({
			"name": name,
			"health": health_str,
			"connection_type": "Dynamic",
			"tools": tools,
			"restart_count": restart_info.restart_count,
			"consecutive_failures": restart_info.consecutive_failures,
		}));
	}

	// Collect full tool details grouped by server
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;
	let mut tools_by_server: std::collections::HashMap<String, Vec<serde_json::Value>> =
		std::collections::HashMap::new();

	for func in &available_functions {
		let server_name = get_tool_server_name_async(&func.name, &config_for_role).await;
		tools_by_server
			.entry(server_name)
			.or_default()
			.push(serde_json::json!({
				"name": func.name,
				"description": func.description,
				"parameters": func.parameters,
			}));
	}

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: "full".to_string(),
			data: serde_json::json!({
				"subcommand": "full",
				"servers": servers_data,
				"tools": tools_by_server,
				"total_tools": available_functions.len(),
			}),
		},
	)))
}

async fn handle_mcp_health(config: &Config, role: &str) -> Result<CommandResult> {
	let config_for_role = config.get_merged_config_for_role(role);

	if config_for_role.mcp.servers.is_empty() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Mcp {
				mcp_command: "health".to_string(),
				data: serde_json::json!({"subcommand": "health", "servers": [], "message": "No MCP servers configured"}),
			},
		)));
	}

	let monitor_running = crate::mcp::health_monitor::is_health_monitor_running();

	if let Err(e) = crate::mcp::health_monitor::force_health_check(&config_for_role).await {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Mcp {
				mcp_command: "health".to_string(),
				data: serde_json::json!({
					"subcommand": "health",
					"monitor_running": monitor_running,
					"error": format!("Health check failed: {}", e),
				}),
			},
		)));
	}

	let server_report = crate::mcp::server::get_server_status_report();
	let mut servers_data = Vec::new();

	for server in &config_for_role.mcp.servers {
		if let McpConnectionType::Http | McpConnectionType::Stdin = server.connection_type() {
			let (health, restart_info) = if let Some((h, r)) = server_report.get(server.name()) {
				(*h, r.clone())
			} else {
				let health = check_server_health_on_demand(server).await;
				(health, Default::default())
			};

			let health_str = match health {
				crate::mcp::process::ServerHealth::Running => "running",
				crate::mcp::process::ServerHealth::Dead => "dead",
				crate::mcp::process::ServerHealth::Restarting => "restarting",
				crate::mcp::process::ServerHealth::Failed => "failed",
				crate::mcp::process::ServerHealth::Unreachable => "unreachable",
			};

			let last_checked_secs = restart_info.last_health_check.and_then(|t| {
				std::time::SystemTime::now()
					.duration_since(t)
					.ok()
					.map(|d| d.as_secs())
			});

			servers_data.push(serde_json::json!({
				"name": server.name(),
				"health": health_str,
				"restart_count": restart_info.restart_count,
				"consecutive_failures": restart_info.consecutive_failures,
				"last_checked_secs_ago": last_checked_secs,
			}));
		}
	}

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: "health".to_string(),
			data: serde_json::json!({
				"subcommand": "health",
				"monitor_running": monitor_running,
				"servers": servers_data,
			}),
		},
	)))
}

async fn handle_mcp_dump(config: &Config, role: &str) -> Result<CommandResult> {
	let config_for_role = config.get_merged_config_for_role(role);
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;

	let tools: Vec<serde_json::Value> = available_functions
		.iter()
		.map(|func| {
			serde_json::json!({
				"name": func.name,
				"description": func.description,
				"parameters": func.parameters,
			})
		})
		.collect();

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: "dump".to_string(),
			data: serde_json::json!({
				"subcommand": "dump",
				"tools": tools,
				"total_tools": available_functions.len(),
			}),
		},
	)))
}

async fn handle_mcp_validate(config: &Config, role: &str) -> Result<CommandResult> {
	let config_for_role = config.get_merged_config_for_role(role);
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;

	let mut all_valid = true;
	let mut tools_validation: Vec<serde_json::Value> = Vec::new();

	for func in &available_functions {
		let mut issues = Vec::new();

		let has_type = func.parameters.get("type").is_some();
		let has_one_of = func.parameters.get("oneOf").is_some();
		if !has_type && !has_one_of {
			issues.push("Missing 'type' or 'oneOf' field in root schema".to_string());
		}

		if let Some(properties) = func.parameters.get("properties") {
			if let Some(props_obj) = properties.as_object() {
				for (prop_name, prop_def) in props_obj {
					if prop_def.get("type").is_none() && prop_def.get("oneOf").is_none() {
						issues.push(format!(
							"Property '{}' missing 'type' or 'oneOf' field",
							prop_name
						));
					}
				}
			}
		} else if has_type && func.parameters.get("type").and_then(|t| t.as_str()) == Some("object")
		{
			issues.push("Object type schema missing 'properties' field".to_string());
		}

		if !issues.is_empty() {
			all_valid = false;
		}

		tools_validation.push(serde_json::json!({
			"name": func.name,
			"valid": issues.is_empty(),
			"issues": issues,
		}));
	}

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: "validate".to_string(),
			data: serde_json::json!({
				"subcommand": "validate",
				"all_valid": all_valid,
				"tools": tools_validation,
				"total_tools": available_functions.len(),
			}),
		},
	)))
}

fn handle_mcp_invalid() -> Result<CommandResult> {
	// Invalid subcommand
	let json_output = serde_json::json!({
		"subcommand": "invalid",
		"message": "Invalid MCP subcommand"
	});
	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Mcp {
			mcp_command: "invalid".to_string(),
			data: json_output,
		},
	)))
}
/// Perform on-demand health check for a server that's not in the status report
async fn check_server_health_on_demand(
	server: &crate::config::McpServerConfig,
) -> crate::mcp::process::ServerHealth {
	match server.connection_type() {
		McpConnectionType::Stdin => {
			// For stdin servers, check if the process is running
			if crate::mcp::process::is_server_running(server.name()) {
				crate::mcp::process::ServerHealth::Running
			} else {
				crate::mcp::process::ServerHealth::Dead
			}
		}
		McpConnectionType::Http => {
			if server.command().is_some() {
				// Local HTTP server - check if the process is running
				if crate::mcp::process::is_server_running(server.name()) {
					crate::mcp::process::ServerHealth::Running
				} else {
					crate::mcp::process::ServerHealth::Dead
				}
			} else {
				// Remote HTTP server - perform HTTP health check
				match perform_http_health_check_sync(server).await {
					Ok(true) => crate::mcp::process::ServerHealth::Running,
					Ok(false) => crate::mcp::process::ServerHealth::Dead,
					Err(_) => crate::mcp::process::ServerHealth::Dead,
				}
			}
		}
		McpConnectionType::Builtin => {
			// Builtin servers are always running
			crate::mcp::process::ServerHealth::Running
		}
	}
}

/// Perform HTTP health check for remote servers (duplicate of health_monitor function)
async fn perform_http_health_check_sync(server: &crate::config::McpServerConfig) -> Result<bool> {
	if let Some(url) = server.url() {
		let client = reqwest::Client::builder()
			.timeout(std::time::Duration::from_secs(5)) // 5 second timeout for health checks
			.build()?;

		// Try to make a JSON-RPC tools/list request to check if server is responding
		let health_url = url.trim_end_matches("/");

		// Use the same header setup as the main server implementation
		let mut headers = reqwest::header::HeaderMap::new();
		headers.insert(
			reqwest::header::CONTENT_TYPE,
			reqwest::header::HeaderValue::from_static("application/json"),
		);

		// Use tools/list for health check (same as main functionality)
		let jsonrpc_request = crate::mcp::server::create_tools_list_request();

		match client
			.post(health_url)
			.headers(headers)
			.json(&jsonrpc_request)
			.send()
			.await
		{
			Ok(response) => {
				let is_healthy =
					response.status().is_success() || response.status().is_client_error();
				// Both 2xx and 4xx are considered "server responding" - 5xx or connection errors are not
				Ok(is_healthy)
			}
			Err(_) => Ok(false),
		}
	} else {
		Err(anyhow::anyhow!("No URL configured for HTTP server"))
	}
}
