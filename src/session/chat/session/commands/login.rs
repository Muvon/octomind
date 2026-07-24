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

//! /login — sign in to Octomind from an ACP client (the octoweb panel).
//!
//! Returns the verification URL + code immediately and drives the RFC 8628 poll
//! on a background thread. The client opens the URL in a browser tab and watches
//! `/usage` flip to signed-in; on success this process's `.env`, `auth.json`,
//! and `OCTOHUB_API_KEY` are all updated via [`crate::account::finish_login`].
//! Already-signed-in short-circuits — same as `octomind login`.

use super::{CommandOutput, CommandResult};
use crate::account;
use anyhow::Result;
use std::time::Duration;

pub async fn handle_login() -> Result<CommandResult> {
	if let Some(account) = account::whoami().await.ok().flatten() {
		return Ok(CommandResult::HandledWithOutput(Box::new(
			CommandOutput::Login {
				already_signed_in: true,
				account: Some(format!("{} ({})", account.email, account.plan)),
				verification_url: None,
				user_code: None,
			},
		)));
	}

	let start = account::start_login().await?;
	let verification_url = account::panel_url(&start.verification_url_complete);
	let user_code = start.user_code.clone();
	let device_code = start.device_code;
	let interval = Duration::from_secs(start.interval);

	// Drive the poll off the ACP thread on its own runtime: it is self-contained
	// (network + file writes, no session state), so a plain OS thread avoids
	// depending on the single-threaded LocalSet's lifetime and keeps `/login`
	// returning immediately so the client can open the URL.
	std::thread::spawn(move || {
		let Ok(rt) = tokio::runtime::Builder::new_current_thread()
			.enable_all()
			.build()
		else {
			return;
		};
		rt.block_on(async move {
			match account::poll_login(&device_code, interval).await {
				Ok(claim) => {
					if let Err(e) = account::finish_login(&claim) {
						crate::log_error!("ACP /login: could not store credentials: {}", e);
					} else {
						crate::log_debug!("ACP /login: signed in");
					}
				}
				Err(e) => crate::log_debug!("ACP /login: {}", e),
			}
		});
	});

	Ok(CommandResult::HandledWithOutput(Box::new(
		CommandOutput::Login {
			already_signed_in: false,
			account: None,
			verification_url: Some(verification_url),
			user_code: Some(user_code),
		},
	)))
}
