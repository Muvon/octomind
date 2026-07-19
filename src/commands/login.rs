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
//! It collects a hub key (written to the user-scope `.env` as `OCTOHUB_API_KEY`,
//! which every octomind process loads at startup) and a panel session (stored by
//! [`crate::account`], refreshed automatically from then on).

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use serde::Deserialize;
use std::time::Duration;

use octomind::account;
use octomind::directories::get_config_dir;
use octomind::session::chat::{block_close_ok, block_line, block_open, block_row, key_width};

/// Stop polling well after the server's own TTL rather than hanging forever.
const MAX_POLL: Duration = Duration::from_secs(15 * 60);

#[derive(Args, Debug)]
pub struct LoginArgs {
	/// Sign in again even if this machine already has a session.
	#[arg(long)]
	pub force: bool,

	/// Print the URL instead of opening a browser.
	#[arg(long)]
	pub no_browser: bool,
}

#[derive(Deserialize)]
struct Start {
	device_code: String,
	user_code: String,
	verification_url: String,
	verification_url_complete: String,
	interval: u64,
}

#[derive(Deserialize)]
struct Claim {
	api_key: String,
	jwt: String,
	refresh_token: String,
}

/// Swap the origin of a server-supplied verification URL for the local panel.
fn repoint(url: &str, panel: &str) -> String {
	match url.find("/app") {
		Some(i) => format!("{}{}", panel.trim_end_matches('/'), &url[i..]),
		None => url.to_string(),
	}
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

	let start: Start = account::post_public(
		"/auth/cli",
		serde_json::json!({
			"client": format!("octomind/{} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS),
		}),
	)
	.await?
	.map_err(|e| anyhow::anyhow!("could not start login: {e}"))?;

	let panel = std::env::var(account::PANEL_URL_ENV).ok();
	let repoint_for = |url: &str| match &panel {
		Some(p) => repoint(url, p),
		None => url.to_string(),
	};
	let confirm_url = repoint_for(&start.verification_url_complete);

	block_open("login", Some("octomind account"));
	let kw = key_width(["code", "url"]);
	block_row(
		"code",
		&start.user_code.bright_yellow().bold().to_string(),
		kw,
	);
	block_row(
		"url",
		&repoint_for(&start.verification_url)
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

	let interval = Duration::from_secs(start.interval.clamp(1, 30));
	let deadline = std::time::Instant::now() + MAX_POLL;
	let claim = loop {
		if std::time::Instant::now() >= deadline {
			bail!("login timed out — run `octomind login` again");
		}
		tokio::time::sleep(interval).await;
		match account::post_public::<Claim>(
			"/auth/cli/token",
			serde_json::json!({ "device_code": start.device_code }),
		)
		.await?
		{
			Ok(c) => break c,
			Err(e) if e == "pending" => continue,
			Err(e) if e == "expired" => bail!("that code expired — run `octomind login` again"),
			Err(e) => bail!("login failed: {e}"),
		}
	};

	let env_path = write_hub_key(&claim.api_key)?;
	account::save_session(&account::Session {
		jwt: claim.jwt,
		refresh_token: claim.refresh_token,
		api_url: account::api_url(),
	})?;
	// Same process, so anything later in this run picks it up without a restart.
	std::env::set_var(account::HUB_KEY_ENV, &claim.api_key);

	let who = account::whoami().await.ok().flatten();
	let kw = key_width(["account", "key", "stored"]);
	if let Some(a) = &who {
		block_row("account", &a.email.bright_green().to_string(), kw);
	}
	block_row("key", "octomind-cli", kw);
	block_row("stored", &env_path.display().to_string(), kw);
	block_close_ok("login", Some("signed in"));
	println!();
	Ok(())
}

/// Upsert `OCTOHUB_API_KEY` in the user-scope `.env` — the one every octomind
/// process loads at startup. Other variables in that file are left alone; a new
/// login replaces the previous key rather than appending a second line.
fn write_hub_key(key: &str) -> Result<std::path::PathBuf> {
	let path = get_config_dir()?.join(".env");
	let existing = std::fs::read_to_string(&path).unwrap_or_default();
	std::fs::write(&path, upsert(&existing, key))
		.with_context(|| format!("could not write {}", path.display()))?;

	// It holds a live credential: keep it owner-only.
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
	}
	Ok(path)
}

/// Replace any existing `OCTOHUB_API_KEY` line, keep every other line as-is.
fn upsert(existing: &str, key: &str) -> String {
	let prefix = format!("{}=", account::HUB_KEY_ENV);
	let mut out: Vec<&str> = existing
		.lines()
		.filter(|l| {
			!l.trim_start()
				.trim_start_matches("export ")
				.starts_with(&prefix)
		})
		.collect();
	let line = format!("{prefix}{key}");
	out.push(&line);
	out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn upsert_replaces_the_key_and_keeps_the_rest() {
		let before = "OPENROUTER_API_KEY=abc\nOCTOHUB_API_KEY=old\nFOO=bar\n";
		let after = upsert(before, "new");
		assert_eq!(
			after,
			"OPENROUTER_API_KEY=abc\nFOO=bar\nOCTOHUB_API_KEY=new\n"
		);
		// Empty file, exported form, and repeated logins all converge on one line.
		assert_eq!(upsert("", "k"), "OCTOHUB_API_KEY=k\n");
		assert_eq!(
			upsert("export OCTOHUB_API_KEY=old", "k"),
			"OCTOHUB_API_KEY=k\n"
		);
		assert_eq!(
			upsert(&after, "third"),
			"OPENROUTER_API_KEY=abc\nFOO=bar\nOCTOHUB_API_KEY=third\n"
		);
	}

	#[test]
	fn repoint_swaps_origin_keeps_path_and_query() {
		assert_eq!(
			repoint(
				"https://octomind.run/app/login/cli?code=AB12-CD34",
				"http://localhost:5199"
			),
			"http://localhost:5199/app/login/cli?code=AB12-CD34"
		);
	}

	#[test]
	fn repoint_leaves_unrecognized_urls_alone() {
		assert_eq!(
			repoint("https://example.com/x", "http://localhost:1"),
			"https://example.com/x"
		);
	}
}
