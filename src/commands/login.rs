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
//!
//! `octomind login chatgpt` instead runs Sign in with ChatGPT, so `chatgpt:<model>`
//! bills the user's ChatGPT plan; that OAuth flow and its token storage live in
//! octolib.

use anyhow::Result;
use clap::Args;
use colored::Colorize;
use std::time::Duration;

use octolib::llm::providers::chatgpt;
use octomind::account;
use octomind::session::chat::{block_close_ok, block_line, block_open, block_row, key_width};

/// App name suggested on the ChatGPT consent page.
const CHATGPT_APP_NAME: &str = "Octomind";

#[derive(Args, Debug)]
pub struct LoginArgs {
	/// What to sign in to.
	#[arg(value_enum, default_value_t = LoginTarget::Octomind)]
	pub target: LoginTarget,

	/// Sign in again even if this machine already has a session.
	#[arg(long)]
	pub force: bool,

	/// Print the URL instead of opening a browser.
	#[arg(long)]
	pub no_browser: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginTarget {
	/// Your Octomind account.
	Octomind,
	/// Your ChatGPT subscription, used through `chatgpt:<model>`.
	Chatgpt,
}

pub async fn execute(args: &LoginArgs) -> Result<()> {
	match args.target {
		LoginTarget::Octomind => login_octomind(args).await,
		LoginTarget::Chatgpt => login_chatgpt(args).await,
	}
}

async fn login_octomind(args: &LoginArgs) -> Result<()> {
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

async fn login_chatgpt(args: &LoginArgs) -> Result<()> {
	if !args.force {
		if let Some(account) = chatgpt::current_account()? {
			block_open("login", Some("chatgpt"));
			let kw = key_width(["account"]);
			block_row(
				"account",
				&email_label(&account).bright_green().to_string(),
				kw,
			);
			block_close_ok("login", Some("already signed in"));
			println!();
			println!("Use `octomind login chatgpt --force` to sign in again.");
			return Ok(());
		}
	}

	let pending = chatgpt::start_login(CHATGPT_APP_NAME).await?;
	let authorize_url = pending.authorize_url();

	block_open("login", Some("chatgpt"));
	block_line("Approve access in your browser to use your ChatGPT plan.");
	if args.no_browser {
		block_line(&format!("Open: {authorize_url}"));
	} else if open::that(authorize_url).is_err() {
		// The redirect lands on this machine's loopback, so the URL must be
		// opened in a browser running here.
		block_line(&format!("Could not open a browser. Open: {authorize_url}"));
	}
	block_line("waiting…");

	let account = pending.finish().await?;
	let models = chatgpt::list_models().await?;

	let kw = key_width(["account", "models"]);
	block_row(
		"account",
		&email_label(&account).bright_green().to_string(),
		kw,
	);
	let slugs: Vec<&str> = models.iter().map(|model| model.slug.as_str()).collect();
	block_row("models", &slugs.join(", "), kw);
	block_close_ok("login", Some("signed in"));
	println!();
	println!("Use it with model = \"chatgpt:<model>\".");
	Ok(())
}

fn email_label(account: &chatgpt::Account) -> &str {
	account
		.email
		.as_deref()
		.unwrap_or("(no email on the account)")
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
