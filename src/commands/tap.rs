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
use octomind::agent::taps;
use octomind::session::chat::{
	block_close_ok, block_line, block_open, block_row, block_section, key_width,
};

#[derive(Args, Debug)]
pub struct TapArgs {
	/// Tap to add in `user/repo` format. Omit to list all active taps.
	///
	/// Examples:
	///   octomind tap myorg/repo           # clones https://github.com/myorg/octomind-repo
	///   octomind tap myorg/repo ./local   # uses local directory
	#[arg(value_name = "TAP")]
	pub tap: Option<String>,

	/// Local directory path for the tap (skips git clone).
	#[arg(value_name = "PATH")]
	pub local_path: Option<String>,
}

pub fn execute(args: &TapArgs) -> Result<()> {
	use colored::Colorize;

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
