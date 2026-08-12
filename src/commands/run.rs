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

use anyhow::Result;
use clap::Args;
use octomind::config::Config;
use octomind::session;
use std::io::{self, IsTerminal, Read};

#[derive(Args, Debug)]
pub struct RunArgs {
	/// Agent tag (e.g. `developer:general`) or role name (e.g. `developer`).
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

	/// Keep the session alive indefinitely, waiting for messages injected via `octomind inject`.
	/// Implies non-interactive mode (requires --format).
	#[arg(long)]
	pub daemon: bool,

	/// Restrict all filesystem writes to the current working directory
	#[arg(long)]
	pub sandbox: bool,

	/// Activate a webhook hook by name (defined in [[hooks]] config).
	/// Can be specified multiple times for multiple hooks.
	#[arg(long = "hook", value_name = "NAME")]
	pub hooks: Vec<String>,

	/// Enforce structured output: path to a JSON Schema file. The model's final
	/// response is constrained to match it. Fails if the resolved model has no
	/// structured-output support.
	#[arg(long = "schema", value_name = "PATH")]
	pub schema: Option<String>,
}
pub async fn execute(args: &RunArgs, config: &Config) -> Result<()> {
	// Daemon mode: no spinner, but still use readline if terminal input available.
	// --format=jsonl always uses non-interactive mode (piped input).
	let is_interactive_session = args.format.is_none() && std::io::stdin().is_terminal();

	// Read piped stdin immediately — before any async init that may spawn subprocesses
	// which inherit (and can drain) the pipe fd on some platforms (macOS).
	let piped_input = if !is_interactive_session {
		if args.daemon && std::io::stdin().is_terminal() {
			Some(String::new())
		} else {
			Some(read_input()?)
		}
	} else {
		None
	};

	// Load + validate the structured-output schema file (if any) before init.
	// Model-capability support is checked once the session resolves a model.
	let schema = match &args.schema {
		Some(path) => Some(session::load_structured_output_schema(path)?),
		None => None,
	};

	// Resuming without a tag comes back as the session's own role, not the config
	// default — otherwise continuing a conversation silently swaps the agent, its
	// model and its tools. An explicit tag always wins: that IS the switch.
	let tag = match &args.tag {
		Some(tag) => Some(tag.clone()),
		None => resumed_session_name(args).and_then(|name| octomind::session::resume_role(&name)),
	};

	// Resolve config and role (tap/dep resolution only — MCP init happens after session ID is set)
	let (run_config, role) =
		octomind::agent::resolver::resolve_config_and_role(tag.as_deref(), config, None).await?;

	let session_args = octomind::session::chat::session::GenericSessionArgs {
		role: role.clone(),
		role_explicit: args.tag.is_some(),
		mode: args.format.clone().unwrap_or_else(|| "plain".to_string()),
		name: args.name.clone(),
		resume: args.resume.clone(),
		resume_recent: args.resume_recent,
		model: args.model.clone(),
		daemon: args.daemon,
		hooks: args.hooks.clone(),
		schema,
		..Default::default()
	};

	let result = if is_interactive_session {
		session::chat::run_interactive_session(&session_args, &run_config).await
	} else {
		session::chat::run_interactive_session_with_input(
			&session_args,
			&run_config,
			&piped_input.unwrap(),
		)
		.await
	};
	// What the supervisor did and spent this run (gate runs, steers, blocks,
	// condense savings, its own token/time bill) — the counters exist only in
	// this process, so surface them before exit.
	if let Some(stats) = octomind::supervisor::stats::snapshot() {
		octomind::log_debug!("supervisor session stats: {}", stats);
	}
	result
}

/// Which existing session this invocation will resume, if any.
///
/// `--name` is included because it resumes when the session already exists; a
/// name with no file behind it is a brand-new session and must not recover a
/// role from anywhere.
fn resumed_session_name(args: &RunArgs) -> Option<String> {
	if let Some(session) = &args.resume {
		return Some(session.clone());
	}
	if args.resume_recent {
		let current_dir = octomind::mcp::get_thread_working_directory();
		return octomind::session::find_most_recent_session_for_project(&current_dir)
			.ok()
			.flatten();
	}
	args.name.clone()
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
