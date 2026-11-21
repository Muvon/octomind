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

//! How octomind talks to one plugin: JSON-RPC 2.0 lines over a child's
//! standard input and output or a unix socket, or one POST per message.
//!
//! One connection per plugin per process. Requests are multiplexed by id: a
//! reader task routes each response to its waiter, so two sessions never block
//! each other.

use super::install;
use super::protocol::{CancelParams, ProgressParams, RpcRequest, RpcResponse, METHOD_CANCEL, METHOD_PROGRESS};
use crate::config::{PluginConfig, PluginKind};
use crate::mcp::client::resolve_env_placeholders;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};

/// First id of a connection. Ids are never reused within one connection.
const FIRST_REQUEST_ID: u64 = 1;
/// A notification has no answer to wait for, so it gets a fixed limit over HTTP.
const HTTP_NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// How long a request may wait for its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitLimit {
	/// The request fails when this long passes without an answer or a `progress` notification for it.
	pub silence: Duration,
	/// The request fails when this long passes since it was written, whatever the plugin sent meanwhile.
	pub total: Duration,
}

/// One connection to one plugin.
#[async_trait]
pub trait Transport: Send + Sync {
	/// Send a request and wait for its answer within `limit`.
	async fn request(&self, method: &str, params: Value, limit: WaitLimit) -> Result<Value>;
	/// Send a notification: no answer is expected.
	async fn notify(&self, method: &str, params: Value) -> Result<()>;
	/// False once the connection is known to be gone. The host then connects again.
	fn is_connected(&self) -> bool;
}

/// Opens connections. Tests replace it with an in-memory one.
#[async_trait]
pub trait Connector: Send + Sync {
	async fn connect(&self, config: &PluginConfig) -> Result<Arc<dyn Transport>>;
}

pub struct ProcessConnector;

#[async_trait]
impl Connector for ProcessConnector {
	async fn connect(&self, config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		Ok(match config.kind {
			PluginKind::Stdio => {
				install_if_missing(config).await?;
				Arc::new(spawn_stdio(config)?)
			}
			PluginKind::Socket => {
				// Only a program that is about to be started needs to be there.
				if !socket_answers(config).await {
					install_if_missing(config).await?;
				}
				Arc::new(connect_socket(config).await?)
			}
			PluginKind::Http => Arc::new(HttpTransport::new(config)?),
		})
	}
}

/// Download the program of an entry that names a signed release, when it is
/// not installed yet. Entries without one start as they always did.
async fn install_if_missing(config: &PluginConfig) -> Result<()> {
	if config.install_manifest.is_some() {
		install::resolve_command(config).await?;
	}
	Ok(())
}

#[cfg(unix)]
async fn socket_answers(config: &PluginConfig) -> bool {
	let Some(path) = config.path.as_deref() else {
		return false;
	};
	tokio::net::UnixStream::connect(path).await.is_ok()
}

#[cfg(not(unix))]
async fn socket_answers(_config: &PluginConfig) -> bool {
	false
}

#[cfg(unix)]
use super::socket::connect_socket;

#[cfg(not(unix))]
async fn connect_socket(config: &PluginConfig) -> Result<LineTransport> {
	bail!("Plugin '{}': socket needs a unix platform, use type stdio", config.name)
}

// ---------------------------------------------------------------------------
// Line transport: stdio and sockets
// ---------------------------------------------------------------------------

/// Where the reader delivers what arrives for one request.
struct Waiter {
	answer: oneshot::Sender<Result<Value>>,
	/// Woken by each `progress` notification for the request.
	progress: Arc<Notify>,
}

type Waiters = Arc<parking_lot::Mutex<HashMap<u64, Waiter>>>;

type SharedWriter = Arc<AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>>;

/// Lives for one request. When the request ends without an answer, because it
/// timed out or its future was dropped (the user pressed Ctrl+C), it takes the
/// waiter out and tells the plugin to stop the work.
struct PendingRequest {
	id: u64,
	waiters: Waiters,
	writer: SharedWriter,
	open: Arc<AtomicBool>,
	answered: bool,
}

impl Drop for PendingRequest {
	fn drop(&mut self) {
		self.waiters.lock().remove(&self.id);
		if self.answered || !self.open.load(Ordering::SeqCst) {
			return;
		}
		// Drop cannot await. Without a runtime (shutdown) there is nobody to tell.
		let Ok(runtime) = tokio::runtime::Handle::try_current() else {
			return;
		};
		let writer = self.writer.clone();
		let id = self.id;
		runtime.spawn(async move {
			let params = serde_json::to_value(CancelParams { id }).expect("cancel params serialize");
			let mut line =
				serde_json::to_string(&RpcRequest::new(None, METHOD_CANCEL, params)).expect("cancel serializes");
			line.push('\n');
			let mut writer = writer.lock().await;
			let _ = writer.write_all(line.as_bytes()).await;
			let _ = writer.flush().await;
		});
	}
}

pub struct LineTransport {
	name: String,
	writer: SharedWriter,
	waiters: Waiters,
	next_id: AtomicU64,
	open: Arc<AtomicBool>,
	reader: tokio::task::JoinHandle<()>,
	/// Held so a child started for this connection dies with it (`kill_on_drop`).
	_child: Option<tokio::process::Child>,
}

impl LineTransport {
	pub fn new<R, W>(name: &str, reader: R, writer: W, child: Option<tokio::process::Child>) -> Self
	where
		R: AsyncRead + Send + Unpin + 'static,
		W: AsyncWrite + Send + Unpin + 'static,
	{
		let waiters: Waiters = Arc::default();
		let open = Arc::new(AtomicBool::new(true));
		let reader = tokio::spawn(read_loop(name.to_string(), reader, waiters.clone(), open.clone()));
		Self {
			name: name.to_string(),
			writer: Arc::new(AsyncMutex::new(Box::new(writer))),
			waiters,
			next_id: AtomicU64::new(FIRST_REQUEST_ID),
			open,
			reader,
			_child: child,
		}
	}

	async fn write(&self, message: &RpcRequest) -> Result<()> {
		if !self.is_connected() {
			bail!("plugin '{}' closed the connection", self.name);
		}
		let mut line = serde_json::to_string(message).context("Failed to encode plugin message")?;
		line.push('\n');
		// A task of its own finishes the line even when the caller stops waiting
		// (timeout, Ctrl+C); a dropped write would leave half a line on the pipe.
		let writer = self.writer.clone();
		tokio::spawn(async move {
			let mut writer = writer.lock().await;
			writer.write_all(line.as_bytes()).await?;
			writer.flush().await
		})
		.await
		.context("plugin write task failed")??;
		Ok(())
	}
}

impl Drop for LineTransport {
	fn drop(&mut self) {
		self.reader.abort();
	}
}

#[async_trait]
impl Transport for LineTransport {
	async fn request(&self, method: &str, params: Value, limit: WaitLimit) -> Result<Value> {
		let id = self.next_id.fetch_add(1, Ordering::Relaxed);
		let (answer, mut receiver) = oneshot::channel();
		let progress = Arc::new(Notify::new());
		self.waiters.lock().insert(
			id,
			Waiter {
				answer,
				progress: progress.clone(),
			},
		);
		let mut pending = PendingRequest {
			id,
			waiters: self.waiters.clone(),
			writer: self.writer.clone(),
			open: self.open.clone(),
			answered: false,
		};
		// The reader clears `open` before it drains the waiters, so a waiter added
		// after the drain is caught here and one added before it is drained.
		if !self.is_connected() {
			bail!("plugin '{}' closed the connection", self.name);
		}
		let message = RpcRequest::new(Some(id), method, params);
		let write = self.write(&message);
		tokio::pin!(write);
		let mut written = false;
		let last_moment = tokio::time::Instant::now() + limit.total;
		// The write is part of the wait: a plugin that does not read its input must not hold the caller.
		// Each loop turn starts a new silence clock, so a `progress` notification restarts it.
		loop {
			tokio::select! {
				biased;
				answer = &mut receiver => {
					pending.answered = true;
					return answer.map_err(|_| anyhow!("plugin '{}' dropped the request", self.name))?;
				}
				done = &mut write, if !written => {
					done?;
					written = true;
				}
				_ = progress.notified() => {}
				_ = tokio::time::sleep_until(last_moment) => {
					bail!("no answer within {}s", limit.total.as_secs());
				}
				_ = tokio::time::sleep(limit.silence) => {
					bail!("no answer within {}s", limit.silence.as_secs());
				}
			}
		}
	}

	async fn notify(&self, method: &str, params: Value) -> Result<()> {
		self.write(&RpcRequest::new(None, method, params)).await
	}

	fn is_connected(&self) -> bool {
		self.open.load(Ordering::SeqCst)
	}
}

async fn read_loop<R: AsyncRead + Unpin>(name: String, reader: R, waiters: Waiters, open: Arc<AtomicBool>) {
	let mut lines = BufReader::new(reader).lines();
	loop {
		match lines.next_line().await {
			Ok(Some(line)) => route_line(&name, &line, &waiters),
			Ok(None) => break,
			Err(error) => {
				crate::log_debug!("Plugin '{}' read failed: {}", name, error);
				break;
			}
		}
	}
	open.store(false, Ordering::SeqCst);
	let pending: Vec<_> = waiters.lock().drain().collect();
	for (_, waiter) in pending {
		let _ = waiter
			.answer
			.send(Err(anyhow!("plugin '{}' closed the connection", name)));
	}
}

fn route_line(name: &str, line: &str, waiters: &Waiters) {
	if line.trim().is_empty() {
		return;
	}
	// A response has no `method`, so only a message of the plugin's own parses as a request.
	if let Ok(notification) = serde_json::from_str::<RpcRequest>(line) {
		route_notification(name, &notification, waiters);
		return;
	}
	let response: RpcResponse = match serde_json::from_str(line) {
		Ok(response) => response,
		Err(error) => {
			// A plugin may print logs on its standard output; they are not messages.
			crate::log_debug!("Plugin '{}' sent a line that is not a response: {}", name, error);
			return;
		}
	};
	let Some(waiter) = waiters.lock().remove(&response.id) else {
		crate::log_debug!("Plugin '{}' answered unknown request {}", name, response.id);
		return;
	};
	let _ = waiter.answer.send(response_value(name, response));
}

/// The one message a plugin sends on its own is `progress`. It never answers a
/// request; it only tells the waiting side that the plugin is still working.
fn route_notification(name: &str, notification: &RpcRequest, waiters: &Waiters) {
	if notification.method != METHOD_PROGRESS || notification.id.is_some() {
		crate::log_debug!(
			"Plugin '{}' sent '{}', which octomind does not take",
			name,
			notification.method
		);
		return;
	}
	let params: ProgressParams = match serde_json::from_value(notification.params.clone()) {
		Ok(params) => params,
		Err(error) => {
			crate::log_debug!("Plugin '{}' sent progress without a request id: {}", name, error);
			return;
		}
	};
	match waiters.lock().get(&params.id) {
		Some(waiter) => waiter.progress.notify_one(),
		None => crate::log_debug!("Plugin '{}' reported progress on unknown request {}", name, params.id),
	}
}

fn response_value(name: &str, response: RpcResponse) -> Result<Value> {
	match response.error {
		Some(error) => bail!("plugin '{}' returned error {}: {}", name, error.code, error.message),
		None => Ok(response.result.unwrap_or(Value::Null)),
	}
}

// ---------------------------------------------------------------------------
// Child processes
// ---------------------------------------------------------------------------

/// `{{ENV:KEY}}` keys in `texts` that are unset or empty.
fn missing_env_keys<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
	let mut keys: Vec<String> = texts
		.into_iter()
		.flat_map(crate::agent::inputs::extract_env_keys)
		.filter(|key| std::env::var(key).map(|value| value.is_empty()).unwrap_or(true))
		.collect();
	keys.sort();
	keys.dedup();
	keys
}

fn ensure_env_set<'a>(config: &PluginConfig, texts: impl IntoIterator<Item = &'a str>) -> Result<()> {
	let missing = missing_env_keys(texts);
	if !missing.is_empty() {
		bail!(
			"Plugin '{}' requires env vars: {}. Set them before starting the plugin",
			config.name,
			missing.join(", ")
		);
	}
	Ok(())
}

/// The program and arguments of a plugin with `{{ENV:KEY}}` resolved.
pub(super) fn resolved_command(config: &PluginConfig) -> Result<tokio::process::Command> {
	let command = config
		.command
		.as_deref()
		.with_context(|| format!("Plugin '{}' has no command", config.name))?;
	ensure_env_set(
		config,
		std::iter::once(command).chain(config.args.iter().map(String::as_str)),
	)?;
	// A program downloaded by `install` lives in the plugin directory, which is not on PATH.
	let command = resolve_env_placeholders(command);
	let program = install::find_installed(&command, &config.name)?.unwrap_or_else(|| command.into());
	let mut process = tokio::process::Command::new(program);
	process.args(config.args.iter().map(|arg| resolve_env_placeholders(arg)));
	// A process group of its own, so Ctrl+C at the terminal does not reach the plugin.
	#[cfg(unix)]
	process.process_group(0);
	#[cfg(windows)]
	process.creation_flags(CREATE_NEW_PROCESS_GROUP);
	Ok(process)
}

fn spawn_stdio(config: &PluginConfig) -> Result<LineTransport> {
	let mut process = resolved_command(config)?;
	process
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::piped())
		.stderr(std::process::Stdio::piped())
		.kill_on_drop(true);
	let mut child = process
		.spawn()
		.with_context(|| format!("Failed to start plugin '{}'", config.name))?;
	let stdin = child.stdin.take().context("plugin stdin was not piped")?;
	let stdout = child.stdout.take().context("plugin stdout was not piped")?;
	let stderr = child.stderr.take().context("plugin stderr was not piped")?;

	#[cfg(unix)]
	if let Some(pid) = child.id() {
		// The MCP shutdown sweep kills every registered process group.
		crate::mcp::process::register_pgid(&format!("plugin:{}", config.name), pid);
	}

	let name = config.name.clone();
	tokio::spawn(async move {
		let mut lines = BufReader::new(stderr).lines();
		while let Ok(Some(line)) = lines.next_line().await {
			crate::log_debug!("Plugin '{}' stderr: {}", name, line.trim());
		}
	});
	Ok(LineTransport::new(&config.name, stdout, stdin, Some(child)))
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

pub struct HttpTransport {
	name: String,
	url: String,
	client: reqwest::Client,
	next_id: AtomicU64,
}

impl HttpTransport {
	pub fn new(config: &PluginConfig) -> Result<Self> {
		let url = config
			.url
			.clone()
			.with_context(|| format!("Plugin '{}' has no url", config.name))?;
		ensure_env_set(config, config.headers.values().map(String::as_str))?;
		let mut headers = reqwest::header::HeaderMap::new();
		for (header, value) in &config.headers {
			let header_name = reqwest::header::HeaderName::from_bytes(header.as_bytes()).map_err(|error| {
				anyhow!(
					"Invalid header name '{}' for plugin '{}': {}",
					header,
					config.name,
					error
				)
			})?;
			let header_value =
				reqwest::header::HeaderValue::from_str(&resolve_env_placeholders(value)).map_err(|error| {
					anyhow!(
						"Invalid value for header '{}' of plugin '{}': {}",
						header,
						config.name,
						error
					)
				})?;
			headers.insert(header_name, header_value);
		}
		let client = reqwest::Client::builder()
			.default_headers(headers)
			.build()
			.context("Failed to build the HTTP client for a plugin")?;
		Ok(Self {
			name: config.name.clone(),
			url,
			client,
			next_id: AtomicU64::new(FIRST_REQUEST_ID),
		})
	}

	async fn post(&self, message: &RpcRequest, timeout: Duration) -> reqwest::Result<reqwest::Response> {
		self.client.post(&self.url).timeout(timeout).json(message).send().await
	}

	fn describe(&self, error: reqwest::Error) -> anyhow::Error {
		anyhow!("plugin '{}': {}", self.name, error)
	}
}

/// Over HTTP there is no connection to tell the plugin on: when a request ends
/// without an answer (timeout, or its future dropped), one best-effort POST of
/// `cancel` goes out.
struct HttpCancel {
	id: u64,
	client: reqwest::Client,
	url: String,
	answered: bool,
}

impl Drop for HttpCancel {
	fn drop(&mut self) {
		if self.answered {
			return;
		}
		let Ok(runtime) = tokio::runtime::Handle::try_current() else {
			return;
		};
		let (client, url, id) = (self.client.clone(), self.url.clone(), self.id);
		runtime.spawn(async move {
			let params = serde_json::to_value(CancelParams { id }).expect("cancel params serialize");
			let message = RpcRequest::new(None, METHOD_CANCEL, params);
			let _ = client
				.post(&url)
				.timeout(HTTP_NOTIFY_TIMEOUT)
				.json(&message)
				.send()
				.await;
		});
	}
}

#[async_trait]
impl Transport for HttpTransport {
	// One POST has no channel for `progress` notifications: the wait is `limit.silence`
	// and `limit.total` does not apply.
	async fn request(&self, method: &str, params: Value, limit: WaitLimit) -> Result<Value> {
		let id = self.next_id.fetch_add(1, Ordering::Relaxed);
		let mut cancel = HttpCancel {
			id,
			client: self.client.clone(),
			url: self.url.clone(),
			answered: false,
		};
		let sent = self
			.post(&RpcRequest::new(Some(id), method, params), limit.silence)
			.await;
		// An error status is an answer; only a timeout leaves the plugin working.
		cancel.answered = !matches!(&sent, Err(error) if error.is_timeout());
		let response = sent
			.and_then(reqwest::Response::error_for_status)
			.map_err(|error| self.describe(error))?;
		let response: RpcResponse = response
			.json()
			.await
			.map_err(|error| anyhow!("plugin '{}' sent an unreadable answer: {}", self.name, error))?;
		if response.id != id {
			bail!(
				"plugin '{}' answered request {} to request {}",
				self.name,
				response.id,
				id
			);
		}
		response_value(&self.name, response)
	}

	async fn notify(&self, method: &str, params: Value) -> Result<()> {
		self.post(&RpcRequest::new(None, method, params), HTTP_NOTIFY_TIMEOUT)
			.await
			.and_then(reqwest::Response::error_for_status)
			.map(|_| ())
			.map_err(|error| self.describe(error))
	}

	fn is_connected(&self) -> bool {
		true
	}
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
