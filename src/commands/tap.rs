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
	match &args.tap {
		Some(tap_arg) => {
			let full_arg = match &args.local_path {
				Some(path) => format!("{} {}", tap_arg, path),
				None => tap_arg.clone(),
			};
			taps::add_tap(&full_arg)?;
			if let Some(ref path) = args.local_path {
				println!("✓ Tapped: {} (local: {})", tap_arg, path);
			} else {
				println!("✓ Tapped: {}", tap_arg);
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
