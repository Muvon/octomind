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

use anyhow::{Context, Result};
use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};
use octomind::agent::{inputs, registry};
use octomind::config::{loading::merge_agent_toml, Config};
use octomind::session;
use std::io::{self, IsTerminal, Read};
use std::time::Duration;

#[derive(Args, Debug)]
pub struct RunArgs {
	/// Agent tag (e.g. `developer:rust`) or role name (e.g. `developer`).
	/// Omit to use the default role from config.
	#[arg(value_name = "TAG")]
	pub tag: Option<String>,

	/// Session name — creates a named session or resumes it if it already exists.
	#[arg(long, short = 'n', value_name = "NAME")]
	pub name: Option<String>,

	/// Resume a specific session by name.
	#[arg(long, short = 'r', value_name = "SESSION")]
	pub resume: Option<String>,

	/// Resume the most recent session for the current working directory.
	#[arg(long)]
	pub resume_recent: bool,

	/// Output format: plain or jsonl. When set, runs non-interactively
	/// (reads input from stdin).
	#[arg(long = "format")]
	pub format: Option<String>,

	/// Override the model for this session (e.g. `openrouter:anthropic/claude-sonnet-4`).
	/// Priority: CLI --model > role.model > config.model
	#[arg(long, short = 'm', value_name = "MODEL")]
	pub model: Option<String>,

	/// Restrict all filesystem writes to the current working directory
	#[arg(long)]
	pub sandbox: bool,
}
pub async fn execute(args: &RunArgs, config: &Config) -> Result<()> {
	let is_interactive = args.format.is_none() && std::io::stdin().is_terminal();

	// Resolve config + role: registry agent vs plain role name
	let (run_config, role) = resolve_config_and_role(args, config).await?;

	// Initialize MCP servers (spinner only in interactive mode)
	init_mcp(&role, &run_config, is_interactive).await?;

	let session_args = octomind::session::chat::session::GenericSessionArgs {
		role: role.clone(),
		mode: args.format.clone().unwrap_or_else(|| "plain".to_string()),
		name: args.name.clone(),
		resume: args.resume.clone(),
		resume_recent: args.resume_recent,
		model: args.model.clone(),
		..Default::default()
	};

	if is_interactive {
		session::chat::run_interactive_session(&session_args, &run_config).await
	} else {
		let input = read_input()?;
		session::chat::run_interactive_session_with_input(&session_args, &run_config, &input).await
	}
}

/// Returns (merged_config, role_name). If tag contains `:` it's a registry agent;
/// otherwise it's treated as a plain role name (default: "developer").
async fn resolve_config_and_role(args: &RunArgs, config: &Config) -> Result<(Config, String)> {
	let tag = args.tag.as_deref().unwrap_or("developer");

	if tag.contains(':') {
		// Registry agent: fetch manifest, resolve inputs, merge config
		let raw_toml = registry::fetch_manifest(tag, &config.registry)
			.await
			.context(format!("Failed to fetch agent manifest for '{tag}'"))?;
		let resolved_toml = inputs::resolve_inputs(&raw_toml).await?;
		let merged = merge_agent_toml(config, &resolved_toml)
			.context("Failed to merge agent manifest into config")?;

		// First role in merged config that isn't in the base config
		let base_names: std::collections::HashSet<&str> =
			config.roles.iter().map(|r| r.name.as_str()).collect();
		let role = merged
			.roles
			.iter()
			.find(|r| !base_names.contains(r.name.as_str()))
			.map(|r| r.name.clone())
			.context(format!(
				"Agent manifest for '{tag}' must define at least one new [[roles]] entry"
			))?;
		Ok((merged, role))
	} else {
		// Plain role name — use config as-is
		Ok((config.clone(), tag.to_string()))
	}
}

/// Read input from stdin (piped or interactive prompt is not our job here).
fn read_input() -> Result<String> {
	if !std::io::stdin().is_terminal() {
		let mut buf = String::new();
		io::stdin().read_to_string(&mut buf)?;
		let input = buf.trim().to_string();
		if input.is_empty() {
			anyhow::bail!("No input provided via stdin");
		}
		Ok(input)
	} else {
		anyhow::bail!("--format requires input via stdin or piped data")
	}
}

async fn init_mcp(role: &str, config: &Config, is_interactive: bool) -> Result<()> {
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
		let cb = |name: &str| spinner.set_message(format!("Starting MCP server: {name}"));
		let result =
			octomind::mcp::initialize_mcp_for_role_with_callback(role, config, Some(&cb)).await;
		spinner.finish_and_clear();
		print!("\x1B[2K\r");
		std::io::Write::flush(&mut std::io::stdout()).ok();
		result
	} else {
		octomind::mcp::initialize_mcp_for_role(role, config).await
	}
}
