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

use clap::Args;
use octomind::acp::AcpRunOptions;

#[derive(Args, Debug)]
pub struct AcpArgs {
	/// Agent tag (e.g. `developer` or `developer:rust`): a tap agent, or a role defined in your config, which wins over a tap agent of the same name.
	/// Omit to use the default role from config.
	#[arg(value_name = "TAG")]
	pub tag: Option<String>,

	/// Session name — used as the preferred name when the client creates a session.
	#[arg(long, short = 'n', value_name = "NAME")]
	pub name: Option<String>,

	/// Resume a specific session by name on the next client `new_session` request.
	#[arg(long, short = 'r', value_name = "SESSION")]
	pub resume: Option<String>,

	/// Resume the most recent session for the current working directory on the
	/// next client `new_session` request.
	#[arg(long)]
	pub resume_recent: bool,

	/// Override the model for sessions started by this ACP agent
	/// (e.g. `openrouter:anthropic/claude-sonnet-4`).
	/// Priority: CLI --model > role.model > config.model
	#[arg(long, short = 'm', value_name = "MODEL")]
	pub model: Option<String>,

	/// Restrict all filesystem writes to the current working directory
	#[arg(long)]
	pub sandbox: bool,

	/// Every session this agent creates is read-only: only tools whose
	/// `readOnlyHint` is true are offered or run.
	#[arg(long)]
	pub readonly: bool,

	/// Set by a parent that starts a copy of itself: this session may not start
	/// further copies of itself.
	#[arg(long, hide = true)]
	pub copy: bool,
	/// Load the exact manifest pinned to this tap-run id before initializing any servers.
	#[arg(long, hide = true, requires_all = ["name","tag"])]
	pub bound_tap: bool,
}

/// Execute the acp command — runs Octomind as an ACP agent over stdio
pub async fn execute(args: &AcpArgs, config: &octomind::Config) -> Result<(), anyhow::Error> {
	// stdout is the protocol stream, so a role that cannot start (a missing
	// INPUT/ENV value, an unknown tag) is reported over ACP rather than by
	// exiting: a parent agent would otherwise only see the pipe close.
	let (resolved_config, role, startup_error) = match if args.bound_tap {
		start_bound_tap(args, config).await
	} else {
		super::common::startup(args.tag.as_deref(), config, false).await
	} {
		Ok((resolved_config, role)) => (resolved_config, role, None),
		Err(e) => {
			let tag = args.tag.clone().unwrap_or_else(|| config.default.clone());
			let message = format!("Agent '{tag}' cannot start: {e:#}");
			(config.clone(), tag, Some(message))
		}
	};
	let options = AcpRunOptions {
		name: args.name.clone(),
		resume: args.resume.clone(),
		resume_recent: args.resume_recent,
		model: args.model.clone(),
		startup_error,
		mode: octomind::session::context::SessionMode {
			readonly: args.readonly,
			copy: args.copy,
		},
	};
	octomind::acp::run(resolved_config, role, options).await
}

/// Start a run pinned to the tap manifest written for it by its parent.
async fn start_bound_tap(args: &AcpArgs, config: &octomind::Config) -> anyhow::Result<(octomind::Config, String)> {
	let tag = args.tag.as_deref().expect("clap requires --tag with --bound-tap");
	let id = args.name.as_deref().expect("clap requires --name with --bound-tap");
	let manifest = octomind::agent::manifest::Manifest::read_run(id, tag)?;
	let (mut resolved, role) = octomind::agent::resolver::resolve_manifest(manifest, config, None).await?;
	resolved.bound_tap_run = true;
	octomind::mcp::initialize_mcp_for_role(&role, &resolved).await?;
	Ok((resolved, role))
}
