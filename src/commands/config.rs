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

use clap::Args;
use colored::Colorize;

use octomind::config::{Config, McpConnectionType, McpServerConfig};
use octomind::directories;
use octomind::session::chat::{
	block_close_err, block_close_ok, block_line, block_open, block_row, block_row_text,
	block_section, block_section_with, key_width,
};

#[derive(Args)]
pub struct ConfigArgs {
	/// Set the root-level model (provider:model format, e.g., openrouter:anthropic/claude-3.5-sonnet)
	#[arg(long)]
	pub model: Option<String>,

	/// Set API key for a provider (format: provider:key, e.g., openrouter:your-key)
	#[arg(long)]
	pub api_key: Option<String>,

	/// Set log level (none, info, debug)
	#[arg(long)]
	pub log_level: Option<String>,

	/// Set MCP providers
	#[arg(long)]
	pub mcp_providers: Option<String>,

	/// Add/configure MCP server (format: name,url=X|command=Y,args=Z)
	#[arg(long)]
	pub mcp_server: Option<String>,

	/// Set custom system prompt (or 'default' to reset to default)
	#[arg(long)]
	pub system: Option<String>,

	/// Enable markdown rendering for AI responses
	#[arg(long)]
	pub markdown_enable: Option<bool>,

	/// Set markdown theme (default, dark, light, ocean, solarized, monokai)
	#[arg(long)]
	pub markdown_theme: Option<String>,

	/// List all available markdown themes
	#[arg(long)]
	pub list_themes: bool,

	/// Show current configuration values with defaults
	#[arg(long)]
	pub show: bool,

	/// Validate configuration without making changes
	#[arg(long)]
	pub validate: bool,

	/// Upgrade config file to latest version
	#[arg(long)]
	pub upgrade: bool,
}

// Handle the configuration command
pub fn execute(args: &ConfigArgs, mut config: Config) -> Result<(), anyhow::Error> {
	// If list themes flag is set, display available themes and exit
	if args.list_themes {
		list_markdown_themes();
		return Ok(());
	}

	// If show flag is set, display current configuration with defaults and exit
	if args.show {
		show_configuration(&config)?;
		return Ok(());
	}

	// If validation flag is set, just validate and exit
	if args.validate {
		block_open("config validate", None);
		match config.validate() {
			Ok(()) => {
				block_line(&"Configuration is valid.".bright_green().to_string());
				block_close_ok("config validate", Some("valid"));
				println!();
				return Ok(());
			}
			Err(e) => {
				block_close_err("config validate", &e.to_string());
				println!();
				return Err(e);
			}
		}
	}

	// If upgrade flag is set, perform manual upgrade and exit
	if args.upgrade {
		let config_path = directories::get_config_file_path()?;
		octomind::config::migrations::force_upgrade_config(&config_path)?;
		return Ok(());
	}

	// Buffer the change confirmations so the whole modification path renders as
	// one /config block at the end (matches tool/command output style).
	let mut changes: Vec<(String, String)> = Vec::new();

	// Set root-level model if specified
	if let Some(model) = &args.model {
		if !model.contains(':') {
			block_open("config", None);
			block_close_err(
				"config",
				"model must be in provider:model format (e.g., openrouter:anthropic/claude-3.5-sonnet)",
			);
			println!();
			return Ok(());
		}
		config.model = model.clone();
		changes.push(("model".to_string(), model.clone()));
	}

	// Set API key for provider if specified — env vars only
	if let Some(api_key_input) = &args.api_key {
		let parts: Vec<&str> = api_key_input.splitn(2, ':').collect();
		if parts.len() != 2 {
			block_open("config", None);
			block_close_err(
				"config",
				"api key must be in provider:key format (e.g., openrouter:your-key)",
			);
			println!();
			return Ok(());
		}
		let provider = parts[0];
		block_open("config", None);
		block_line(
			&"API keys can no longer be set in config file for security reasons."
				.bright_red()
				.to_string(),
		);
		block_line(
			&format!(
				"Set the environment variable instead: export {}_API_KEY=your-key",
				provider.to_uppercase()
			)
			.dimmed()
			.to_string(),
		);
		block_close_err("config", "use environment variable");
		println!();
		return Ok(());
	}

	// Set log level if specified
	if let Some(log_level_str) = &args.log_level {
		match log_level_str.to_lowercase().as_str() {
			"none" => {
				config.log_level = octomind::config::LogLevel::None;
				changes.push(("log level".to_string(), "None".to_string()));
			}
			"info" => {
				config.log_level = octomind::config::LogLevel::Info;
				changes.push(("log level".to_string(), "Info".to_string()));
			}
			"debug" => {
				config.log_level = octomind::config::LogLevel::Debug;
				changes.push(("log level".to_string(), "Debug".to_string()));
			}
			_ => {
				block_open("config", None);
				block_close_err(
					"config",
					&format!("invalid log level '{}' (none, info, debug)", log_level_str),
				);
				println!();
				return Ok(());
			}
		}
	}

	if let Some(enable_markdown) = args.markdown_enable {
		config.enable_markdown_rendering = enable_markdown;
		changes.push((
			"markdown".to_string(),
			if enable_markdown {
				"enabled"
			} else {
				"disabled"
			}
			.to_string(),
		));
	}

	if let Some(theme) = &args.markdown_theme {
		let valid_themes = octomind::session::chat::markdown::MarkdownTheme::all_themes();
		if valid_themes.contains(&theme.as_str()) {
			config.markdown_theme = theme.clone();
			changes.push(("markdown theme".to_string(), theme.clone()));
		} else {
			block_open("config", None);
			block_close_err(
				"config",
				&format!(
					"invalid theme '{}' (valid: {})",
					theme,
					valid_themes.join(", ")
				),
			);
			println!();
			return Ok(());
		}
	}

	if let Some(providers) = &args.mcp_providers {
		let server_names: Vec<String> =
			providers.split(',').map(|s| s.trim().to_string()).collect();
		config.mcp.servers.clear();
		for server_name in &server_names {
			if !config.mcp.servers.iter().any(|s| s.name() == *server_name) {
				let server = McpServerConfig::builtin(server_name, 30, Vec::new());
				config.mcp.servers.push(server);
			}
		}
		changes.push(("mcp servers".to_string(), providers.clone()));
	}

	if let Some(server_config) = &args.mcp_server {
		let parts: Vec<&str> = server_config.split(',').collect();
		if parts.len() < 2 {
			block_open("config", None);
			block_close_err("config", "mcp server format: name,url=X|command=Y,args=Z");
			println!();
			return Ok(());
		}
		let name = parts[0].trim().to_string();
		let mut url: Option<String> = None;
		let mut command: Option<String> = None;
		let mut args_vec: Vec<String> = Vec::new();
		let mut timeout_seconds: u64 = 30;
		let mut connection_type = McpConnectionType::Http;
		let mut warnings: Vec<String> = Vec::new();

		for part in &parts[1..] {
			let kv: Vec<&str> = part.split('=').collect();
			if kv.len() == 2 {
				let key = kv[0].trim();
				let value = kv[1].trim();
				match key {
					"url" => url = Some(value.to_string()),
					"command" => command = Some(value.to_string()),
					"args" => {
						args_vec = value
							.split(' ')
							.map(|s| s.trim().to_string())
							.filter(|s| !s.is_empty())
							.collect()
					}
					"type" => match value.to_lowercase().as_str() {
						"http" => connection_type = McpConnectionType::Http,
						"stdio" => connection_type = McpConnectionType::Stdin,
						"builtin" => connection_type = McpConnectionType::Builtin,
						_ => warnings.push(format!(
							"unknown server type '{}', defaulting to HTTP",
							value
						)),
					},
					"timeout" | "timeout_seconds" => {
						if let Ok(timeout) = value.parse::<u64>() {
							timeout_seconds = timeout;
						} else {
							warnings.push(format!("invalid timeout '{}', using default", value));
						}
					}
					_ => warnings.push(format!("unknown server config key '{}'", key)),
				}
			}
		}

		let server = match connection_type {
			McpConnectionType::Builtin => {
				McpServerConfig::builtin(&name, timeout_seconds, Vec::new())
			}
			McpConnectionType::Http => match url {
				Some(url) => McpServerConfig::http(&name, &url, timeout_seconds, Vec::new()),
				None => {
					block_open("config", None);
					for w in &warnings {
						block_line(&w.yellow().to_string());
					}
					block_close_err("config", "url required for HTTP MCP server");
					println!();
					return Ok(());
				}
			},
			McpConnectionType::Stdin => match command {
				Some(command) => {
					McpServerConfig::stdin(&name, &command, args_vec, timeout_seconds, Vec::new())
				}
				None => {
					block_open("config", None);
					for w in &warnings {
						block_line(&w.yellow().to_string());
					}
					block_close_err("config", "command required for stdin MCP server");
					println!();
					return Ok(());
				}
			},
		};
		config.mcp.servers.retain(|s| s.name() != name);
		config.mcp.servers.push(server);
		changes.push(("mcp server".to_string(), format!("{} added/updated", name)));
		for w in warnings {
			changes.push(("warning".to_string(), w));
		}
	}

	if let Some(system_prompt) = &args.system {
		if system_prompt.to_lowercase() == "default" {
			config.system = None;
			changes.push(("system prompt".to_string(), "default".to_string()));
		} else {
			config.system = Some(system_prompt.clone());
			changes.push(("system prompt".to_string(), "custom".to_string()));
		}
	}

	let modified = !changes.is_empty();

	// One unified /config block: changes + save status + current state.
	block_open("config", None);

	if modified {
		block_section("changes");
		let kw = key_width(changes.iter().map(|(k, _)| k.as_str()));
		for (k, v) in &changes {
			block_row(k, &v.bright_green().to_string(), kw);
		}
		if let Err(e) = config.save() {
			block_close_err("config", &format!("save failed: {}", e));
			println!();
			return Err(e);
		}
		block_section("status");
		let kw = key_width(["saved"]);
		block_row(
			"saved",
			&"configuration written".bright_green().to_string(),
			kw,
		);
	} else {
		let config_path = directories::get_config_file_path()?;
		block_section("status");
		let kw = key_width(["config file"]);
		if config_path.exists() {
			block_row(
				"config file",
				&format!("{} (no changes)", config_path.display())
					.dimmed()
					.to_string(),
				kw,
			);
		} else {
			let new_path = Config::create_default_config()?;
			block_row(
				"config file",
				&format!("{} (created default)", new_path.display())
					.bright_green()
					.to_string(),
				kw,
			);
		}
	}

	// ── current state ─────────────────────────────────────────────────
	block_section("current");
	let cur_kw = key_width([
		"root model",
		"log level",
		"markdown",
		"theme",
		"system prompt",
	]);
	block_row(
		"root model",
		&config.get_effective_model().bright_white().to_string(),
		cur_kw,
	);
	block_row("log level", &format!("{:?}", config.log_level), cur_kw);
	block_row(
		"markdown",
		if config.enable_markdown_rendering {
			"enabled"
		} else {
			"disabled"
		},
		cur_kw,
	);
	block_row("theme", &config.markdown_theme, cur_kw);
	block_row(
		"system prompt",
		if config.system.is_some() {
			"custom"
		} else {
			"default"
		},
		cur_kw,
	);

	// ── env api keys ─────────────────────────────────────────────────
	block_section("api keys");
	render_env_api_key_row("OpenRouter", "OPENROUTER_API_KEY");
	render_env_api_key_row("OpenAI", "OPENAI_API_KEY");
	render_env_api_key_row("Anthropic", "ANTHROPIC_API_KEY");
	render_env_api_key_row("Google", "GOOGLE_APPLICATION_CREDENTIALS");
	render_env_api_key_row("Amazon", "AWS_ACCESS_KEY_ID");
	render_env_api_key_row("Cloudflare", "CLOUDFLARE_API_TOKEN");

	// ── mcp ───────────────────────────────────────────────────────────
	let dev_mcp_enabled = config
		.role_map
		.get("developer")
		.map(|r| !r.mcp.server_refs.is_empty())
		.unwrap_or(false);
	let ass_mcp_enabled = config
		.role_map
		.get("assistant")
		.map(|r| !r.mcp.server_refs.is_empty())
		.unwrap_or(false);
	block_section("mcp");
	let mcp_kw = key_width(["developer", "assistant"]);
	block_row(
		"developer",
		if dev_mcp_enabled {
			"enabled"
		} else {
			"disabled"
		},
		mcp_kw,
	);
	block_row(
		"assistant",
		if ass_mcp_enabled {
			"enabled"
		} else {
			"disabled"
		},
		mcp_kw,
	);

	if !config.mcp.servers.is_empty() {
		block_section("mcp servers");
		for server in &config.mcp.servers {
			let name = server.name();
			let effective_type = match name {
				"core" | "agent" => McpConnectionType::Builtin,
				_ => server.connection_type(),
			};
			let detail = match effective_type {
				McpConnectionType::Builtin => match name {
					"core" => "built-in core tools".dimmed().to_string(),
					"agent" => "built-in agent tool".dimmed().to_string(),
					_ => "built-in tools".dimmed().to_string(),
				},
				McpConnectionType::Http | McpConnectionType::Stdin => {
					if let Some(url) = server.url() {
						format!("HTTP: {}", url).dimmed().to_string()
					} else if let Some(command) = server.command() {
						format!("Command: {}", command).dimmed().to_string()
					} else {
						"external (not configured)".yellow().to_string()
					}
				}
			};
			block_row_text(&format!("{}  {}", name.bright_white(), detail));
		}
	}

	let suffix = if modified {
		Some(format!("{} change(s)", changes.len()))
	} else {
		Some("no changes".to_string())
	};
	block_close_ok("config", suffix.as_deref());
	println!();
	Ok(())
}

/// Render an env-var API key row under the current /config block.
fn render_env_api_key_row(provider: &str, env_var: &str) {
	match std::env::var(env_var) {
		Ok(value) if !value.trim().is_empty() => {
			let tracker = octomind::config::get_env_tracker();
			let source_desc = tracker.lock().unwrap().get_source_description(env_var);
			block_row_text(&format!(
				"{}  {} {}",
				format!("{:<11}", provider).bright_white(),
				"✓".bright_green(),
				format!("set via {}", source_desc).dimmed(),
			));
		}
		_ => {
			block_row_text(&format!(
				"{}  {} {}",
				format!("{:<11}", provider).bright_white(),
				"✗".bright_red(),
				format!("not set (export {})", env_var).dimmed(),
			));
		}
	}
}

/// Display available markdown themes with descriptions
fn list_markdown_themes() {
	let themes = vec![
		(
			"default",
			"Improved default theme with gold headers and enhanced contrast",
			"Most terminal setups",
		),
		(
			"dark",
			"Optimized for dark terminals with bright, vibrant colors",
			"Dark terminal backgrounds",
		),
		(
			"light",
			"Perfect for light terminal backgrounds with darker colors",
			"Light terminal backgrounds",
		),
		(
			"ocean",
			"Beautiful blue-green palette inspired by ocean themes",
			"Users who prefer calm, aquatic colors",
		),
		(
			"solarized",
			"Based on the popular Solarized color scheme",
			"Fans of the classic Solarized palette",
		),
		(
			"monokai",
			"Inspired by the popular Monokai syntax highlighting theme",
			"Users familiar with Monokai from code editors",
		),
	];

	block_open("config list-themes", None);
	for (name, description, best_for) in &themes {
		block_section_with(name, "");
		let kw = key_width(["description", "best for", "usage"]);
		block_row("description", &description.dimmed().to_string(), kw);
		block_row("best for", &best_for.dimmed().to_string(), kw);
		block_row(
			"usage",
			&format!("octomind config --markdown-theme {}", name).to_string(),
			kw,
		);
	}
	block_section("tips");
	block_row_text(
		&"Themes work in sessions, ask command, and multimode"
			.dimmed()
			.to_string(),
	);
	block_row_text(
		&"Enable markdown: octomind config --markdown-enable true"
			.dimmed()
			.to_string(),
	);
	block_row_text(
		&"View current theme: octomind config --show"
			.dimmed()
			.to_string(),
	);
	block_close_ok(
		"config list-themes",
		Some(&format!("{} theme(s)", themes.len())),
	);
	println!();
}

/// Display comprehensive configuration information with defaults
fn show_configuration(config: &Config) -> Result<(), anyhow::Error> {
	let config_path = directories::get_config_file_path()?;
	let subtitle = if config_path.exists() {
		config_path.display().to_string()
	} else {
		format!("{} (not created yet)", config_path.display())
	};
	block_open("config", Some(&subtitle));

	// ── system-wide ───────────────────────────────────────────────────
	block_section("system");
	let kw = key_width(["root model", "log level", "markdown", "theme", "max tokens"]);
	block_row(
		"root model",
		&if config.model.is_empty() || config.model == "openrouter:anthropic/claude-3.5-haiku" {
			format!("{} (default)", config.get_effective_model())
				.bright_white()
				.to_string()
		} else {
			config.model.bright_white().to_string()
		},
		kw,
	);
	block_row("log level", &format!("{:?}", config.log_level), kw);
	block_row(
		"markdown",
		if config.enable_markdown_rendering {
			"enabled"
		} else {
			"disabled"
		},
		kw,
	);
	block_row("theme", &config.markdown_theme, kw);
	block_row(
		"max tokens",
		&format!(
			"{} ({})",
			config.max_session_tokens_threshold,
			if config.max_session_tokens_threshold > 0 {
				"enabled"
			} else {
				"disabled"
			}
		),
		kw,
	);

	// ── api keys ──────────────────────────────────────────────────────
	block_section("api keys");
	render_env_api_key_row("OpenRouter", "OPENROUTER_API_KEY");
	render_env_api_key_row("OpenAI", "OPENAI_API_KEY");
	render_env_api_key_row("Anthropic", "ANTHROPIC_API_KEY");
	render_env_api_key_row("Google", "GOOGLE_APPLICATION_CREDENTIALS");
	render_env_api_key_row("Amazon", "AWS_ACCESS_KEY_ID");
	render_env_api_key_row("Cloudflare", "CLOUDFLARE_API_TOKEN");

	// ── roles ─────────────────────────────────────────────────────────
	let (_dev_config, dev_mcp, _dev_layers, _dev_commands, dev_system) =
		config.get_role_config("developer");
	let (_ass_config, ass_mcp, _ass_layers, _ass_commands, ass_system) =
		config.get_role_config("assistant");

	block_section("roles");
	let role_kw = key_width(["developer", "assistant"]);
	block_row(
		"developer",
		&format!(
			"{} (system-wide) {} {} char prompt",
			config.get_effective_model(),
			"·".bright_black(),
			dev_system.len()
		),
		role_kw,
	);
	block_row(
		"assistant",
		&format!(
			"{} (system-wide) {} {} char prompt",
			config.get_effective_model(),
			"·".bright_black(),
			ass_system.len()
		),
		role_kw,
	);

	// ── mcp ───────────────────────────────────────────────────────────
	block_section("mcp");
	let mcp_kw = key_width(["registry", "developer", "assistant"]);
	block_row(
		"registry",
		&format!("{} server(s)", config.mcp.servers.len()),
		mcp_kw,
	);
	block_row(
		"developer",
		&if dev_mcp.server_refs.is_empty() {
			"None (MCP disabled)".dimmed().to_string()
		} else {
			dev_mcp.server_refs.join(", ")
		},
		mcp_kw,
	);
	block_row(
		"assistant",
		&if ass_mcp.server_refs.is_empty() {
			"None (MCP disabled)".dimmed().to_string()
		} else {
			ass_mcp.server_refs.join(", ")
		},
		mcp_kw,
	);

	if !config.mcp.servers.is_empty() {
		block_section("mcp servers");
		for server in &config.mcp.servers {
			let name = server.name();
			let effective_type = match name {
				"core" | "agent" => McpConnectionType::Builtin,
				_ => server.connection_type(),
			};
			let detail = match effective_type {
				McpConnectionType::Builtin => match name {
					"core" => "built-in core tools".dimmed().to_string(),
					"agent" => "built-in agent tool".dimmed().to_string(),
					_ => "built-in tools".dimmed().to_string(),
				},
				McpConnectionType::Http | McpConnectionType::Stdin => {
					if let Some(url) = server.url() {
						format!("HTTP: {}", url).dimmed().to_string()
					} else if let Some(command) = server.command() {
						format!("Command: {}", command).dimmed().to_string()
					} else {
						"external (not configured)".yellow().to_string()
					}
				}
			};
			block_row_text(&format!("{}  {}", name.bright_white(), detail));
			if server.timeout_seconds() != 30 {
				block_row_text(
					&format!("  timeout: {}s", server.timeout_seconds())
						.dimmed()
						.to_string(),
				);
			}
			if !server.tools().is_empty() {
				block_row_text(
					&format!("  tools: {}", server.tools().join(", "))
						.dimmed()
						.to_string(),
				);
			}
		}
	}


	block_close_ok("config", Some(&config.get_effective_model()));
	println!();
	Ok(())
}
