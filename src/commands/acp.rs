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
	/// Agent tag (e.g. `developer:general`) or role name (e.g. `developer`).
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

	/// Activate a webhook hook by name (defined in [[hooks]] config).
	/// Can be specified multiple times for multiple hooks.
	#[arg(long = "hook", value_name = "NAME")]
	pub hooks: Vec<String>,
}

/// Execute the acp command — runs Octomind as an ACP agent over stdio
pub async fn execute(args: &AcpArgs, config: &octomind::Config) -> Result<(), anyhow::Error> {
	let (resolved_config, role) =
		super::common::startup(args.tag.as_deref(), config, false).await?;
	let options = AcpRunOptions {
		name: args.name.clone(),
		resume: args.resume.clone(),
		resume_recent: args.resume_recent,
		model: args.model.clone(),
		hooks: args.hooks.clone(),
	};
	octomind::acp::run(resolved_config, role, options).await
}
