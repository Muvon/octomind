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

//! Unix socket connection to a plugin that may outlive octomind.

use super::transport::{resolved_command, LineTransport};
use crate::config::PluginConfig;
use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};
use tokio::net::UnixStream;

/// How long a freshly started plugin gets to open its socket.
const SOCKET_START_WAIT: Duration = Duration::from_secs(5);
const SOCKET_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Connect to the plugin's socket. When nothing listens and `command` is set,
/// start the program detached, so it can serve other octomind processes too,
/// and retry for a bounded time.
pub async fn connect_socket(config: &PluginConfig) -> Result<LineTransport> {
	let path = config
		.path
		.as_deref()
		.with_context(|| format!("Plugin '{}' has no socket path", config.name))?;
	let first_error = match UnixStream::connect(path).await {
		Ok(stream) => return Ok(wrap(config, stream)),
		Err(error) => error,
	};
	if config.command.is_none() {
		bail!("Plugin '{}': nothing answers on {}: {}", config.name, path, first_error);
	}
	crate::log_debug!("Plugin '{}': nothing answers on {}, starting it", config.name, path);
	start_detached(config)?;

	let deadline = Instant::now() + SOCKET_START_WAIT;
	loop {
		tokio::time::sleep(SOCKET_RETRY_INTERVAL).await;
		match UnixStream::connect(path).await {
			Ok(stream) => return Ok(wrap(config, stream)),
			Err(error) if Instant::now() >= deadline => {
				bail!(
					"Plugin '{}' did not open {} within {}s: {}",
					config.name,
					path,
					SOCKET_START_WAIT.as_secs(),
					error
				);
			}
			Err(_) => {}
		}
	}
}

fn wrap(config: &PluginConfig, stream: UnixStream) -> LineTransport {
	let (reader, writer) = stream.into_split();
	LineTransport::new(&config.name, reader, writer, None)
}

fn start_detached(config: &PluginConfig) -> Result<()> {
	let mut process = resolved_command(config)?;
	process
		.stdin(std::process::Stdio::null())
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.kill_on_drop(false);
	process
		.spawn()
		.with_context(|| format!("Failed to start plugin '{}'", config.name))?;
	Ok(())
}
