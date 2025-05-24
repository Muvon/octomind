use clap::Args;

use octodev::config::{Config, EmbeddingProvider, McpServerConfig, McpServerMode, McpServerType};

#[derive(Args)]
pub struct ConfigArgs {
	/// Set the embedding provider (jina or fastembed)
	#[arg(long, short)]
	pub provider: Option<String>,

	/// Set the Jina API key
	#[arg(long)]
	pub jina_key: Option<String>,

	/// Set the OpenRouter API key
	#[arg(long)]
	pub openrouter_key: Option<String>,

	/// Set the OpenRouter model
	#[arg(long)]
	pub openrouter_model: Option<String>,

	/// Set the code embedding model for FastEmbed
	#[arg(long)]
	pub fastembed_code_model: Option<String>,

	/// Set the text embedding model for FastEmbed
	#[arg(long)]
	pub fastembed_text_model: Option<String>,

	/// Enable MCP protocol
	#[arg(long)]
	pub mcp_enable: Option<bool>,

	/// Set MCP providers
	#[arg(long)]
	pub mcp_providers: Option<String>,

	/// Add/configure MCP server (format: name,url=X|command=Y,args=Z)
	#[arg(long)]
	pub mcp_server: Option<String>,

	/// Enable GraphRAG for code relationship analysis
	#[arg(long)]
	pub graphrag_enable: Option<bool>,

	/// Set custom system prompt (or 'default' to reset to default)
	#[arg(long)]
	pub system: Option<String>,

	/// Enable markdown rendering for AI responses
	#[arg(long)]
	pub markdown_enable: Option<bool>,

	/// Validate configuration without making changes
	#[arg(long)]
	pub validate: bool,
}

// Handle the configuration command
pub fn execute(args: &ConfigArgs, mut config: Config) -> Result<(), anyhow::Error> {
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

	// Update provider if specified
	if let Some(provider) = &args.provider {
		match provider.to_lowercase().as_str() {
			"jina" => {
				config.embedding_provider = EmbeddingProvider::Jina;
				println!("Set embedding provider to Jina");
				modified = true;
			},
			"fastembed" => {
				config.embedding_provider = EmbeddingProvider::FastEmbed;
				println!("Set embedding provider to FastEmbed");
				modified = true;
			},
			_ => {
				println!("Unknown provider: {}", provider);
				println!("Valid providers are 'jina' or 'fastembed'.");
			},
		}
	}

	// Update Jina API key if specified
	if let Some(jina_key) = &args.jina_key {
		config.jina_api_key = Some(jina_key.clone());
		println!("Set Jina API key in configuration");
		modified = true;
	}

	// Update OpenRouter API key if specified
	if let Some(openrouter_key) = &args.openrouter_key {
		config.openrouter.api_key = Some(openrouter_key.clone());
		println!("Set OpenRouter API key in configuration");
		modified = true;
	}

	// Update OpenRouter model if specified
	if let Some(openrouter_model) = &args.openrouter_model {
		config.openrouter.model = openrouter_model.clone();
		println!("Set OpenRouter model to {}", openrouter_model);
		modified = true;
	}

	// Update FastEmbed code model if specified
	if let Some(code_model) = &args.fastembed_code_model {
		config.fastembed.code_model = code_model.clone();
		println!("Set FastEmbed code model to {}", code_model);
		modified = true;
	}

	// Update FastEmbed text model if specified
	if let Some(text_model) = &args.fastembed_text_model {
		config.fastembed.text_model = text_model.clone();
		println!("Set FastEmbed text model to {}", text_model);
		modified = true;
	}

	// Enable/disable MCP protocol
	if let Some(enable_mcp) = args.mcp_enable {
		config.mcp.enabled = enable_mcp;
		println!("MCP protocol {}", if enable_mcp { "enabled" } else { "disabled" });
		modified = true;
	}

	// Enable/disable GraphRAG
	if let Some(enable_graphrag) = args.graphrag_enable {
		config.graphrag.enabled = enable_graphrag;
		println!("GraphRAG {}", if enable_graphrag { "enabled" } else { "disabled" });
		modified = true;
	}

	// Enable/disable markdown rendering
	if let Some(enable_markdown) = args.markdown_enable {
		config.openrouter.enable_markdown_rendering = enable_markdown;
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
		let octodev_dir = Config::ensure_octodev_dir()?;
		let config_path = octodev_dir.join("config.toml");

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
	println!("Embedding provider: {:?}", config.embedding_provider);

	if let Some(key) = &config.jina_api_key {
		println!("Jina API key: {}", "*".repeat(key.len()));
	} else {
		println!("Jina API key: Not set (will use JINA_API_KEY environment variable if available)");
	}

	if let Some(key) = &config.openrouter.api_key {
		println!("OpenRouter API key: {}", "*".repeat(key.len()));
	} else {
		println!("OpenRouter API key: Not set (will use OPENROUTER_API_KEY environment variable if available)");
	}

	println!("OpenRouter model: {}", config.openrouter.model);
	println!("FastEmbed code model: {}", config.fastembed.code_model);
	println!("FastEmbed text model: {}", config.fastembed.text_model);
	println!("MCP protocol: {}", if config.mcp.enabled { "enabled" } else { "disabled" });
	
	// Show MCP servers
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
						if let Some(url) = &server.url {
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
	println!("GraphRAG: {}", if config.graphrag.enabled { "enabled" } else { "disabled" });
	println!("Markdown rendering: {}", if config.openrouter.enable_markdown_rendering { "enabled" } else { "disabled" });

	// Show system prompt status
	if config.system.is_some() {
		println!("System prompt: Custom");
	} else {
		println!("System prompt: Default");
	}

	Ok(())
}
