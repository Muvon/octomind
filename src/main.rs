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

use clap::{Parser, Subcommand, CommandFactory};
use clap_complete::{generate, Shell};

use octomind::config::Config;
use octomind::session;

mod commands;

#[derive(Parser)]
#[command(name = "octomind")]
#[command(version = "0.1.0")]
#[command(about = "Octomind is a smart AI developer assistant with configurable MCP support")]
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
	let args = CliArgs::parse();

	// Load configuration
	let config = Config::load()?;

	// Setup cleanup for MCP server processes when the program exits
	let result = run_with_cleanup(args, config).await;

	// Make sure to clean up any started server processes
	if let Err(e) = octomind::mcp::server::cleanup_servers() {
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
		Commands::Vars(vars_args) => {
			commands::vars::execute(vars_args, &config).await?
		},
		Commands::Completion { shell } => {
			let mut app = CliArgs::command();
			let name = app.get_name().to_string();
			generate(*shell, &mut app, name, &mut std::io::stdout());
		},
	}

	Ok(())
}