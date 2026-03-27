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

#[derive(Args, Debug)]
pub struct CompleteArgs {
	/// The subcommand to generate completions for (e.g. `run`).
	#[arg(value_name = "SUBCOMMAND")]
	pub subcommand: String,
}

pub fn execute(args: &CompleteArgs, config: &Config) -> Result<()> {
	match args.subcommand.as_str() {
		"run" => {
			// Agent tags from all taps (cached locally, no network)
			let tags = octomind::agent::taps::list_agent_tags().unwrap_or_default();
			for tag in &tags {
				println!("{tag}");
			}
			// Role names from config
			for role in &config.roles {
				println!("{}", role.name);
			}
		}
		_ => {
			// Unknown subcommand — emit nothing, let the shell fall back to file completion
		}
	}
	Ok(())
}
