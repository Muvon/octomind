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
use colored::Colorize;
use octomind::agent::taps;
use octomind::session::chat::{block_close_ok, block_open, block_row, key_width};

#[derive(Args, Debug)]
pub struct UntapArgs {
	/// Tap name to remove in `user/repo` format.
	#[arg(value_name = "TAP")]
	pub name: String,
}

pub fn execute(args: &UntapArgs) -> Result<()> {
	taps::remove_tap(&args.name)?;
	block_open("untap", None);
	let kw = key_width(["removed"]);
	block_row("removed", &args.name.bright_yellow().to_string(), kw);
	block_close_ok("untap", Some(&args.name));
	println!();
	Ok(())
}
