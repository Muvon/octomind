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

// Import terminal output prelude to shadow std macros globally
// This automatically suspends the spinner before printing to prevent interference

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

use octomind::config::Config;

mod commands;

#[derive(Parser)]
#[command(name = "octomind")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Octomind is a smart AI developer assistant with configurable MCP support")]
struct CliArgs {
	#[command(subcommand)]
	command: Commands,
}

#[derive(Subcommand)]
enum Commands {
	/// Generate a default configuration file
	Config(commands::ConfigArgs),

	/// Run an agent or start an interactive session.
	/// TAG can be a registry agent (e.g. `developer:rust`) or a role name (e.g. `developer`).
	/// Use --format to run non-interactively.
	Run(commands::RunArgs),

	/// Start WebSocket server for remote AI sessions
	Server(commands::ServerArgs),

	/// Run as an ACP (Agent Client Protocol) agent over stdio
	Acp(commands::AcpArgs),

	/// Add a registry tap (agent source URL).
	/// Omit URL to list all active taps.
	Tap(commands::TapArgs),

	/// Remove a previously added registry tap.
	Untap(commands::UntapArgs),

	/// Show all available placeholder variables and their values
	Vars(commands::VarsArgs),

	/// Generate shell completion scripts
	Completion {
		/// The shell to generate completion for
		#[arg(value_enum)]
		shell: Shell,
	},
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
	// Initialize environment tracker before loading .env
	let _tracker = octomind::config::get_env_tracker();

	// Load .env file from current directory (if exists)
	// This will override existing environment variables with .env values
	if let Err(e) = octomind::config::get_env_tracker()
		.lock()
		.unwrap()
		.load_dotenv_override()
	{
		octomind::log_debug!("Failed to load .env file: {}", e);
	}

	// Seed the thread-local working directory with the real launch cwd immediately,
	// so get_thread_working_directory() never falls back to a wrong std::env::current_dir().
	let launch_cwd = std::env::current_dir().unwrap_or_default();
	octomind::mcp::set_session_working_directory(launch_cwd);

	let args = CliArgs::parse();

	// Load configuration
	let config = Config::load()?;

	// Setup cleanup for MCP server processes when the program exits
	let result = run_with_cleanup(args, config).await;

	// Make sure to clean up any started server processes
	if let Err(e) = octomind::mcp::server::cleanup_servers() {
		octomind::log_error!("Warning: Error cleaning up MCP servers: {}", e);
	}

	result
}

/// Initialize MCP servers and tool map for role-based commands with progress indicator
async fn initialize_mcp_for_role_with_progress(
	role: &str,
	config: &Config,
	is_interactive: bool,
) -> Result<(), anyhow::Error> {
	use indicatif::{ProgressBar, ProgressStyle};
	use std::time::Duration;

	// Only show spinner in interactive mode
	if is_interactive {
		let spinner = ProgressBar::new_spinner();
		spinner.set_style(
			ProgressStyle::default_spinner()
				.template(" {spinner:.cyan} {msg:.cyan}")
				.unwrap()
				.tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧"),
		);
		spinner.set_message("Initializing MCP servers...");
		spinner.enable_steady_tick(Duration::from_millis(80));

		// Create progress callback to update spinner message
		let progress_callback = |server_name: &str| {
			spinner.set_message(format!("Starting MCP server: {}", server_name));
		};

		let result = octomind::mcp::initialize_mcp_for_role_with_callback(
			role,
			config,
			Some(&progress_callback),
		)
		.await;

		// Ensure spinner is fully cleared with ANSI escape codes
		spinner.finish_and_clear();
		// Clear entire line and move cursor to beginning
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();

		result
	} else {
		// Non-interactive mode - no spinner
		octomind::mcp::initialize_mcp_for_role(role, config).await
	}
}

async fn run_with_cleanup(args: CliArgs, config: Config) -> Result<(), anyhow::Error> {
	let log_level = config.log_level.as_str();
	if let Commands::Run(_) = &args.command {
		if let Err(e) = octomind::logging::tracing_setup::init_tracing(
			octomind::logging::tracing_setup::LoggingMode::Cli,
			log_level,
		) {
			eprintln!("Warning: Failed to initialize tracing: {e}");
		}
	}

	let sandbox_enabled = match &args.command {
		Commands::Run(a) => config.sandbox || a.sandbox,
		Commands::Server(a) => config.sandbox || a.sandbox,
		Commands::Acp(a) => config.sandbox || a.sandbox,
		_ => false,
	};
	if sandbox_enabled {
		let cwd = std::env::current_dir()?;
		octomind::sandbox::apply(&cwd)?;
	}

	// MCP init is handled inside commands::run::execute (needs merged config for agents).
	// Server and ACP init their own MCP here.
	match &args.command {
		Commands::Server(server_args) => {
			initialize_mcp_for_role_with_progress(&server_args.role, &config, true).await?;
		}
		Commands::Acp(acp_args) => {
			initialize_mcp_for_role_with_progress(&acp_args.role, &config, false).await?;
		}
		_ => {}
	}

	match args.command {
		Commands::Config(config_args) => commands::config::execute(&config_args, config)?,
		Commands::Run(run_args) => commands::run::execute(&run_args, &config).await?,
		Commands::Server(server_args) => commands::server::execute(&server_args, &config).await?,
		Commands::Acp(acp_args) => commands::acp::execute(&acp_args, &config).await?,
		Commands::Tap(tap_args) => commands::tap::execute(&tap_args)?,
		Commands::Untap(untap_args) => commands::untap::execute(&untap_args)?,
		Commands::Vars(vars_args) => commands::vars::execute(&vars_args, &config).await?,
		Commands::Completion { shell } => {
			let mut app = CliArgs::command();
			let name = app.get_name().to_string();
			generate(shell, &mut app, name, &mut std::io::stdout());
		}
	}

	Ok(())
}
