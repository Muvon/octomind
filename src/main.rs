use clap::{Parser, Subcommand};

use octodev::config::Config;
use octodev::store::Store;
use octodev::session;

mod commands;

#[derive(Parser)]
#[command(name = "octodev")]
#[command(version = "0.1.0")]
#[command(about = "Octodev is smart developer assistant based on your codebase")]
struct OctodevArgs {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Subcommand)]
enum Commands {
	/// Index the current directory's codebase
	Index(commands::IndexArgs),

	/// Search the codebase with a natural language query
	Search(commands::SearchArgs),

	/// View file signatures (functions, methods, etc.)
	View(commands::ViewArgs),

	/// Watch for changes in the codebase and reindex automatically
	Watch(commands::WatchArgs),

	/// Generate a default configuration file
	Config(commands::ConfigArgs),

	/// Start an interactive coding session
	Session(commands::SessionArgs),

	/// Query and explore the code relationship graph (GraphRAG)
	#[command(name = "graphrag")]
	GraphRAG(commands::GraphRAGArgs),

	/// Clear all database tables (useful for debugging)
	Clear,
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
		return commands::config::execute(config_args, config);
	}

	// Initialize the store
	let store = Store::new().await?;
	store.initialize_collections().await?;

	// Execute the appropriate command
	match &args.command {
		Commands::Index(index_args) => {
			commands::index::execute(&store, &config, index_args).await?
		},
		Commands::Search(search_args) => {
			commands::search::execute(&store, search_args, &config).await?
		},
		Commands::View(view_args) => {
			commands::view::execute(&store, view_args, &config).await?
		},
		Commands::Watch(watch_args) => {
			commands::watch::execute(&store, &config, watch_args).await?
		},
		Commands::Session(session_args) => {
			session::chat::run_interactive_session(session_args, &store, &config).await?
		},
		Commands::GraphRAG(graphrag_args) => {
			commands::graphrag::execute(&store, graphrag_args, &config).await?
		},
		Commands::Clear => {
			commands::clear::execute(&store).await?
		},
		Commands::Config(_) => unreachable!(), // Already handled above
	}

	Ok(())
}