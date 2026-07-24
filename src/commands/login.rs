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

//! `octomind login` — sign in to your Octomind account from the terminal.
//!
//! Device-authorization flow (the shape of `gh auth login`, RFC 8628): the CLI
//! asks the API to start a login, shows a short code, and the user confirms that
//! code in the browser where they are already signed in. Nothing here handles a
//! password.
//!
//! The device flow itself lives in [`crate::account`] so this command and the
//! ACP `/login` command (driven by the octoweb panel) mint credentials the same
//! way; this file is just the terminal presentation around it.

use anyhow::Result;
use clap::Args;
use colored::Colorize;
use std::time::Duration;

use octomind::account;
use octomind::session::chat::{block_close_ok, block_line, block_open, block_row, key_width};

#[derive(Args, Debug)]
pub struct LoginArgs {
	/// Sign in again even if this machine already has a session.
	#[arg(long)]
	pub force: bool,

	/// Print the URL instead of opening a browser.
	#[arg(long)]
	pub no_browser: bool,
}

pub async fn execute(args: &LoginArgs) -> Result<()> {
	// Already signed in is worth saying out loud rather than silently minting a
	// second set of credentials and killing the ones that were working.
	if !args.force {
		if let Some(account) = account::whoami().await? {
			block_open("login", Some("octomind account"));
			let kw = key_width(["account", "plan"]);
			block_row("account", &account.email.bright_green().to_string(), kw);
			block_row("plan", &account.plan, kw);
			block_close_ok("login", Some("already signed in"));
			println!();
			println!("Use `octomind login --force` to sign in as someone else.");
			return Ok(());
		}
	}

	let start = account::start_login().await?;
	let confirm_url = account::panel_url(&start.verification_url_complete);

	block_open("login", Some("octomind account"));
	let kw = key_width(["code", "url"]);
	block_row(
		"code",
		&start.user_code.bright_yellow().bold().to_string(),
		kw,
	);
	block_row(
		"url",
		&account::panel_url(&start.verification_url)
			.bright_cyan()
			.to_string(),
		kw,
	);
	block_line("");
	block_line("Confirm the code in your browser to finish signing in.");

	if args.no_browser {
		block_line(&format!("Open: {confirm_url}"));
	} else if open::that(&confirm_url).is_err() {
		// Headless box, no xdg-open, SSH session — the URL is still actionable.
		block_line(&format!("Could not open a browser. Open: {confirm_url}"));
	}
	block_line("waiting…");

	let claim =
		account::poll_login(&start.device_code, Duration::from_secs(start.interval)).await?;
	let env_path = account::finish_login(&claim)?;

	let who = account::whoami().await.ok().flatten();
	let kw = key_width(["account", "key", "stored"]);
	if let Some(a) = &who {
		block_row("account", &a.email.bright_green().to_string(), kw);
	}
	// The server names the key; echo ITS answer so the row always matches what the
	// Keys page shows, including for older servers that ignore the device id.
	block_row("key", &claim.key_name, kw);
	block_row("stored", &env_path.display().to_string(), kw);
	block_close_ok("login", Some("signed in"));
	println!();
	Ok(())
}
