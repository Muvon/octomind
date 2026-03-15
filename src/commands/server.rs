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

use clap::Args;

#[derive(Args, Debug)]
pub struct ServerArgs {
	/// Agent tag (e.g. `developer:rust`) or role name (e.g. `developer`).
	/// Omit to use the default role from config.
	#[arg(value_name = "TAG")]
	pub tag: Option<String>,

	/// Host address to bind to
	#[arg(long, default_value = "127.0.0.1")]
	pub host: String,

	/// Port to listen on
	#[arg(long, short, default_value = "8080")]
	pub port: u16,

	/// Restrict all filesystem writes to the current working directory
	#[arg(long)]
	pub sandbox: bool,
}

/// Execute the server command
pub async fn execute(args: &ServerArgs, config: &octomind::Config) -> Result<(), anyhow::Error> {
	use octomind::websocket::WebSocketServer;

	let (resolved_config, role) =
		super::common::resolve_config_and_role(args.tag.as_deref(), config, None).await?;

	// Initialize tracing for WebSocket mode - logs to file
	// stdout/stderr are used for server status messages
	let log_level = resolved_config.log_level.as_str();
	if let Err(e) = octomind::logging::tracing_setup::init_tracing(
		octomind::logging::tracing_setup::LoggingMode::WebSocket,
		log_level,
	) {
		eprintln!("Warning: Failed to initialize tracing: {}", e);
	}

	super::common::startup_mcp_only(&role, &resolved_config, true).await?;

	// Create and start WebSocket server
	let server = WebSocketServer::new(&args.host, args.port, resolved_config, role)?;
	server.start().await?;

	Ok(())
}
