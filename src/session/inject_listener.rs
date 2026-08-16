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

//! Unix Domain Socket listener for external message injection.
//!
//! Each running session binds a UDS at `<run_dir>/<stem>.sock` and writes its
//! PID to `<run_dir>/<name>.pid`, where the run dir is `$XDG_RUNTIME_DIR/octomind`
//! or `<system tmp>/octomind-<uid>` (see `directories::get_run_dir`). The socket
//! stem shortens long session names to fit the sun_path limit (see
//! `directories::session_socket_path`).
//!
//! The `octomind send` command connects to this socket, sends a UTF-8 message,
//! shuts down the write half, and reads back `"ok\n"` or `"error: ...\n"`.

// Unix domain sockets are not available on Windows — the entire implementation
// is gated to cfg(unix). A no-op stub is provided for other platforms.

#[cfg(unix)]
mod imp {
	use std::path::PathBuf;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::UnixListener;
	use tokio::task::AbortHandle;

	use crate::session::inbox::{push_inbox_message_for_session, InboxMessage, InboxSource};
	use crate::{log_debug, log_error};

	fn socket_path(session_name: &str) -> anyhow::Result<PathBuf> {
		crate::directories::session_socket_path(session_name)
	}

	fn pid_path(session_name: &str) -> anyhow::Result<PathBuf> {
		Ok(crate::directories::get_run_dir()?.join(format!("{}.pid", session_name)))
	}

	/// RAII guard that stops the inject listener and cleans up socket/PID files on drop.
	pub struct InjectListenerGuard {
		session_name: String,
		_abort: AbortHandle,
	}

	impl Drop for InjectListenerGuard {
		fn drop(&mut self) {
			self._abort.abort();
			if let Ok(sock) = socket_path(&self.session_name) {
				let _ = std::fs::remove_file(&sock);
			}
			if let Ok(pid) = pid_path(&self.session_name) {
				let _ = std::fs::remove_file(&pid);
			}
			log_debug!(
				"Inject listener cleaned up for session '{}'",
				self.session_name
			);
		}
	}

	pub fn start_inject_listener(session_name: &str) -> InjectListenerGuard {
		let session_name = session_name.to_string();

		let handle = tokio::spawn({
			let name = session_name.clone();
			async move {
				if let Err(e) = run_listener(&name).await {
					log_error!("Inject listener error for session '{}': {}", name, e);
				}
			}
		});

		InjectListenerGuard {
			session_name,
			_abort: handle.abort_handle(),
		}
	}

	async fn run_listener(session_name: &str) -> anyhow::Result<()> {
		let sock = socket_path(session_name)?;

		// Remove stale socket file if it exists (e.g. from a previous crash).
		if sock.exists() {
			std::fs::remove_file(&sock)?;
		}

		let listener = UnixListener::bind(&sock)?;
		log_debug!("Inject listener bound to {:?}", sock);

		// Write PID file so `octomind inject` can verify the process is alive.
		let pid = std::process::id();
		let pid_file = pid_path(session_name)?;
		std::fs::write(&pid_file, pid.to_string())?;
		log_debug!("Inject listener PID {} written to {:?}", pid, pid_file);

		loop {
			match listener.accept().await {
				Ok((mut stream, _)) => {
					log_debug!(
						"Inject listener: connection accepted for session '{}'",
						session_name
					);

					// Read the full message (client shuts down write half after sending).
					let mut buf = Vec::new();
					match stream.read_to_end(&mut buf).await {
						Ok(0) => {
							// Empty message — ignore silently.
							let _ = stream.write_all(b"error: empty message\n").await;
						}
						Ok(_) => {
							let content = String::from_utf8_lossy(&buf).trim().to_string();
							if content.is_empty() {
								let _ = stream.write_all(b"error: empty message\n").await;
								continue;
							}

							log_debug!(
								"Inject listener: received {} bytes for session '{}'",
								content.len(),
								session_name
							);

							push_inbox_message_for_session(
								session_name,
								InboxMessage {
									source: InboxSource::Inject,
									content,
								},
							);

							let _ = stream.write_all(b"ok\n").await;
						}
						Err(e) => {
							log_error!("Inject listener: read error: {}", e);
							let _ = stream
								.write_all(format!("error: read failed: {}\n", e).as_bytes())
								.await;
						}
					}
				}
				Err(e) => {
					// Listener itself failed — log and stop.
					log_error!("Inject listener: accept error: {}", e);
					break;
				}
			}
		}

		Ok(())
	}
}

#[cfg(windows)]
mod imp {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	use tokio::net::windows::named_pipe::ServerOptions;
	use tokio::task::AbortHandle;

	use crate::session::inbox::{push_inbox_message_for_session, InboxMessage, InboxSource};
	use crate::{log_debug, log_error};

	/// Named pipe path for a session: `\\.\pipe\octomind-<name>`
	fn pipe_name(session_name: &str) -> String {
		format!(r"\\.\pipe\octomind-{}", session_name)
	}

	/// RAII guard that stops the inject listener task on drop.
	pub struct InjectListenerGuard {
		session_name: String,
		_abort: AbortHandle,
	}

	impl Drop for InjectListenerGuard {
		fn drop(&mut self) {
			self._abort.abort();
			log_debug!(
				"Inject listener cleaned up for session '{}'",
				self.session_name
			);
		}
	}

	pub fn start_inject_listener(session_name: &str) -> InjectListenerGuard {
		let session_name = session_name.to_string();

		let handle = tokio::spawn({
			let name = session_name.clone();
			async move {
				if let Err(e) = run_listener(&name).await {
					log_error!("Inject listener error for session '{}': {}", name, e);
				}
			}
		});

		InjectListenerGuard {
			session_name,
			_abort: handle.abort_handle(),
		}
	}

	async fn run_listener(session_name: &str) -> anyhow::Result<()> {
		let name = pipe_name(session_name);
		log_debug!("Inject listener binding named pipe {:?}", name);

		// Create the first server instance. `first_pipe_instance` ensures no
		// stale pipe from a previous crash can conflict.
		let mut server = ServerOptions::new()
			.first_pipe_instance(true)
			.create(&name)?;

		loop {
			// Wait for a client to connect.
			server.connect().await?;
			log_debug!(
				"Inject listener: connection accepted for session '{}'",
				session_name
			);

			// Read the full message (client shuts down write half after sending).
			let mut buf = Vec::new();
			match server.read_to_end(&mut buf).await {
				Ok(0) => {
					let _ = server.write_all(b"error: empty message\n").await;
				}
				Ok(_) => {
					let content = String::from_utf8_lossy(&buf).trim().to_string();
					if content.is_empty() {
						let _ = server.write_all(b"error: empty message\n").await;
					} else {
						log_debug!(
							"Inject listener: received {} bytes for session '{}'",
							content.len(),
							session_name
						);

						push_inbox_message_for_session(
							session_name,
							InboxMessage {
								source: InboxSource::Inject,
								content,
							},
						);

						let _ = server.write_all(b"ok\n").await;
					}
				}
				Err(e) => {
					log_error!("Inject listener: read error: {}", e);
					let _ = server
						.write_all(format!("error: read failed: {}\n", e).as_bytes())
						.await;
				}
			}

			// Disconnect this client and reuse the server instance for the next one.
			server.disconnect()?;
		}
	}
}

pub use imp::{start_inject_listener, InjectListenerGuard};
