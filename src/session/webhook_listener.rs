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

//! HTTP webhook listener for external hook-to-inbox injection.
//!
//! Each activated hook binds an HTTP server on its configured address.
//! Incoming POST requests are piped through the hook's script:
//! body → stdin, stdout → session inbox (if exit code 0).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::AbortHandle;

use crate::config::HookConfig;
use crate::session::inbox::{push_inbox_message_for_session, InboxMessage, InboxSource};
use crate::{log_debug, log_error, log_info};

/// RAII guard that stops the webhook listener on drop.
pub struct WebhookListenerGuard {
	hook_name: String,
	_abort: AbortHandle,
}

impl Drop for WebhookListenerGuard {
	fn drop(&mut self) {
		self._abort.abort();
		log_debug!("Webhook listener '{}' cleaned up", self.hook_name);
	}
}

/// Validate hook configuration before starting.
/// Returns the parsed `SocketAddr` and validated script path.
pub fn validate_hook(hook: &HookConfig) -> anyhow::Result<(SocketAddr, PathBuf)> {
	let addr: SocketAddr = hook.bind.parse().map_err(|e| {
		anyhow::anyhow!(
			"Hook '{}': invalid bind address '{}': {}",
			hook.name,
			hook.bind,
			e
		)
	})?;

	let script_path = PathBuf::from(&hook.script);
	if !script_path.exists() {
		anyhow::bail!(
			"Hook '{}': script '{}' does not exist",
			hook.name,
			hook.script
		);
	}
	if !script_path.is_file() {
		anyhow::bail!(
			"Hook '{}': script '{}' is not a file",
			hook.name,
			hook.script
		);
	}

	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		let metadata = std::fs::metadata(&script_path)?;
		if metadata.permissions().mode() & 0o111 == 0 {
			anyhow::bail!(
				"Hook '{}': script '{}' is not executable",
				hook.name,
				hook.script
			);
		}
	}

	Ok((addr, script_path))
}

/// Start a webhook listener for a hook configuration.
/// Returns an RAII guard that stops the listener on drop.
pub async fn start_webhook_listener(
	session_name: &str,
	hook: &HookConfig,
	addr: SocketAddr,
	script_path: PathBuf,
) -> anyhow::Result<WebhookListenerGuard> {
	let hook_name = hook.name.clone();
	let timeout_secs = hook.timeout;
	let session = session_name.to_string();

	// Bind early so errors surface before spawning the background task.
	let listener = TcpListener::bind(addr)
		.await
		.map_err(|e| anyhow::anyhow!("Hook '{}': failed to bind on {}: {}", hook_name, addr, e))?;
	log_info!("Webhook '{}' listening on {}", hook_name, addr);

	let name_for_task = hook_name.clone();
	let handle = tokio::spawn(async move {
		if let Err(e) = run_listener(
			listener,
			&session,
			&name_for_task,
			&script_path,
			timeout_secs,
		)
		.await
		{
			log_error!("Webhook listener '{}' error: {}", name_for_task, e);
		}
	});

	Ok(WebhookListenerGuard {
		hook_name,
		_abort: handle.abort_handle(),
	})
}

async fn run_listener(
	listener: TcpListener,
	session_name: &str,
	hook_name: &str,
	script_path: &std::path::Path,
	timeout_secs: u64,
) -> anyhow::Result<()> {
	loop {
		let (stream, remote_addr) = listener.accept().await?;
		let io = TokioIo::new(stream);

		let session = session_name.to_string();
		let hook = hook_name.to_string();
		let script = script_path.to_path_buf();

		tokio::spawn(async move {
			let hook_for_err = hook.clone();
			let svc = service_fn(move |req: Request<Incoming>| {
				let session = session.clone();
				let hook = hook.clone();
				let script = script.clone();
				async move { handle_request(req, &session, &hook, &script, timeout_secs).await }
			});

			if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
				log_debug!(
					"Webhook '{}' connection error from {}: {}",
					hook_for_err,
					remote_addr,
					e
				);
			}
		});
	}
}

async fn handle_request(
	req: Request<Incoming>,
	session_name: &str,
	hook_name: &str,
	script_path: &std::path::Path,
	timeout_secs: u64,
) -> Result<Response<Full<Bytes>>, Infallible> {
	if req.method() != hyper::Method::POST {
		return Ok(response(
			StatusCode::METHOD_NOT_ALLOWED,
			"Only POST allowed\n",
		));
	}

	// Collect request metadata for script environment.
	let method = req.method().to_string();
	let path = req.uri().path().to_string();
	let query = req.uri().query().unwrap_or("").to_string();
	let content_type = req
		.headers()
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_string();

	let mut header_envs: Vec<(String, String)> = Vec::new();
	for (name, value) in req.headers() {
		if let Ok(val) = value.to_str() {
			let env_name = format!(
				"HOOK_HEADER_{}",
				name.as_str().to_uppercase().replace('-', "_")
			);
			header_envs.push((env_name, val.to_string()));
		}
	}

	// Read body.
	let body_bytes = match req.collect().await {
		Ok(collected) => collected.to_bytes(),
		Err(e) => {
			log_error!("Webhook '{}': failed to read body: {}", hook_name, e);
			return Ok(response(
				StatusCode::BAD_REQUEST,
				&format!("Failed to read body: {}\n", e),
			));
		}
	};

	// Spawn script process.
	let mut cmd = Command::new(script_path);
	cmd.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.env("HOOK_NAME", hook_name)
		.env("HOOK_METHOD", &method)
		.env("HOOK_PATH", &path)
		.env("HOOK_QUERY", &query)
		.env("HOOK_CONTENT_TYPE", &content_type)
		.env("HOOK_SESSION", session_name);

	for (key, val) in &header_envs {
		cmd.env(key, val);
	}

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			log_error!("Webhook '{}': failed to spawn script: {}", hook_name, e);
			return Ok(response(
				StatusCode::INTERNAL_SERVER_ERROR,
				&format!("Script spawn error: {}\n", e),
			));
		}
	};

	// Write body to stdin, then close it.
	if let Some(mut stdin) = child.stdin.take() {
		let _ = stdin.write_all(&body_bytes).await;
		drop(stdin);
	}

	// Wait for script with timeout.
	let result =
		tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

	match result {
		Ok(Ok(output)) => {
			if output.status.success() {
				let content = String::from_utf8_lossy(&output.stdout).trim().to_string();
				if content.is_empty() {
					log_debug!(
						"Webhook '{}': script returned empty output, skipping",
						hook_name
					);
					return Ok(response(StatusCode::NO_CONTENT, ""));
				}

				log_debug!(
					"Webhook '{}': injecting {} bytes into session '{}'",
					hook_name,
					content.len(),
					session_name
				);

				push_inbox_message_for_session(
					session_name,
					InboxMessage {
						source: InboxSource::Webhook {
							hook: hook_name.to_string(),
						},
						content,
					},
				);

				Ok(response(StatusCode::OK, "ok\n"))
			} else {
				let stderr = String::from_utf8_lossy(&output.stderr);
				let code = output.status.code().unwrap_or(-1);
				log_error!(
					"Webhook '{}': script exited with code {}: {}",
					hook_name,
					code,
					stderr.trim()
				);
				Ok(response(
					StatusCode::INTERNAL_SERVER_ERROR,
					&format!("Script error (exit {})\n", code),
				))
			}
		}
		Ok(Err(e)) => {
			log_error!("Webhook '{}': script IO error: {}", hook_name, e);
			Ok(response(
				StatusCode::INTERNAL_SERVER_ERROR,
				&format!("Script IO error: {}\n", e),
			))
		}
		Err(_) => {
			log_error!(
				"Webhook '{}': script timed out after {}s",
				hook_name,
				timeout_secs
			);
			Ok(response(StatusCode::GATEWAY_TIMEOUT, "Script timeout\n"))
		}
	}
}

fn response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
	Response::builder()
		.status(status)
		.body(Full::new(Bytes::from(body.to_string())))
		.unwrap()
}
