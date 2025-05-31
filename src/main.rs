use clap::{Parser, Subcommand};

use octodev::config::Config;
use octodev::session;

mod commands;

#[derive(Parser)]
#[command(name = "octodev")]
#[command(version = "0.1.0")]
#[command(about = "Octodev is a smart AI developer assistant with configurable MCP support")]
struct CliArgs {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Subcommand)]
enum Commands {
	/// Generate a default configuration file
	Config(commands::ConfigArgs),

	/// Start an interactive coding session
	Session(commands::SessionArgs),

	/// Ask a question and get an AI response without session management
	Ask(commands::AskArgs),

	/// Execute shell commands through AI with confirmation
	Shell(commands::ShellArgs),
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
	let args = CliArgs::parse();

	// Load configuration
	let config = Config::load()?;

	// Setup cleanup for MCP server processes when the program exits
	let result = run_with_cleanup(args, config).await;

	// Make sure to clean up any started server processes
	if let Err(e) = octodev::mcp::server::cleanup_servers() {
		eprintln!("Warning: Error cleaning up MCP servers: {}", e);
	}

	result
}

async fn run_with_cleanup(args: CliArgs, config: Config) -> Result<(), anyhow::Error> {
	// Execute the appropriate command
	match &args.command {
		Commands::Config(config_args) => {
			commands::config::execute(config_args, config)?
		},
		Commands::Session(session_args) => {
			session::chat::run_interactive_session(session_args, &config).await?
		},
		Commands::Ask(ask_args) => {
			commands::ask::execute(ask_args, &config).await?
		},
		Commands::Shell(shell_args) => {
			commands::shell::execute(shell_args, &config).await?
		},
	}

	Ok(())
}
