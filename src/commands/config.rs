use clap::Args;

use octodev::config::{Config, McpServerConfig, McpServerMode, McpServerType};
use octodev::directories;

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

	/// Enable MCP protocol
	#[arg(long)]
	pub mcp_enable: Option<bool>,

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

	/// Show current configuration values with defaults
	#[arg(long)]
	pub show: bool,

	/// Validate configuration without making changes
	#[arg(long)]
	pub validate: bool,
}

// Handle the configuration command
pub fn execute(args: &ConfigArgs, mut config: Config) -> Result<(), anyhow::Error> {
	// If show flag is set, display current configuration with defaults and exit
	if args.show {
		show_configuration(&config)?;
		return Ok(());
	}

	// If validation flag is set, just validate and exit
	if args.validate {
		match config.validate() {
			Ok(()) => {
				println!("✅ Configuration is valid!");
				return Ok(());
			}
			Err(e) => {
				eprintln!("❌ Configuration validation failed: {}", e);
				return Err(e);
			}
		}
	}

	let mut modified = false;

	// Set root-level model if specified
	if let Some(model) = &args.model {
		// Validate model format
		if !model.contains(':') {
			eprintln!("Error: Model must be in provider:model format (e.g., openrouter:anthropic/claude-3.5-sonnet)");
			return Ok(());
		}
		
		config.model = model.clone();
		println!("Set root-level model to {}", model);
		modified = true;
	}

	// Set API key for provider if specified
	if let Some(api_key_input) = &args.api_key {
		// Parse provider:key format
		let parts: Vec<&str> = api_key_input.splitn(2, ':').collect();
		if parts.len() != 2 {
			eprintln!("Error: API key must be in provider:key format (e.g., openrouter:your-key)");
			return Ok(());
		}

		let provider = parts[0];
		let key = parts[1];

		match provider {
			"openrouter" => {
				config.providers.openrouter.api_key = Some(key.to_string());
				println!("Set OpenRouter API key");
			}
			"openai" => {
				config.providers.openai.api_key = Some(key.to_string());
				println!("Set OpenAI API key");
			}
			"anthropic" => {
				config.providers.anthropic.api_key = Some(key.to_string());
				println!("Set Anthropic API key");
			}
			"google" => {
				config.providers.google.api_key = Some(key.to_string());
				println!("Set Google API key");
			}
			"amazon" => {
				config.providers.amazon.api_key = Some(key.to_string());
				println!("Set Amazon API key");
			}
			"cloudflare" => {
				config.providers.cloudflare.api_key = Some(key.to_string());
				println!("Set Cloudflare API key");
			}
			_ => {
				eprintln!("Error: Unsupported provider '{}'. Supported: openrouter, openai, anthropic, google, amazon, cloudflare", provider);
				return Ok(());
			}
		}
		modified = true;
	}

	// Set log level if specified
	if let Some(log_level_str) = &args.log_level {
		match log_level_str.to_lowercase().as_str() {
			"none" => {
				config.log_level = octodev::config::LogLevel::None;
				println!("Set log level to None");
			}
			"info" => {
				config.log_level = octodev::config::LogLevel::Info;
				println!("Set log level to Info");
			}
			"debug" => {
				config.log_level = octodev::config::LogLevel::Debug;
				println!("Set log level to Debug");
			}
			_ => {
				eprintln!("Error: Invalid log level '{}'. Valid options: none, info, debug", log_level_str);
				return Ok(());
			}
		}
		modified = true;
	}

	// Enable/disable MCP protocol
	if let Some(enable_mcp) = args.mcp_enable {
		config.mcp.enabled = enable_mcp;
		println!("MCP protocol {}", if enable_mcp { "enabled" } else { "disabled" });
		modified = true;
	}

	// Enable/disable markdown rendering
	if let Some(enable_markdown) = args.markdown_enable {
		config.enable_markdown_rendering = enable_markdown;
		println!("Markdown rendering {}", if enable_markdown { "enabled" } else { "disabled" });
		modified = true;
	}

	// Update MCP server references if specified
	if let Some(providers) = &args.mcp_providers {
		let server_names: Vec<String> = providers
			.split(',')
			.map(|s| s.trim().to_string())
			.collect();

		// Clear existing servers and add new ones
		config.mcp.servers.clear();
		for server_name in &server_names {
			// Create basic server config if not exists
			if !config.mcp.servers.contains_key(server_name) {
				config.mcp.servers.insert(
					server_name.clone(),
					McpServerConfig::from_name(server_name)
				);
			}
		}

		println!("Set MCP servers to: {}", providers);
		modified = true;
	}

	// Configure MCP server if specified
	if let Some(server_config) = &args.mcp_server {
		// Parse server config string: name,url=X|command=Y,args=Z
		let parts: Vec<&str> = server_config.split(',').collect();

		if parts.len() < 2 {
			println!("Invalid MCP server configuration format. Expected format: name,url=X|command=Y,args=Z");
		} else {
			let name = parts[0].trim().to_string();

			// Create a new server config
			let mut server = McpServerConfig {
				enabled: true,
				name: name.clone(),
				server_type: McpServerType::External, // Default to external type
				url: None,
				command: None,
				args: Vec::new(),
				auth_token: None,
				mode: McpServerMode::Http, // Default to HTTP mode
				tools: Vec::new(),
				timeout_seconds: 30, // Default timeout
			};

			// Process remaining parts
			for part in &parts[1..] {
				let kv: Vec<&str> = part.split('=').collect();
				if kv.len() == 2 {
					let key = kv[0].trim();
					let value = kv[1].trim();

					match key {
						"url" => {
							server.url = Some(value.to_string());
						},
						"command" => {
							server.command = Some(value.to_string());
						},
						"args" => {
							server.args = value.split(' ')
								.map(|s| s.trim().to_string())
								.filter(|s| !s.is_empty())
								.collect();
						},
						"token" | "auth_token" => {
							server.auth_token = Some(value.to_string());
						},
						"mode" => {
							match value.to_lowercase().as_str() {
								"http" => server.mode = McpServerMode::Http,
								"stdin" => server.mode = McpServerMode::Stdin,
								_ => println!("Unknown server mode: {}, defaulting to HTTP", value),
							}
						},
						"timeout" | "timeout_seconds" => {
							if let Ok(timeout) = value.parse::<u64>() {
								server.timeout_seconds = timeout;
							} else {
								println!("Invalid timeout value: {}, using default", value);
							}
						},
						_ => {
							println!("Unknown server config key: {}", key);
						}
					}
				}
			}

			// Validate the server config
			match server.server_type {
				McpServerType::External => {
					if server.url.is_none() && server.command.is_none() {
						println!("Error: Either url or command must be specified for external MCP server");
						return Ok(());
					}
				}
				_ => {
					// Built-in servers are always valid
				}
			}

			// Enable MCP if not already enabled
			if !config.mcp.enabled {
				config.mcp.enabled = true;
				println!("Automatically enabled MCP protocol for server integration");
			}

			// Add the new server to registry
			config.mcp.servers.insert(name.clone(), server);

			println!("Added/updated MCP server: {}", name);
			modified = true;
		}
	}

	// Update system prompt if specified
	if let Some(system_prompt) = &args.system {
		if system_prompt.to_lowercase() == "default" {
			// Reset to default
			config.system = None;
			println!("Reset system prompt to default");
		} else {
			// Set custom prompt
			config.system = Some(system_prompt.clone());
			println!("Set custom system prompt");
		}
		modified = true;
	}

	// If no modifications were made, create a default config
	if !modified {
		// Check if config file already exists
		let config_path = directories::get_config_file_path()?;

		if config_path.exists() {
			println!("Configuration file already exists at: {}", config_path.display());
			println!("No changes were made to the configuration.");
		} else {
			let config_path = Config::create_default_config()?;
			println!("Created default configuration file at: {}", config_path.display());
		}
	} else {
		// Save the updated configuration
		if let Err(e) = config.save() {
			eprintln!("Error saving configuration: {}", e);
			return Err(e);
		}
		println!("Configuration saved successfully");
	}

	// Show current configuration
	println!("\nCurrent configuration:");

	// Show root-level model
	println!("Root model: {}", config.get_effective_model());

	// Show provider API keys
	println!("Provider API keys:");
	show_api_key_status("  OpenRouter", &config.providers.openrouter.api_key, "OPENROUTER_API_KEY");
	show_api_key_status("  OpenAI", &config.providers.openai.api_key, "OPENAI_API_KEY");
	show_api_key_status("  Anthropic", &config.providers.anthropic.api_key, "ANTHROPIC_API_KEY");
	show_api_key_status("  Google", &config.providers.google.api_key, "GOOGLE_APPLICATION_CREDENTIALS");
	show_api_key_status("  Amazon", &config.providers.amazon.api_key, "AWS_ACCESS_KEY_ID");
	show_api_key_status("  Cloudflare", &config.providers.cloudflare.api_key, "CLOUDFLARE_API_TOKEN");

	// Show role configurations
	println!("Role configurations:");
	println!("  Developer model: {}", config.developer.config.model);
	println!("  Assistant model: {}", config.assistant.config.model);
	
	// Show MCP status using the new structure
	println!("MCP protocol: {}", if config.mcp.enabled { "enabled" } else { "disabled" });

	// Show MCP servers from global config
	if config.mcp.enabled {
		if !config.mcp.servers.is_empty() {
			println!("MCP servers:");
			for (name, server) in &config.mcp.servers {
				let status = if server.enabled { "enabled" } else { "disabled" };

				// Auto-detect server type for display
				let effective_type = match name.as_str() {
					"developer" => McpServerType::Developer,
					"filesystem" => McpServerType::Filesystem,
					_ => McpServerType::External,
				};

				match effective_type {
					McpServerType::Developer => println!("  - {} (built-in developer tools) - {}", name, status),
					McpServerType::Filesystem => println!("  - {} (built-in filesystem tools) - {}", name, status),
					McpServerType::External => {
						if name == "octocode" {
							if server.enabled {
								println!("  - {} (codebase analysis) - {} ✓", name, status);
							} else {
								println!("  - {} (codebase analysis) - {} (binary not found in PATH)", name, status);
							}
						} else if let Some(url) = &server.url {
							println!("  - {} (HTTP: {}) - {}", name, url, status);
						} else if let Some(command) = &server.command {
							println!("  - {} (Command: {}) - {}", name, command, status);
						} else {
							println!("  - {} (external, not configured) - {}", name, status);
						}
					}
				}
			}
		} else {
			println!("MCP servers: None configured");
		}
	}

	println!("Log level: {:?}", config.log_level);
	println!("Markdown rendering: {}", if config.enable_markdown_rendering { "enabled" } else { "disabled" });

	// Show system prompt status
	if config.system.is_some() {
		println!("System prompt: Custom");
	} else {
		println!("System prompt: Default");
	}

	Ok(())
}

/// Display comprehensive configuration information with defaults
fn show_configuration(config: &Config) -> Result<(), anyhow::Error> {
	println!("🔧 Octodev Configuration\n");

	// Configuration file location
	let config_path = directories::get_config_file_path()?;
	if config_path.exists() {
		println!("📁 Config file: {}", config_path.display());
	} else {
		println!("📁 Config file: {} (not created yet)", config_path.display());
	}
	println!();

	// Root-level configuration
	println!("🌍 System-wide Settings");
	println!("  Model (root):              {}", 
		if config.model.is_empty() || config.model == "openrouter:anthropic/claude-3.5-haiku" {
			format!("{} (default)", config.get_effective_model())
		} else {
			config.model.clone()
		}
	);
	println!("  Log level:                 {:?}", config.log_level);
	println!("  Markdown rendering:        {}", if config.enable_markdown_rendering { "enabled" } else { "disabled" });
	println!("  MCP response warning:      {} tokens", config.mcp_response_warning_threshold);
	println!("  Max request tokens:        {} tokens", config.max_request_tokens_threshold);
	println!("  Auto-truncation:           {}", if config.enable_auto_truncation { "enabled" } else { "disabled" });
	println!("  Cache percentage threshold: {}%", config.cache_tokens_pct_threshold);
	println!("  Cache absolute threshold:  {} tokens", config.cache_tokens_absolute_threshold);
	println!("  Cache timeout:             {} seconds", config.cache_timeout_seconds);
	println!();

	// Provider API keys
	println!("🔑 Provider API Keys");
	show_api_key_status("OpenRouter", &config.providers.openrouter.api_key, "OPENROUTER_API_KEY");
	show_api_key_status("OpenAI", &config.providers.openai.api_key, "OPENAI_API_KEY");
	show_api_key_status("Anthropic", &config.providers.anthropic.api_key, "ANTHROPIC_API_KEY");
	show_api_key_status("Google", &config.providers.google.api_key, "GOOGLE_APPLICATION_CREDENTIALS");
	show_api_key_status("Amazon", &config.providers.amazon.api_key, "AWS_ACCESS_KEY_ID");
	show_api_key_status("Cloudflare", &config.providers.cloudflare.api_key, "CLOUDFLARE_API_TOKEN");
	println!();

	// Role configurations
	println!("👤 Role Configurations");
	
	// Developer role
	println!("  Developer Role:");
	let (dev_config, dev_mcp, dev_layers, _dev_commands, dev_system) = config.get_mode_config("developer");
	println!("    Model:           {}", dev_config.model);
	println!("    Layers enabled:  {}", dev_config.enable_layers);
	if let Some(_system) = dev_system {
		println!("    System prompt:   Custom");
	} else {
		println!("    System prompt:   Default");
	}

	// Assistant role
	println!("  Assistant Role:");
	let (ass_config, ass_mcp, _ass_layers, _ass_commands, ass_system) = config.get_mode_config("assistant");
	println!("    Model:           {}", ass_config.model);
	println!("    Layers enabled:  {}", ass_config.enable_layers);
	if let Some(_system) = ass_system {
		println!("    System prompt:   Custom");
	} else {
		println!("    System prompt:   Default");
	}
	println!();

	// MCP Configuration
	println!("🔧 MCP (Model Context Protocol) Configuration");
	
	// Global MCP
	println!("  Global MCP:");
	println!("    Enabled:         {}", config.mcp.enabled);
	if config.mcp.enabled {
		show_mcp_servers(&config.mcp.servers);
	}

	// Developer role MCP
	println!("  Developer Role MCP:");
	println!("    Enabled:         {}", dev_mcp.enabled);
	if dev_mcp.enabled {
		show_mcp_servers(&dev_mcp.servers);
	}

	// Assistant role MCP
	println!("  Assistant Role MCP:");
	println!("    Enabled:         {}", ass_mcp.enabled);
	if ass_mcp.enabled {
		show_mcp_servers(&ass_mcp.servers);
	}
	println!();

	// Layer configurations
	if dev_config.enable_layers || ass_config.enable_layers {
		println!("📚 Layer Configurations");
		
		if let Some(layers) = dev_layers {
			println!("  Developer Role Layers: {} configured", layers.len());
			for layer in layers {
				if layer.enabled {
					println!("    ✅ {} (temp: {:.1})", layer.name, layer.temperature);
				} else {
					println!("    ❌ {} (disabled)", layer.name);
				}
			}
		}

		if let Some(layers) = &config.layers {
			println!("  Global Layers: {} configured", layers.len());
			for layer in layers {
				if layer.enabled {
					println!("    ✅ {} (temp: {:.1})", layer.name, layer.temperature);
				} else {
					println!("    ❌ {} (disabled)", layer.name);
				}
			}
		}
		println!();
	}

	Ok(())
}

/// Show the status of an API key with environment variable fallback
fn show_api_key_status(provider: &str, config_key: &Option<String>, env_var: &str) {
	match config_key {
		Some(key) => println!("{:<15} Set in config ({})", provider, mask_key(key)),
		None => {
			if std::env::var(env_var).is_ok() {
				println!("{:<15} Set via {} environment variable", provider, env_var);
			} else {
				println!("{:<15} Not set", provider);
			}
		}
	}
}

/// Display MCP server configurations
fn show_mcp_servers(servers: &std::collections::HashMap<String, McpServerConfig>) {
	if servers.is_empty() {
		println!("    Servers:         None configured");
		return;
	}

	println!("    Servers:");
	for (name, server) in servers {
		let status = if server.enabled { "✅" } else { "❌" };
		
		// Auto-detect server type for display
		let effective_type = match name.as_str() {
			"developer" => McpServerType::Developer,
			"filesystem" => McpServerType::Filesystem,
			_ => McpServerType::External,
		};

		match effective_type {
			McpServerType::Developer => {
				println!("      {} {} (built-in developer tools)", status, name);
			},
			McpServerType::Filesystem => {
				println!("      {} {} (built-in filesystem tools)", status, name);
			},
			McpServerType::External => {
				if name == "octocode" {
					if server.enabled {
						println!("      {} {} (codebase analysis)", status, name);
					} else {
						println!("      {} {} (binary not found in PATH)", status, name);
					}
				} else if let Some(url) = &server.url {
					println!("      {} {} (HTTP: {})", status, name, url);
				} else if let Some(command) = &server.command {
					println!("      {} {} (Command: {})", status, name, command);
				} else {
					println!("      {} {} (external, not configured)", status, name);
				}
			}
		}

		// Show additional server details if configured
		if server.timeout_seconds != 30 {
			println!("        Timeout: {} seconds", server.timeout_seconds);
		}
		if !server.tools.is_empty() {
			println!("        Tools: {}", server.tools.join(", "));
		}
	}
}

/// Mask an API key for display purposes
fn mask_key(key: &str) -> String {
	if key.len() <= 8 {
		"*".repeat(key.len())
	} else {
		format!("{}...{}", &key[..4], &key[key.len()-4..])
	}
}