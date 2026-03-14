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

use anyhow::{Context, Result};
use clap::Args;

use octomind::agent::{inputs, registry};
use octomind::config::{loading::merge_agent_toml, Config};
use octomind::session;

#[derive(Args, Debug)]
pub struct AgentArgs {
	/// Agent tag to run, e.g. `developer:rust` or `developer:rust@1.2`
	#[arg()]
	pub tag: String,

	/// Session name (optional)
	#[arg(long, short)]
	pub name: Option<String>,

	/// Restrict all filesystem writes to the current working directory
	#[arg(long)]
	pub sandbox: bool,
}

pub async fn execute(args: &AgentArgs, config: &Config) -> Result<()> {
	// 1. Fetch the manifest TOML from registry (or cache)
	let raw_toml = registry::fetch_manifest(&args.tag, &config.registry)
		.await
		.context(format!("Failed to fetch agent manifest for '{}'", args.tag))?;

	// 2. Resolve {{INPUT:KEY}} placeholders — prompts user for missing credentials
	let resolved_toml = inputs::resolve_inputs(&raw_toml).await?;

	// 3. Merge agent manifest into base config (additive for servers + roles)
	let merged_config = merge_agent_toml(config, &resolved_toml)
		.context("Failed to merge agent manifest into config")?;

	// 4. Extract the role name the manifest declares (first role not in base config)
	let base_role_names: std::collections::HashSet<&str> =
		config.roles.iter().map(|r| r.name.as_str()).collect();
	let agent_role = merged_config
		.roles
		.iter()
		.find(|r| !base_role_names.contains(r.name.as_str()))
		.map(|r| r.name.clone())
		.context(format!(
			"Agent manifest for '{}' must define at least one new [[roles]] entry",
			args.tag
		))?;

	// 5. Initialize MCP servers for the agent role (with spinner)
	crate::initialize_mcp_for_role_with_progress(&agent_role, &merged_config, true).await?;

	// 6. Run the interactive session with the merged config and agent role
	let session_args = super::session::SessionArgs {
		name: args.name.clone(),
		resume: None,
		resume_recent: false,
		model: None,
		max_tokens: None,
		temperature: None,
		role: agent_role,
		max_retries: None,
		mode: "plain".to_string(),
		system: None,
		instructions: None,
		schema: None,
		sandbox: args.sandbox || config.sandbox,
	};

	session::chat::run_interactive_session(&session_args, &merged_config).await
}
