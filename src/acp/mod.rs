// Copyright 2025 Muvon Un Limited
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

//! ACP (Agent Client Protocol) server implementation.
//!
//! Runs Octomind as an ACP agent over stdio, compatible with clients
//! like Zed editor and JetBrains IDEs.

mod agent;
pub mod commands;

use agent::OctomindAgent;
use anyhow::Result;
use futures::future::LocalBoxFuture;

use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::config::Config;

/// Run the ACP agent over stdio until the client disconnects.
pub async fn run(config: Config, role: String) -> Result<()> {
	let local = tokio::task::LocalSet::new();

	local
		.run_until(async move {
			let agent = std::rc::Rc::new(OctomindAgent::new(config, role));

			let stdin = tokio::io::stdin().compat();
			let stdout = tokio::io::stdout().compat_write();

			let (conn, io_task) = agent_client_protocol::AgentSideConnection::new(
				std::rc::Rc::clone(&agent),
				stdout,
				stdin,
				|fut: LocalBoxFuture<'static, ()>| {
					tokio::task::spawn_local(fut);
				},
			);

			let conn = std::rc::Rc::new(conn);
			agent.set_connection(std::rc::Rc::clone(&conn));

			// Drive the I/O loop — returns when stdin closes (client disconnected)
			if let Err(e) = io_task.await {
				crate::log_debug!("ACP: I/O loop ended: {}", e);
			}

			Ok(())
		})
		.await
}
