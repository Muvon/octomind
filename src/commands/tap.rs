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

#[derive(Args, Debug)]
pub struct TapArgs {
	/// Tap to add in `user/repo` format, or `user/repo /path/to/local` for local tap.
	/// Omit to list all active taps.
	///
	/// Examples:
	///   octomind tap myorg/repo          # clones https://github.com/myorg/octomind-repo
	///   octomind tap myorg/repo ./local # uses local directory
	#[arg(value_name = "TAP")]
	pub tap: Option<String>,
}

pub fn execute(args: &TapArgs) -> Result<()> {
	match &args.tap {
		Some(tap_arg) => {
			taps::add_tap(tap_arg)?;
			// Parse to show nice output
			let parts: Vec<&str> = tap_arg.split_whitespace().collect();
			if parts.len() > 1 {
				println!("✓ Tapped: {} (local: {})", parts[0], parts[1]);
			} else {
				println!("✓ Tapped: {}", parts[0]);
			}
		}
		None => {
			let user_taps = taps::list_taps()?;
			if user_taps.is_empty() {
				println!("No user taps configured.");
			} else {
				println!("User taps:");
				for tap in &user_taps {
					if let Some(ref path) = tap.local_path {
						println!("  {} (local: {})", tap.name, path);
					} else {
						println!("  {}", tap.name);
					}
				}
			}
			println!("\nBuilt-in: {} (always active)", taps::DEFAULT_TAP);
		}
	}
	Ok(())
}
