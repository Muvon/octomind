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

// MCP command handler

use super::utils::get_tool_server_name_async;
use crate::config::{Config, McpConnectionType};
use anyhow::Result;
use colored::Colorize;

pub async fn handle_mcp(config: &Config, role: &str, params: &[&str]) -> Result<bool> {
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

async fn handle_mcp_list(config: &Config, role: &str) -> Result<bool> {
	// Very short list - just tool names
	println!();
	println!("{}", "Available Tools".bright_cyan().bold());
	println!("{}", "─".repeat(30).dimmed());

	let config_for_role = config.get_merged_config_for_role(role);
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;

	if available_functions.is_empty() {
		println!("{}", "No tools available.".yellow());
	} else {
		// Group tools by server name
		let mut servers: std::collections::HashMap<String, Vec<&crate::mcp::McpFunction>> =
			std::collections::HashMap::new();

		for func in &available_functions {
			let server_name = get_tool_server_name_async(&func.name, &config_for_role).await;
			servers.entry(server_name).or_default().push(func);
		}

		for (server_name, tools) in servers {
			println!();
			println!("  {}", server_name.bright_blue().bold());
			for tool in tools {
				println!("    {}", tool.name.bright_white());
			}
		}
	}

	println!();
	println!(
		"{}",
		"Use '/mcp info' for descriptions or '/mcp full' for detailed parameters.".dimmed()
	);
	Ok(false)
}

async fn handle_mcp_info(config: &Config, role: &str) -> Result<bool> {
	// Default view - server status + tools with short descriptions
	println!();
	println!("{}", "MCP Server Status".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	// Get the merged config for this role
	let config_for_role = config.get_merged_config_for_role(role);

	if config_for_role.mcp.servers.is_empty() {
		println!("{}", "No MCP servers configured for this role.".yellow());
		return Ok(false);
	}

	// Show server status
	let server_report = crate::mcp::server::get_server_status_report();

	for server in &config_for_role.mcp.servers {
		let (health, restart_info) = match server.connection_type {
			McpConnectionType::Builtin => {
				// Internal servers are always running
				(
					crate::mcp::process::ServerHealth::Running,
					Default::default(),
				)
			}
			McpConnectionType::Http | McpConnectionType::Stdin => {
				// External servers - get from status report
				server_report
					.get(&server.name)
					.map(|(h, r)| (*h, r.clone()))
					.unwrap_or((crate::mcp::process::ServerHealth::Dead, Default::default()))
			}
		};

		let health_display = match health {
			crate::mcp::process::ServerHealth::Running => "✅ Running".green(),
			crate::mcp::process::ServerHealth::Dead => "❌ Dead".red(),
			crate::mcp::process::ServerHealth::Restarting => "🔄 Restarting".yellow(),
			crate::mcp::process::ServerHealth::Failed => "💥 Failed".bright_red(),
		};

		println!();
		println!("{}: {}", server.name.bright_white().bold(), health_display);
		println!("  Type: {:?}", server.connection_type);
		// Connection type field was removed

		if !server.tools.is_empty() {
			println!("  Configured tools: {}", server.tools.join(", ").dimmed());
		}

		if restart_info.restart_count > 0 {
			println!("  Restart count: {}", restart_info.restart_count);
			if restart_info.consecutive_failures > 0 {
				println!(
					"  Consecutive failures: {}",
					restart_info.consecutive_failures
				);
			}
		}
	}

	// Show available tools with short descriptions
	println!();
	println!("{}", "Available Tools".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;
	if available_functions.is_empty() {
		println!("{}", "No tools available.".yellow());
	} else {
		// Group tools by server name
		let mut servers: std::collections::HashMap<String, Vec<&crate::mcp::McpFunction>> =
			std::collections::HashMap::new();

		for func in &available_functions {
			let server_name = get_tool_server_name_async(&func.name, &config_for_role).await;
			servers.entry(server_name).or_default().push(func);
		}

		for (server_name, tools) in servers {
			println!();
			println!("  {}", server_name.bright_blue().bold());

			for tool in tools {
				// Show name and short description
				let short_desc = if tool.description.chars().count() > 60 {
					let truncated: String = tool.description.chars().take(57).collect();
					format!("{}...", truncated)
				} else {
					tool.description.clone()
				};

				if short_desc.is_empty() {
					println!("    {}", tool.name.bright_white());
				} else {
					println!("    {} - {}", tool.name.bright_white(), short_desc.dimmed());
				}
			}
		}
	}

	println!();
	println!(
		"{}",
		"Use '/mcp list' for names only or '/mcp full' for detailed parameters.".dimmed()
	);
	Ok(false)
}

async fn handle_mcp_full(config: &Config, role: &str) -> Result<bool> {
	// Full detailed view with parameters
	println!();
	println!(
		"{}",
		"MCP Server Status & Tools (Full Details)"
			.bright_cyan()
			.bold()
	);
	println!("{}", "─".repeat(60).dimmed());

	// Get the merged config for this role
	let config_for_role = config.get_merged_config_for_role(role);

	if config_for_role.mcp.servers.is_empty() {
		println!("{}", "No MCP servers configured for this role.".yellow());
		return Ok(false);
	}

	// Show server status (same as info)
	let server_report = crate::mcp::server::get_server_status_report();

	for server in &config_for_role.mcp.servers {
		let (health, restart_info) = match server.connection_type {
			McpConnectionType::Builtin => (
				crate::mcp::process::ServerHealth::Running,
				Default::default(),
			),
			McpConnectionType::Http | McpConnectionType::Stdin => server_report
				.get(&server.name)
				.map(|(h, r)| (*h, r.clone()))
				.unwrap_or((crate::mcp::process::ServerHealth::Dead, Default::default())),
		};

		let health_display = match health {
			crate::mcp::process::ServerHealth::Running => "✅ Running".green(),
			crate::mcp::process::ServerHealth::Dead => "❌ Dead".red(),
			crate::mcp::process::ServerHealth::Restarting => "🔄 Restarting".yellow(),
			crate::mcp::process::ServerHealth::Failed => "💥 Failed".bright_red(),
		};

		println!();
		println!("{}: {}", server.name.bright_white().bold(), health_display);
		println!("  Type: {:?}", server.connection_type);
		// Connection type field was removed

		if !server.tools.is_empty() {
			println!("  Configured tools: {}", server.tools.join(", ").dimmed());
		}

		if restart_info.restart_count > 0 {
			println!("  Restart count: {}", restart_info.restart_count);
			if restart_info.consecutive_failures > 0 {
				println!(
					"  Consecutive failures: {}",
					restart_info.consecutive_failures
				);
			}
		}
	}

	// Show available tools with full details
	println!();
	println!("{}", "Available Tools (Full Details)".bright_cyan().bold());
	println!("{}", "─".repeat(60).dimmed());

	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;
	if available_functions.is_empty() {
		println!("{}", "No tools available.".yellow());
	} else {
		// Group tools by server name
		let mut servers: std::collections::HashMap<String, Vec<&crate::mcp::McpFunction>> =
			std::collections::HashMap::new();

		for func in &available_functions {
			let server_name = get_tool_server_name_async(&func.name, &config_for_role).await;
			servers.entry(server_name).or_default().push(func);
		}

		for (server_name, tools) in servers {
			println!();
			println!("  {}", server_name.bright_blue().bold());

			for tool in tools {
				// Full detailed view with parameters
				println!("    {}", tool.name.bright_white().bold());

				// Show full description
				if !tool.description.is_empty() {
					println!("      {}", tool.description.dimmed());
				}

				// Show parameters if available
				if let Some(properties) = tool.parameters.get("properties") {
					if let Some(props_obj) = properties.as_object() {
						if !props_obj.is_empty() {
							println!("      {}", "Parameters:".bright_green());

							// Get required parameters
							let required_params: std::collections::HashSet<String> = tool
								.parameters
								.get("required")
								.and_then(|r| r.as_array())
								.map(|arr| {
									arr.iter()
										.filter_map(|v| v.as_str())
										.map(|s| s.to_string())
										.collect()
								})
								.unwrap_or_default();

							for (param_name, param_info) in props_obj {
								let is_required = required_params.contains(param_name);
								let required_marker = if is_required {
									"*".bright_red()
								} else {
									" ".normal()
								};

								let param_type = param_info
									.get("type")
									.and_then(|t| t.as_str())
									.unwrap_or("any");

								let param_desc = param_info
									.get("description")
									.and_then(|d| d.as_str())
									.unwrap_or("");

								println!(
									"        {}{}: {} {}",
									required_marker,
									param_name.bright_cyan(),
									param_type.yellow(),
									if !param_desc.is_empty() {
										format!("- {}", param_desc).dimmed()
									} else {
										"".normal()
									}
								);

								// Show enum values if available
								if let Some(enum_values) = param_info.get("enum") {
									if let Some(enum_array) = enum_values.as_array() {
										let values: Vec<String> = enum_array
											.iter()
											.filter_map(|v| v.as_str())
											.map(|s| s.to_string())
											.collect();
										if !values.is_empty() {
											println!(
												"          {}: {}",
												"options".bright_black(),
												values.join(", ").bright_black()
											);
										}
									}
								}

								// Show default value if available
								if let Some(default_val) = param_info.get("default") {
									println!(
										"          {}: {}",
										"default".bright_black(),
										default_val.to_string().bright_black()
									);
								}
							}
						}
					}
				} else if tool.parameters != serde_json::json!({}) {
					// Show raw parameters if not in standard format
					println!(
						"      {}: {}",
						"Schema".bright_green(),
						tool.parameters.to_string().dimmed()
					);
				}

				println!(); // Add spacing between tools
			}
		}
	}

	println!();
	println!("{}", "Legend: ".bright_yellow());
	println!("  {} Required parameter", "*".bright_red());
	println!(
		"  {}",
		"Use '/mcp list' for names only or '/mcp info' for overview.".dimmed()
	);
	Ok(false)
}

async fn handle_mcp_health(config: &Config, role: &str) -> Result<bool> {
	// Health check and restart subcommand
	println!();
	println!("{}", "MCP Server Health Check".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	let config_for_role = config.get_merged_config_for_role(role);

	if config_for_role.mcp.servers.is_empty() {
		println!("{}", "No MCP servers configured for this role.".yellow());
		return Ok(false);
	}

	// Show current health monitor status
	if crate::mcp::health_monitor::is_health_monitor_running() {
		println!("{}", "🔍 Health monitor: RUNNING".bright_green());
	} else {
		println!("{}", "🔍 Health monitor: STOPPED".bright_red());
	}
	println!();

	// Force a health check on all servers
	println!(
		"{}",
		"Performing health check on all external servers...".bright_blue()
	);

	if let Err(e) = crate::mcp::health_monitor::force_health_check(&config_for_role).await {
		println!("{}: {}", "Health check failed".bright_red(), e);
		return Ok(false);
	}

	// Show updated server status
	let server_report = crate::mcp::server::get_server_status_report();

	for server in &config_for_role.mcp.servers {
		if let McpConnectionType::Http | McpConnectionType::Stdin = server.connection_type {
			let (health, restart_info) = server_report
				.get(&server.name)
				.map(|(h, r)| (*h, r.clone()))
				.unwrap_or((crate::mcp::process::ServerHealth::Dead, Default::default()));

			let health_display = match health {
				crate::mcp::process::ServerHealth::Running => "✅ Running".green(),
				crate::mcp::process::ServerHealth::Dead => "❌ Dead".red(),
				crate::mcp::process::ServerHealth::Restarting => "🔄 Restarting".yellow(),
				crate::mcp::process::ServerHealth::Failed => "💥 Failed".bright_red(),
			};

			println!("{}: {}", server.name.bright_white().bold(), health_display);

			if restart_info.restart_count > 0 {
				println!("  Restart count: {}", restart_info.restart_count);
				if restart_info.consecutive_failures > 0 {
					println!(
						"  Consecutive failures: {}",
						restart_info.consecutive_failures
					);
				}
			}

			// Show last health check time
			if let Some(last_check) = restart_info.last_health_check {
				if let Ok(duration) = std::time::SystemTime::now().duration_since(last_check) {
					println!("  Last checked: {}s ago", duration.as_secs());
				}
			}
		}
	}

	println!();
	println!("{}", "Health check completed. Dead servers will be automatically restarted by the health monitor.".bright_blue());
	Ok(false)
}

async fn handle_mcp_dump(config: &Config, role: &str) -> Result<bool> {
	// Dump raw tool definitions in JSON format for debugging
	println!();
	println!("{}", "Raw MCP Tool Definitions (JSON)".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	let config_for_role = config.get_merged_config_for_role(role);
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;

	if available_functions.is_empty() {
		println!("{}", "No tools available.".yellow());
	} else {
		for (i, func) in available_functions.iter().enumerate() {
			println!();
			println!("{}. {}", i + 1, func.name.bright_white().bold());
			println!(
				"{}",
				serde_json::to_string_pretty(&serde_json::json!({
					"name": func.name,
					"description": func.description,
					"parameters": func.parameters
				}))
				.unwrap_or_default()
			);
		}
	}

	println!();
	println!(
		"{}",
		"Use this output to debug tool schema validation issues.".dimmed()
	);
	Ok(false)
}

async fn handle_mcp_validate(config: &Config, role: &str) -> Result<bool> {
	// Validate tool schema definitions
	println!();
	println!("{}", "MCP Tool Schema Validation".bright_cyan().bold());
	println!("{}", "─".repeat(50).dimmed());

	let config_for_role = config.get_merged_config_for_role(role);
	let available_functions = crate::mcp::get_available_functions(&config_for_role).await;

	if available_functions.is_empty() {
		println!("{}", "No tools available to validate.".yellow());
	} else {
		let mut all_valid = true;

		for (i, func) in available_functions.iter().enumerate() {
			println!();
			println!("{}. Validating {}", i + 1, func.name.bright_white().bold());

			let mut issues = Vec::new();

			// Check if parameters has "type" field OR "oneOf" field (MCP schema requirement)
			let has_type = func.parameters.get("type").is_some();
			let has_one_of = func.parameters.get("oneOf").is_some();

			if !has_type && !has_one_of {
				issues.push("Missing 'type' or 'oneOf' field in root schema".to_string());
			}

			// Check if properties exist and have proper type definitions
			if let Some(properties) = func.parameters.get("properties") {
				if let Some(props_obj) = properties.as_object() {
					for (prop_name, prop_def) in props_obj {
						let prop_has_type = prop_def.get("type").is_some();
						let prop_has_one_of = prop_def.get("oneOf").is_some();

						if !prop_has_type && !prop_has_one_of {
							issues.push(format!(
								"Property '{}' missing 'type' or 'oneOf' field",
								prop_name
							));
						}
					}
				}
			} else if has_type {
				// Only require properties if we have a type field (not for oneOf schemas)
				let schema_type = func.parameters.get("type").and_then(|t| t.as_str());
				if schema_type == Some("object") {
					issues.push("Object type schema missing 'properties' field".to_string());
				}
			}

			if issues.is_empty() {
				println!("  {}", "✅ Valid schema".bright_green());
			} else {
				all_valid = false;
				println!("  {}", "❌ Schema issues found:".bright_red());
				for issue in issues {
					println!("    - {}", issue.yellow());
				}
			}
		}

		println!();
		if all_valid {
			println!("{}", "✅ All tool schemas are valid!".bright_green());
		} else {
			println!(
				"{}",
				"❌ Some tool schemas have validation issues.".bright_red()
			);
			println!(
				"{}",
				"These issues may cause API errors with providers like Anthropic.".yellow()
			);
		}
	}
	Ok(false)
}

fn handle_mcp_invalid() -> Result<bool> {
	// Invalid subcommand
	println!();
	println!("{}", "Invalid MCP subcommand.".bright_red());
	println!();
	println!("{}", "Available subcommands:".bright_cyan());
	println!("  {} - Show tool names only", "/mcp list".cyan());
	println!(
		"  {} - Show server status and tools with descriptions (default)",
		"/mcp info".cyan()
	);
	println!(
		"  {} - Show full details including parameters",
		"/mcp full".cyan()
	);
	println!(
		"  {} - Check server health and attempt restart if needed",
		"/mcp health".cyan()
	);
	println!(
		"  {} - Dump raw tool definitions in JSON format",
		"/mcp dump".cyan()
	);
	println!();
	println!(
		"  {} - Validate tool schema definitions",
		"/mcp validate".cyan()
	);
	println!();
	println!(
		"{}",
		"Usage: /mcp [list|info|full|health|dump|validate]".bright_blue()
	);
	Ok(false)
}
