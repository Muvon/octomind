use std::io::Write;
use std::sync::Arc;
use parking_lot::RwLock;
use clap::{Parser, Subcommand, Args};

use octodev::config::Config;
use octodev::store::Store;
use octodev::state;
use octodev::indexer;
use octodev::session;

#[derive(Parser)]
#[command(name = "octodev")]
#[command(version = "0.1.0")]
#[command(about = "OctoDev is smart developer assistant based on your codebase")]
struct OctodevArgs {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index the current directory's codebase
    Index,

    /// Search the codebase with a natural language query
    Search(SearchArgs),

    /// Watch for changes in the codebase and reindex automatically
    Watch,

    /// Generate a default configuration file
    Config(ConfigArgs),
    
    /// Start an interactive coding session
    Session(SessionArgs),
}

#[derive(Args)]
struct SearchArgs {
    /// Search query
    query: String,

    /// Expand all symbols in matching code blocks
    #[arg(long, short)]
    expand: bool,

    /// Output in JSON format
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ConfigArgs {
    /// Set the embedding provider (jina or fastembed)
    #[arg(long, short)]
    provider: Option<String>,

    /// Set the Jina API key
    #[arg(long)]
    jina_key: Option<String>,

    /// Set the OpenRouter API key
    #[arg(long)]
    openrouter_key: Option<String>,
    
    /// Set the OpenRouter model
    #[arg(long)]
    openrouter_model: Option<String>,

    /// Set the code embedding model for FastEmbed
    #[arg(long)]
    fastembed_code_model: Option<String>,

    /// Set the text embedding model for FastEmbed
    #[arg(long)]
    fastembed_text_model: Option<String>,
    
    /// Enable MCP protocol
    #[arg(long)]
    mcp_enable: Option<bool>,
    
    /// Set MCP providers
    #[arg(long)]
    mcp_providers: Option<String>,
    
    /// Add/configure MCP server (format: name,url=X|command=Y,args=Z)
    #[arg(long)]
    mcp_server: Option<String>,
}

#[derive(Args, Debug)]
struct SessionArgs {
    /// Name of the session to start or resume
    #[arg(long, short)]
    name: Option<String>,
    
    /// Resume an existing session
    #[arg(long, short)]
    resume: Option<String>,
    
    /// Use a specific model instead of the one configured in config
    #[arg(long)]
    model: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args = OctodevArgs::parse();

    // Load configuration - ensure .octodev directory exists
    let config = Config::load()?;

    // Setup cleanup for MCP server processes when the program exits
    let result = run_with_cleanup(args, config).await;
    
    // Make sure to clean up any started server processes
    if let Err(e) = octodev::session::mcp::server::cleanup_servers() {
        eprintln!("Warning: Error cleaning up MCP servers: {}", e);
    }
    
    result
}

async fn run_with_cleanup(args: OctodevArgs, config: Config) -> Result<(), anyhow::Error> {
    // Handle the config command separately
    if let Commands::Config(config_args) = &args.command {
        return handle_config_command(config_args, config);
    }

    // Initialize the store
    let store = Store::new().await?;
    store.initialize_collections().await?;

    // Execute the appropriate command
    match &args.command {
        Commands::Index => {
            index_codebase(&store, &config).await?
        },
        Commands::Search(search_args) => {
            search_codebase(&store, search_args, &config).await?
        },
        Commands::Watch => {
            watch_codebase(&store, &config).await?
        },
        Commands::Session(session_args) => {
            session::chat::run_interactive_session(session_args, &store, &config).await?
        },
        Commands::Config(_) => unreachable!(), // Already handled above
    }

    Ok(())
}

// Handle the configuration command
fn handle_config_command(args: &ConfigArgs, mut config: Config) -> Result<(), anyhow::Error> {
    use octodev::config::EmbeddingProvider;
    
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
    
    // Update MCP providers if specified
    if let Some(providers) = &args.mcp_providers {
        let provider_list: Vec<String> = providers
            .split(',') 
            .map(|s| s.trim().to_string()) 
            .collect();
        config.mcp.providers = provider_list;
        println!("Set MCP providers to: {}", providers);
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
            let mut server = octodev::config::McpServerConfig {
                enabled: true,
                name: name.clone(),
                url: None,
                command: None,
                args: Vec::new(),
                auth_token: None,
                mode: octodev::config::McpServerMode::Http, // Default to HTTP mode
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
                                "http" => server.mode = octodev::config::McpServerMode::Http,
                                "stdin" => server.mode = octodev::config::McpServerMode::Stdin,
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
            if server.url.is_none() && server.command.is_none() {
                println!("Error: Either url or command must be specified for MCP server");
            } else {
                // Enable MCP if not already enabled
                if !config.mcp.enabled {
                    config.mcp.enabled = true;
                    println!("Automatically enabled MCP protocol for server integration");
                }
                
                // Remove any existing server with the same name
                config.mcp.servers.retain(|s| s.name != name);
                
                // Add the new server
                config.mcp.servers.push(server);
                println!("Added/updated MCP server: {}", name);
                modified = true;
            }
        }
    }

    // If no modifications were made, create a default config
    if !modified {
        let config_path = Config::create_default_config()?;
        println!("Created default configuration file at: {}", config_path.display());
    } else {
        // Save the updated configuration
        config.save()?;
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
    println!("MCP providers: {}", config.mcp.providers.join(", "));

    Ok(())
}

async fn index_codebase(store: &Store, config: &Config) -> Result<(), anyhow::Error> {
    let current_dir = std::env::current_dir()?;
    println!("Indexing current directory: {}", current_dir.display());

    let state = state::create_shared_state();
    state.write().current_directory = current_dir;

    // Spawn the progress display task
    let progress_handle = tokio::spawn(display_indexing_progress(state.clone()));

    // Start indexing
    indexer::index_files(store, state.clone(), config).await?;

    // Wait for the progress display to finish
    let _ = progress_handle.await;

    println!("✓ Indexing complete!");
    Ok(())
}

async fn search_codebase(store: &Store, args: &SearchArgs, config: &Config) -> Result<(), anyhow::Error> {
    let current_dir = std::env::current_dir()?;
    let octodev_dir = current_dir.join(".octodev");
    let index_path = octodev_dir.join("storage");

    // Check if we have an index already; if not, create one
    if !index_path.exists() {
        println!("No index found. Indexing current directory first");
        index_codebase(store, config).await?
    }

    println!("Searching for: {}", args.query);
    println!("Using embedding provider: {:?}", config.embedding_provider);

    // Generate embeddings for the query
    let embeddings = indexer::generate_embeddings(&args.query, true, config).await?;

    // Search for matching code blocks
    let mut results = store.get_code_blocks(embeddings).await?;

    // If expand flag is set, expand symbols in the results
    if args.expand {
        println!("Expanding symbols...");
        results = indexer::expand_symbols(store, results).await?;
    }

    // Output the results
    if args.json {
        indexer::render_results_json(&results)?
    } else {
        indexer::render_code_blocks(&results);
    }

    Ok(())
}

async fn watch_codebase(store: &Store, config: &Config) -> Result<(), anyhow::Error> {
    let current_dir = std::env::current_dir()?;
    println!("Starting watch mode for current directory: {}", current_dir.display());
    println!("Initial indexing...");

    // Do initial indexing
    index_codebase(store, config).await?;

    println!("Watching for changes (press Ctrl+C to stop)...");

    // Setup the file watcher with debouncer
    use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    let (tx, rx) = channel();

    // Create a debounced watcher to call our tx sender when files change
    let mut debouncer = new_debouncer(
        Duration::from_secs(2),
        move |res: Result<Vec<DebouncedEvent>, notify::Error>| {
            match res {
                Ok(events) => {
                    if !events.is_empty() {
                        let _ = tx.send(());
                    }
                }
                Err(e) => eprintln!("Error in file watcher: {:?}", e),
            }
        },
    )?;

    // Add the current directory to the watcher
    debouncer.watcher().watch(&current_dir, notify::RecursiveMode::Recursive)?;

    // Create shared state for reindexing
    let state = state::create_shared_state();
    state.write().current_directory = current_dir;

    // Keep a copy of the config for reindexing
    let config = config.clone();

    loop {
        // Wait for changes
        match rx.recv() {
            Ok(()) => {
                println!("\nDetected file changes, reindexing...");

                // Reset the indexing state
                let mut state_guard = state.write();
                state_guard.indexed_files = 0;
                state_guard.indexing_complete = false;
                drop(state_guard);

                // Reindex the codebase
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await; // Give a bit of time for all file changes to complete
                index_codebase(store, &config).await?;
            }
            Err(e) => {
                eprintln!("Watch error: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}

async fn display_indexing_progress(state: Arc<RwLock<state::IndexState>>) {
	let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
	let mut spinner_idx = 0;
	let mut last_indexed = 0;

	while !state.read().indexing_complete {
		let current_indexed = state.read().indexed_files;
		if current_indexed != last_indexed {
			print!("\r{} Indexing: {} files",
				spinner_chars[spinner_idx],
				current_indexed
			);
			std::io::stdout().flush().unwrap();
			last_indexed = current_indexed;
		} else {
			print!("\r{} Indexing: {} files",
				spinner_chars[spinner_idx],
				current_indexed
			);
			std::io::stdout().flush().unwrap();
		}

		spinner_idx = (spinner_idx + 1) % spinner_chars.len();
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
	}

	println!("\rIndexing complete! Total files indexed: {}", state.read().indexed_files);
}