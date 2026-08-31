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
use clap::{Args, Subcommand};
use octomind::agent::{tap_scaffold, taps};
use octomind::session::chat::{
	block_close_ok, block_line, block_open, block_row, block_section, key_width,
};

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct TapArgs {
	#[command(subcommand)]
	pub command: Option<TapCommand>,

	/// Tap to add in `user/repo` format. Omit to list all active taps.
	///
	/// Examples:
	///   octomind tap myorg/repo           # clones https://github.com/myorg/octomind-repo
	///   octomind tap myorg/repo ./local   # uses local directory
	///   octomind tap init myorg/repo      # scaffold a new tap in ./octomind-repo
	#[arg(value_name = "TAP")]
	pub tap: Option<String>,

	/// Local directory path for the tap (skips git clone).
	#[arg(value_name = "PATH")]
	pub local_path: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum TapCommand {
	/// Create a new tap from the default tap's scaffold: render it, validate
	/// it, git-init it, and register it as a local tap ready for `octomind run`.
	Init(TapInitArgs),
}

#[derive(Args, Debug)]
pub struct TapInitArgs {
	/// New tap id in `user/repo` format.
	#[arg(value_name = "TAP")]
	pub tap: String,

	/// Starter agent tag as `domain:spec`. Domain defaults to the repo name,
	/// spec to the scaffold's default.
	#[arg(long, value_name = "DOMAIN:SPEC")]
	pub agent: Option<String>,

	/// Destination directory (defaults to ./octomind-<repo>).
	#[arg(long, value_name = "DIR")]
	pub dir: Option<std::path::PathBuf>,
}

pub fn execute(args: &TapArgs) -> Result<()> {
	use colored::Colorize;

	if let Some(TapCommand::Init(init)) = &args.command {
		let outcome =
			tap_scaffold::init_tap(&init.tap, init.agent.as_deref(), init.dir.as_deref())?;
		block_open("tap", Some("init"));
		let kw = key_width(["name", "dir", "agent", "next"]);
		block_row("name", &outcome.tap_id.bright_green().to_string(), kw);
		block_row(
			"dir",
			&outcome
				.dest
				.display()
				.to_string()
				.bright_white()
				.to_string(),
			kw,
		);
		block_row("agent", &outcome.agent_tag.bright_white().to_string(), kw);
		block_row(
			"next",
			&format!("octomind run {}", outcome.agent_tag)
				.bright_cyan()
				.to_string(),
			kw,
		);
		block_close_ok("tap", Some(&outcome.tap_id));
		println!();
		return Ok(());
	}

	match &args.tap {
		Some(tap_arg) => {
			let full_arg = match &args.local_path {
				Some(path) => format!("{} {}", tap_arg, path),
				None => tap_arg.clone(),
			};
			taps::add_tap(&full_arg)?;
			block_open("tap", None);
			let kw = key_width(["name", "local"]);
			block_row("name", &tap_arg.bright_green().to_string(), kw);
			if let Some(ref path) = args.local_path {
				block_row("local", &path.bright_white().to_string(), kw);
			}
			block_close_ok("tap", Some(tap_arg));
			println!();
		}
		None => {
			let user_taps = taps::list_taps()?;
			block_open("tap", Some("active taps"));
			if user_taps.is_empty() {
				block_line(&"No user taps configured.".dimmed().to_string());
			} else {
				block_section("user");
				let name_width = user_taps
					.iter()
					.map(|t| t.name.len())
					.max()
					.unwrap_or(0)
					.min(40);
				for tap in &user_taps {
					let suffix = tap
						.local_path
						.as_ref()
						.map(|p| format!("(local: {})", p).dimmed().to_string())
						.unwrap_or_default();
					block_row(&tap.name, &suffix, name_width);
				}
			}
			block_section("built-in");
			block_row(
				taps::DEFAULT_TAP,
				&"(always active)".dimmed().to_string(),
				taps::DEFAULT_TAP.len(),
			);
			block_close_ok(
				"tap",
				Some(&format!("{} user + 1 built-in", user_taps.len())),
			);
			println!();
		}
	}
	Ok(())
}

#[cfg(test)]
#[path = "tap_tests.rs"]
mod tests;
