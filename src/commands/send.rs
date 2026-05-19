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

//! `octomind send` — send a message to a running session.
//!
//! On Unix this uses a Unix Domain Socket (`<run_dir>/<name>.sock`).
//! On Windows this uses a Named Pipe (`\\.\pipe\octomind-<name>`).

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use octomind::session::chat::{block_close_ok, block_open, block_row, key_width};
use std::io::{self, Read};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Args, Debug)]
pub struct SendArgs {
	/// Name of the running session to send to.
	#[arg(long, short = 'n', value_name = "NAME")]
	pub name: String,

	/// Message to send. If omitted, reads from stdin.
	#[arg(value_name = "MESSAGE")]
	pub message: Option<String>,
}

pub async fn execute(args: &SendArgs) -> Result<()> {
	let message = match &args.message {
		Some(m) => m.trim().to_string(),
		None => {
			// If stdin is a terminal, there's nothing to read — bail early
			// instead of blocking forever waiting for EOF.
			if io::IsTerminal::is_terminal(&io::stdin()) {
				bail!("message must not be empty (pass as argument or pipe via stdin)");
			}
			let mut buf = String::new();
			io::stdin()
				.read_to_string(&mut buf)
				.context("failed to read message from stdin")?;
			buf.trim().to_string()
		}
	};

	if message.is_empty() {
		bail!("message must not be empty (pass as argument or pipe via stdin)");
	}

	send_message(&args.name, &message).await?;
	block_open("send", None);
	let kw = key_width(["session", "chars"]);
	block_row("session", &args.name.bright_green().to_string(), kw);
	block_row(
		"chars",
		&message
			.chars()
			.count()
			.to_string()
			.bright_white()
			.to_string(),
		kw,
	);
	block_close_ok("send", Some(&args.name));
	println!();
	Ok(())
}

#[cfg(unix)]
async fn send_message(session_name: &str, message: &str) -> Result<()> {
	use tokio::net::UnixStream;

	let sock_path = octomind::directories::get_run_dir()
		.context("failed to resolve run directory")?
		.join(format!("{}.sock", session_name));

	if !sock_path.exists() {
		bail!(
			"no running session named '{}' (socket not found at {:?})",
			session_name,
			sock_path
		);
	}

	let mut stream = UnixStream::connect(&sock_path)
		.await
		.with_context(|| format!("failed to connect to session '{}'", session_name))?;

	stream
		.write_all(message.as_bytes())
		.await
		.context("failed to send message")?;
	stream
		.shutdown()
		.await
		.context("failed to shut down write half")?;

	read_response(&mut stream, session_name).await
}

#[cfg(windows)]
async fn send_message(session_name: &str, message: &str) -> Result<()> {
	use std::time::Duration;
	use tokio::net::windows::named_pipe::ClientOptions;

	// ERROR_PIPE_BUSY (231) — server exists but isn't waiting for a connection yet.
	const ERROR_PIPE_BUSY: i32 = 231;
	// ERROR_FILE_NOT_FOUND (2) — pipe doesn't exist at all (no session running).
	const ERROR_FILE_NOT_FOUND: i32 = 2;

	let pipe_name = format!(r"\\.\pipe\octomind-{}", session_name);

	let mut client = loop {
		match ClientOptions::new().open(&pipe_name) {
			Ok(c) => break c,
			Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
				tokio::time::sleep(Duration::from_millis(50)).await;
			}
			Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
				bail!(
					"no running session named '{}' (named pipe not found: {})",
					session_name,
					pipe_name
				);
			}
			Err(e) => {
				return Err(e)
					.with_context(|| format!("failed to connect to session '{}'", session_name));
			}
		}
	};

	client
		.write_all(message.as_bytes())
		.await
		.context("failed to send message")?;
	client
		.shutdown()
		.await
		.context("failed to shut down write half")?;

	read_response(&mut client, session_name).await
}

async fn read_response<S>(stream: &mut S, session_name: &str) -> Result<()>
where
	S: AsyncReadExt + Unpin,
{
	let mut response = String::new();
	stream
		.read_to_string(&mut response)
		.await
		.context("failed to read response")?;

	let response = response.trim();
	if response == "ok" {
		Ok(())
	} else {
		bail!("session '{}' returned: {}", session_name, response);
	}
}
