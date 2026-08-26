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

//! rmcp-based MCP client registry.
//!
//! Speaks MCP 2026-07-28 (stateless, per-request `_meta`) and falls back to
//! the legacy `initialize` handshake for older servers: on stdio via
//! `ClientLifecycleMode::Auto` (server/discover probe), on Streamable HTTP
//! via an explicit legacy retry as recommended by the spec's backward
//! compatibility section.

use crate::config::{McpConnectionType, McpServerConfig};
use crate::mcp::{oauth, McpToolCall};
use anyhow::{anyhow, Result};
use rmcp::model::{
	CallToolRequest, CallToolRequestParams, CallToolResponse, CancelTaskParams,
	CancelledNotificationParam, ClientCapabilities, ClientInfo, ClientRequest, ElicitRequestParams,
	ElicitResult, ElicitationAction, ElicitationCapability, ExtensionCapabilities,
	FormElicitationCapability, GetTaskParams, Implementation, InputRequest, InputResponses,
	ProgressToken, ProtocolVersion, ServerResult, TaskPayload, UpdateTaskParams,
	UrlElicitationCapability, DEFAULT_MRTR_MAX_ROUNDS, TASKS_EXTENSION_ID,
};
use rmcp::service::{
	ClientLifecycleMode, ClientServiceExt, NotificationContext, PeerRequestOptions, RequestContext,
	RunningService, ServiceError,
};
use rmcp::{ClientHandler, RoleClient};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub type McpService = RunningService<RoleClient, OctoClientHandler>;

lazy_static::lazy_static! {
	/// Active MCP client connections, keyed by server name.
	static ref CLIENTS: RwLock<HashMap<String, Arc<McpService>>> = RwLock::new(HashMap::new());

	/// OAuth bearer token baked into each HTTP connection's transport at
	/// connect time. Compared against the token store before reusing a
	/// connection so refreshed/rotated tokens trigger a reconnect (the
	/// transport cannot change its Authorization header in place).
	static ref HTTP_AUTH_TOKENS: RwLock<HashMap<String, Option<String>>> = RwLock::new(HashMap::new());

	/// Tool call id behind each in-flight progress token. MCP progress carries
	/// only the token, but clients render progress against the tool call it
	/// belongs to, so the mapping is recorded for the life of the request.
	static ref PROGRESS_TOOL_IDS: RwLock<HashMap<ProgressToken, String>> = RwLock::new(HashMap::new());
}

/// Register `token -> tool_id` and unregister on drop, covering the timeout,
/// cancellation and `?` exits of a tool-call round.
struct ProgressTokenBinding(ProgressToken);

impl ProgressTokenBinding {
	fn new(token: &ProgressToken, tool_id: &str) -> Self {
		PROGRESS_TOOL_IDS
			.write()
			.unwrap()
			.insert(token.clone(), tool_id.to_string());
		Self(token.clone())
	}
}

impl Drop for ProgressTokenBinding {
	fn drop(&mut self) {
		PROGRESS_TOOL_IDS.write().unwrap().remove(&self.0);
	}
}

/// Client handler: identifies octomind (with the session context as an
/// experimental capability) and forwards server notifications to the UI.
#[derive(Clone)]
pub struct OctoClientHandler {
	server_name: String,
	session_id: Option<String>,
	/// Advertise the legacy protocol version in the initialize handshake
	/// (set for the explicit legacy retry on Streamable HTTP).
	legacy: bool,
}

impl OctoClientHandler {
	fn new(server_name: &str) -> Self {
		let (_, _, _, session_id, _) = super::process::get_session_context();
		Self {
			server_name: server_name.to_string(),
			session_id: (!session_id.is_empty()).then_some(session_id),
			legacy: false,
		}
	}

	fn emit(&self, method: &str, params: &serde_json::Value) {
		super::process::emit_notification(
			&self.server_name,
			method,
			params,
			self.session_id.as_deref(),
			None,
		);
	}

	fn emit_progress(&self, params: &serde_json::Value, tool_id: Option<String>) {
		super::process::emit_notification(
			&self.server_name,
			"notifications/progress",
			params,
			self.session_id.as_deref(),
			tool_id,
		);
	}
}

impl ClientHandler for OctoClientHandler {
	fn get_info(&self) -> ClientInfo {
		build_client_info(if self.legacy {
			ProtocolVersion::V_2025_03_26
		} else {
			ProtocolVersion::V_2026_07_28
		})
	}

	async fn on_progress(
		&self,
		params: rmcp::model::ProgressNotificationParam,
		_context: NotificationContext<RoleClient>,
	) {
		let tool_id = PROGRESS_TOOL_IDS
			.read()
			.unwrap()
			.get(&params.progress_token)
			.cloned();
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit_progress(&value, tool_id);
	}

	// Logging is deprecated in MCP 2026-07-28 (SEP-2577) but legacy servers
	// still emit it — keep forwarding until the ecosystem catches up.
	#[allow(deprecated)]
	async fn on_logging_message(
		&self,
		params: rmcp::model::LoggingMessageNotificationParam,
		_context: NotificationContext<RoleClient>,
	) {
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/message", &value);
	}

	async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
		crate::mcp::server::clear_function_cache_for_server(&self.server_name);
		self.emit("notifications/tools/list_changed", &serde_json::Value::Null);
	}

	async fn on_cancelled(
		&self,
		params: CancelledNotificationParam,
		_context: NotificationContext<RoleClient>,
	) {
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/cancelled", &value);
	}

	async fn on_resource_updated(
		&self,
		params: rmcp::model::ResourceUpdatedNotificationParam,
		context: NotificationContext<RoleClient>,
	) {
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/resources/updated", &value);

		// A resource we are following just updated (octofs fires this when a
		// detached shell job exits). Recognition is by membership in the watched
		// set — a resource link a tool handed back — not by any URI scheme, so
		// this stays generic across MCP servers. Read the resource and inject its
		// contents so the run loop wakes the model with the result: event-driven,
		// no polling. The read + push runs in a detached task so it neither
		// blocks this notification handler nor re-enters the receive loop.
		let uri = params.uri;
		let Some(session_id) = self.session_id.clone() else {
			return;
		};
		if !crate::session::shell_jobs::is_watched_for_session(&session_id, &uri) {
			return;
		}
		// Clear it now so the loop can never wait forever, even if the read
		// below fails.
		crate::session::shell_jobs::complete_for_session(&session_id, &uri);
		let peer = context.peer.clone();
		tokio::spawn(async move {
			let body = match peer
				.read_resource(rmcp::model::ReadResourceRequestParams::new(uri.clone()))
				.await
			{
				Ok(result) => result
					.contents
					.into_iter()
					.filter_map(|content| match content {
						rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
							Some(text)
						}
						_ => None,
					})
					.collect::<Vec<_>>()
					.join("\n"),
				Err(error) => {
					format!("resource {uri} updated, but reading it failed: {error}")
				}
			};
			let content = format!("<background_job resource=\"{uri}\">\n{body}\n</background_job>");
			crate::session::inbox::push_inbox_message_for_session(
				&session_id,
				crate::session::inbox::InboxMessage {
					source: crate::session::inbox::InboxSource::BackgroundJob { id: uri },
					content,
				},
			);
		});
	}

	async fn on_resource_list_changed(&self, _context: NotificationContext<RoleClient>) {
		self.emit(
			"notifications/resources/list_changed",
			&serde_json::Value::Null,
		);
	}

	async fn on_prompt_list_changed(&self, _context: NotificationContext<RoleClient>) {
		self.emit(
			"notifications/prompts/list_changed",
			&serde_json::Value::Null,
		);
	}

	async fn on_subscriptions_acknowledged(
		&self,
		params: rmcp::model::SubscriptionsAcknowledgedNotificationParams,
		_context: NotificationContext<RoleClient>,
	) {
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/subscriptions/acknowledged", &value);
	}

	async fn on_task_status(
		&self,
		params: rmcp::model::TaskStatusNotificationParams,
		_context: NotificationContext<RoleClient>,
	) {
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/tasks/status", &value);
	}

	/// Elicitation needs an application UI capable of collecting and validating
	/// arbitrary schema-bound input. Octomind currently has no such response
	/// channel in CLI, ACP, or WebSocket mode, so make the protocol decision
	/// explicit and deterministic instead of relying on rmcp's silent default.
	async fn create_elicitation(
		&self,
		request: ElicitRequestParams,
		_context: RequestContext<RoleClient>,
	) -> std::result::Result<ElicitResult, rmcp::ErrorData> {
		let value = serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);
		self.emit("elicitation/requested", &value);
		Ok(ElicitResult::new(ElicitationAction::Decline))
	}

	/// Forward vendor/unknown notifications under their real method name —
	/// the pre-rmcp implementation forwarded every JSON-RPC notification.
	async fn on_custom_notification(
		&self,
		notification: rmcp::model::CustomNotification,
		_context: NotificationContext<RoleClient>,
	) {
		let params = notification.params.unwrap_or(serde_json::Value::Null);
		self.emit(&notification.method, &params);
	}
}

/// Client identity + capabilities sent on every request (modern) or during
/// the initialize handshake (legacy). The octomind session context rides in
/// the experimental capabilities, same as before.
fn build_client_info(protocol_version: ProtocolVersion) -> ClientInfo {
	let (role, spec, project, session_id, workdir) = super::process::get_session_context();
	let git = octolib::utils::is_git_repo(std::path::Path::new(&workdir));
	let session = serde_json::json!({
		"role": role,
		"spec": spec,
		"project": project,
		"session_id": session_id,
		"workdir": workdir,
		"git": git,
	});

	let mut capabilities = ClientCapabilities::default();
	if let serde_json::Value::Object(map) = session {
		capabilities.experimental = Some([("session".to_string(), map)].into());
	}

	// Advertise only capabilities that this handler actually services. Roots
	// and Sampling are deprecated by SEP-2577 and are intentionally absent.
	if protocol_version >= ProtocolVersion::V_2026_07_28 {
		let mut extensions = ExtensionCapabilities::new();
		extensions.insert(TASKS_EXTENSION_ID.to_string(), serde_json::Map::new());
		capabilities.extensions = Some(extensions);
		capabilities.elicitation = Some(
			ElicitationCapability::new()
				.with_form(FormElicitationCapability::default())
				.with_url(UrlElicitationCapability::default()),
		);
	}
	ClientInfo::new(
		capabilities,
		Implementation::new("octomind", env!("CARGO_PKG_VERSION")),
	)
	.with_protocol_version(protocol_version)
}

fn lifecycle() -> ClientLifecycleMode {
	ClientLifecycleMode::Auto {
		preferred_versions: vec![ProtocolVersion::V_2026_07_28],
		// Version advertised in the initialize handshake when the server
		// proves to be legacy — matches what octomind sent before this refactor.
		legacy_version: Some(ProtocolVersion::V_2025_03_26),
	}
}

fn register(server_name: &str, service: McpService) -> Arc<McpService> {
	if let Some(info) = service.peer_info() {
		super::process::store_server_capabilities(server_name, (*info).clone());
	}
	let service = Arc::new(service);
	CLIENTS
		.write()
		.unwrap()
		.insert(server_name.to_string(), service.clone());
	service
}

/// Get the active connection for a server, if any.
pub fn get(server_name: &str) -> Option<Arc<McpService>> {
	CLIENTS.read().unwrap().get(server_name).cloned()
}

/// True when a connection exists and its service loop is still running.
pub fn is_connected(server_name: &str) -> bool {
	get(server_name).map(|s| !s.is_closed()).unwrap_or(false)
}

/// Remove and cancel a client connection. Safe to call for unknown names.
pub fn disconnect(server_name: &str) {
	if let Some(service) = CLIENTS.write().unwrap().remove(server_name) {
		service.cancellation_token().cancel();
	}
	HTTP_AUTH_TOKENS.write().unwrap().remove(server_name);
}

/// Remove and cancel every client connection (program shutdown).
pub fn disconnect_all() {
	let services: Vec<_> = CLIENTS.write().unwrap().drain().collect();
	for (_, service) in services {
		service.cancellation_token().cancel();
	}
	HTTP_AUTH_TOKENS.write().unwrap().clear();
}

/// Names of all registered client connections.
pub fn connected_names() -> Vec<String> {
	CLIENTS.read().unwrap().keys().cloned().collect()
}

/// Spawn a stdio MCP server process and establish an MCP connection over it.
/// Registers the connection in the registry and returns it.
///
/// Tries the modern lifecycle first (`server/discover` probe with rmcp's
/// in-band legacy fallback). Real-world legacy servers often mishandle the
/// probe beyond what the in-band fallback covers — the Python SDK answers
/// `-32602` instead of `-32601`, and rmcp 1.x servers close the connection
/// outright — so on any failure the child is respawned and spoken to with
/// the legacy `initialize` handshake from the start.
pub async fn connect_stdio(server: &McpServerConfig) -> Result<Arc<McpService>> {
	let modern_err = match connect_stdio_once(server, false).await {
		Ok(service) => return Ok(service),
		Err(e) => e,
	};

	crate::log_debug!(
		"Modern MCP connect failed for '{}' ({}), retrying with legacy initialize",
		server.name(),
		modern_err
	);
	connect_stdio_once(server, true)
		.await
		.map_err(|legacy_err| {
			anyhow!(
				"Failed to initialize MCP server '{}' (modern: {}; legacy: {})",
				server.name(),
				modern_err,
				legacy_err
			)
		})
}

/// Collect `{{ENV:KEY}}` placeholders from a server's command, args, and env
/// values that reference unset or empty environment variables.
/// Returns the list of missing keys (empty = all resolved).
pub(crate) fn missing_env_keys(server: &McpServerConfig) -> Vec<String> {
	let mut keys = Vec::new();
	if let Some(cmd) = server.command() {
		keys.extend(crate::agent::inputs::extract_env_keys(cmd));
	}
	for arg in server.args() {
		keys.extend(crate::agent::inputs::extract_env_keys(arg));
	}
	if let Some(env_map) = server.env() {
		for value in env_map.values() {
			keys.extend(crate::agent::inputs::extract_env_keys(value));
		}
	}
	keys.sort();
	keys.dedup();
	keys.into_iter()
		.filter(|key| std::env::var(key).map(|v| v.is_empty()).unwrap_or(true))
		.collect()
}

/// Resolve `{{ENV:KEY}}` placeholders in a string from the parent environment.
/// Unresolved placeholders are left as-is — callers should guard with
/// `missing_env_keys` before relying on the result.
fn resolve_env_placeholders(s: &str) -> String {
	let mut result = s.to_string();
	for key in crate::agent::inputs::extract_env_keys(s) {
		if let Ok(val) = std::env::var(&key) {
			if !val.is_empty() {
				let placeholder = format!("{{{{ENV:{key}}}}}");
				result = result.replace(&placeholder, &val);
			}
		}
	}
	result
}
/// Single stdio connection attempt. The spawned child is owned by the rmcp
/// transport and killed when the transport drops (including on failure).
async fn connect_stdio_once(server: &McpServerConfig, legacy: bool) -> Result<Arc<McpService>> {
	// Guard: refuse to spawn a stdio server whose {{ENV:KEY}} placeholders
	// reference unset env vars. Spawning with the literal placeholder
	// produces a confusing crash in the child process.
	let missing = missing_env_keys(server);
	if !missing.is_empty() {
		return Err(anyhow!(
			"MCP server '{}' requires env vars: {} — set them before starting the server",
			server.name(),
			missing.join(", ")
		));
	}

	let (command, args, cwd) = match server {
		McpServerConfig::Stdin {
			command, args, cwd, ..
		} => {
			// Resolve {{ENV:KEY}} in command and args from the parent environment.
			let cmd = resolve_env_placeholders(command);
			let resolved_args: Vec<String> =
				args.iter().map(|a| resolve_env_placeholders(a)).collect();
			(cmd, resolved_args, cwd.clone())
		}
		_ => return Err(anyhow!("connect_stdio requires a stdio server config")),
	};
	let server_name = server.name().to_string();

	let mut cmd = tokio::process::Command::new(&command);
	cmd.args(&args);
	if let Some(dir) = &cwd {
		cmd.current_dir(dir);
	}
	// Pass env vars from the server config to the child process.
	// {{ENV:KEY}} placeholders are resolved from the parent environment.
	if let Some(env_map) = server.env() {
		for (key, value) in env_map {
			let resolved = resolve_env_placeholders(value);
			cmd.env(key, resolved);
		}
	}
	// Isolate from the parent process group so terminal Ctrl+C doesn't kill servers.
	#[cfg(unix)]
	cmd.process_group(0);
	#[cfg(windows)]
	cmd.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP

	let (transport, stderr) = rmcp::transport::TokioChildProcess::builder(cmd)
		.stderr(std::process::Stdio::piped())
		.spawn()
		.map_err(|e| anyhow!("Failed to start MCP server '{}': {}", server_name, e))?;

	if let Some(pid) = transport.id() {
		super::process::register_pgid(&server_name, pid);
	}

	// Drain stderr into the per-server diagnostic buffer (last 50 lines).
	if let Some(stderr) = stderr {
		let buf = super::process::stderr_buffer_for(&server_name);
		let sname = server_name.clone();
		tokio::spawn(async move {
			use tokio::io::AsyncBufReadExt;
			let mut lines = tokio::io::BufReader::new(stderr).lines();
			while let Ok(Some(line)) = lines.next_line().await {
				let trimmed = line.trim().to_string();
				if trimmed.is_empty() {
					continue;
				}
				crate::log_debug!("MCP '{}' stderr: {}", sname, trimmed);
				if let Ok(mut b) = buf.lock() {
					b.push(trimmed);
					if b.len() > 50 {
						let drain_count = b.len() - 50;
						b.drain(..drain_count);
					}
				}
			}
		});
	}

	let handler = OctoClientHandler {
		legacy,
		..OctoClientHandler::new(&server_name)
	};
	let mode = if legacy {
		ClientLifecycleMode::Initialize
	} else {
		lifecycle()
	};
	let service = tokio::time::timeout(
		Duration::from_secs(server.timeout_seconds()),
		handler.serve_with_lifecycle(transport, mode),
	)
	.await
	.map_err(|_| anyhow!("Timed out establishing MCP connection to '{}'", server_name))?
	.map_err(|e| anyhow!("Failed to initialize MCP server '{}': {}", server_name, e))?;

	Ok(register(&server_name, service))
}

/// Connect to a remote Streamable HTTP MCP server.
/// Registers the connection in the registry and returns it.
pub async fn connect_http(server: &McpServerConfig) -> Result<Arc<McpService>> {
	let url = match server {
		McpServerConfig::Http { url, .. } => url.clone(),
		_ => return Err(anyhow!("connect_http requires an http server config")),
	};
	let server_name = server.name().to_string();

	let auth_token = fetch_http_token(&url, &server_name).await;

	let make_transport = || {
		let mut config =
			rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
				url.clone(),
			);
		if let Some(token) = &auth_token {
			config = config.auth_header(token.clone());
		}
		rmcp::transport::StreamableHttpClientTransport::with_client(
			reqwest::Client::default(),
			config,
		)
	};

	let handler = OctoClientHandler::new(&server_name);
	let connect_timeout = Duration::from_secs(server.timeout_seconds());
	let service = match tokio::time::timeout(
		connect_timeout,
		handler
			.clone()
			.serve_with_lifecycle(make_transport(), lifecycle()),
	)
	.await
	{
		Ok(Ok(service)) => service,
		Ok(Err(modern_err)) => {
			// Spec-recommended HTTP backward compatibility: when the modern
			// probe fails, retry with the legacy initialize handshake.
			crate::log_debug!(
				"Modern MCP connect failed for '{}' ({}), retrying with legacy initialize",
				server_name,
				modern_err
			);
			let legacy_handler = OctoClientHandler {
				legacy: true,
				..handler
			};
			tokio::time::timeout(
				connect_timeout,
				legacy_handler
					.serve_with_lifecycle(make_transport(), ClientLifecycleMode::Initialize),
			)
			.await
			.map_err(|_| anyhow!("Timed out connecting to MCP server '{}'", server_name))?
			.map_err(|e| {
				anyhow!(
					"Failed to initialize MCP server '{}' (modern: {}; legacy: {})",
					server_name,
					modern_err,
					e
				)
			})?
		}
		Err(_) => {
			return Err(anyhow!(
				"Timed out connecting to MCP server '{}'",
				server_name
			))
		}
	};

	HTTP_AUTH_TOKENS
		.write()
		.unwrap()
		.insert(server_name.clone(), auth_token);
	Ok(register(&server_name, service))
}

/// Resolve the current OAuth access token for an HTTP server via MCP
/// Authorization Discovery (RFC 9728). Returns None when the server needs no
/// auth or no token is obtainable non-interactively.
async fn fetch_http_token(url: &str, server_name: &str) -> Option<String> {
	match oauth::discover_oauth_from_mcp_server(url, server_name).await {
		Ok(discovered) => match oauth::get_access_token(&discovered, server_name, false).await {
			Ok(Some(token)) => Some(token),
			Ok(None) => {
				crate::log_error!(
					"OAuth authentication was cancelled for server '{}'",
					server_name
				);
				None
			}
			Err(e) => {
				crate::log_error!(
					"Failed to get OAuth access token for server '{}': {}",
					server_name,
					e
				);
				None
			}
		},
		Err(e) => {
			crate::log_debug!(
				"MCP Authorization discovery failed for server '{}': {}",
				server_name,
				e
			);
			None
		}
	}
}

/// Get the existing connection for a server or establish a new one.
/// Stdio servers go through process-management bookkeeping in `process.rs`.
pub async fn get_or_connect(server: &McpServerConfig) -> Result<Arc<McpService>> {
	match server.connection_type() {
		McpConnectionType::Http => {
			if let Some(service) = get(server.name()) {
				if !service.is_closed() && http_auth_token_still_current(server).await {
					return Ok(service);
				}
				disconnect(server.name());
			}
			connect_http(server).await
		}
		McpConnectionType::Stdin => {
			if let Some(service) = get(server.name()) {
				if !service.is_closed()
					&& super::process::is_stdio_process_alive(server.name()).unwrap_or(true)
				{
					return Ok(service);
				}
				disconnect(server.name());
			}
			super::process::ensure_server_running(server).await?;
			get(server.name())
				.ok_or_else(|| anyhow!("Server '{}' started but not registered", server.name()))
		}
		McpConnectionType::Builtin => Err(anyhow!("Builtin servers have no MCP client")),
	}
}

/// True when the token store still yields the same bearer token this HTTP
/// connection was created with. The transport's Authorization header is fixed
/// at connect time, so an expired/rotated token requires a reconnect — the
/// pre-rmcp implementation resolved the token before every HTTP call, and
/// this check preserves that self-healing behavior for persistent connections.
async fn http_auth_token_still_current(server: &McpServerConfig) -> bool {
	let McpServerConfig::Http { url, .. } = server else {
		return true;
	};
	let current = fetch_http_token(url, server.name()).await;
	let stored = HTTP_AUTH_TOKENS
		.read()
		.unwrap()
		.get(server.name())
		.cloned()
		.unwrap_or(None);
	if current != stored {
		crate::log_debug!(
			"OAuth token changed for server '{}' — reconnecting with fresh credentials",
			server.name()
		);
		return false;
	}
	true
}

/// List all tools from a server (rmcp drives pagination internally).
pub async fn list_tools(server: &McpServerConfig) -> Result<Vec<rmcp::model::Tool>> {
	let service = get_or_connect(server).await?;
	tokio::time::timeout(
		Duration::from_secs(server.timeout_seconds()),
		service.peer().list_all_tools(),
	)
	.await
	.map_err(|_| anyhow!("tools/list timed out for server '{}'", server.name()))?
	.map_err(|e| anyhow!("tools/list failed for server '{}': {}", server.name(), e))
}

/// Wait until the cancellation token fires. A dropped sender has the same
/// terminal meaning as an explicit cancellation: no owner remains to resume
/// the operation.
async fn wait_cancelled(token: &mut Option<tokio::sync::watch::Receiver<bool>>) {
	match token {
		Some(rx) => {
			while !*rx.borrow() {
				if rx.changed().await.is_err() {
					break;
				}
			}
		}
		None => std::future::pending::<()>().await,
	}
}

fn idle_timeout_message(tool_name: &str, server_name: &str, timeout_seconds: u64) -> String {
	format!(
		"MCP tool '{tool_name}' on '{server_name}' timed out after PT{timeout_seconds}S idle. Cancellation sent; check for side effects before retrying. The call reported no liveness at all, so this is a hung or wedged command, not merely a slow one — a long build or test suite keeps its call alive on its own. Fix or narrow the command rather than detaching it: a detached command discards its output and has to be polled for, which costs more than the wait it avoids."
	)
}

fn absolute_timeout_message(
	tool_name: &str,
	server_name: &str,
	idle_timeout_seconds: u64,
	absolute_timeout_seconds: u64,
) -> String {
	format!(
		"MCP tool '{tool_name}' on '{server_name}' exceeded PT{absolute_timeout_seconds}S total while reporting progress (idle PT{idle_timeout_seconds}S). Cancellation sent; check for side effects before retrying with a smaller call or background task."
	)
}

async fn call_tool_round(
	service: &McpService,
	server: &McpServerConfig,
	params: CallToolRequestParams,
	tool_id: &str,
	cancellation_token: &mut Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<CallToolResponse> {
	let idle_timeout_seconds = server.timeout_seconds();
	let options = PeerRequestOptions::with_timeout(Duration::from_secs(idle_timeout_seconds))
		.reset_timeout_on_progress();
	// Progress keeps a call alive but must not extend it forever: a runaway
	// command that streams output resets the idle timeout on every line, so
	// without a ceiling one spinning tool stalls the whole session until an
	// external kill. The cap scales with the server's configured timeout, so
	// deliberately long-tool servers keep their headroom.
	const PROGRESS_EXTENSION_CAP: u64 = 20;
	let absolute_cap =
		Duration::from_secs(idle_timeout_seconds.saturating_mul(PROGRESS_EXTENSION_CAP));
	let tool_name = params.name.clone();
	let handle = service
		.peer()
		.send_cancellable_request(
			ClientRequest::CallToolRequest(CallToolRequest::new(params)),
			options,
		)
		.await
		.map_err(|e| anyhow!("Failed to send tools/call to '{}': {}", server.name(), e))?;
	let request_id = handle.id.clone();
	let peer = handle.peer.clone();
	let _progress_binding = ProgressTokenBinding::new(&handle.progress_token, tool_id);
	let result = tokio::select! {
		result = handle.await_response() => result,
		_ = tokio::time::sleep(absolute_cap) => {
			let _ = peer
				.notify_cancelled(CancelledNotificationParam::new(
					Some(request_id),
					Some("absolute tool-call time cap exceeded".to_string()),
				))
				.await;
			return Err(anyhow!(absolute_timeout_message(
				&tool_name,
				server.name(),
				idle_timeout_seconds,
				absolute_cap.as_secs(),
			)));
		}
		_ = wait_cancelled(cancellation_token) => {
			let _ = peer
				.notify_cancelled(CancelledNotificationParam::new(
					Some(request_id),
					Some("cancelled by user".to_string()),
				))
				.await;
			return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
		}
	};
	let result = match result {
		Err(ServiceError::Timeout { .. }) => {
			return Err(anyhow!(idle_timeout_message(
				&tool_name,
				server.name(),
				idle_timeout_seconds,
			)))
		}
		other => other?,
	};
	match result {
		ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
		ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
		ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
		_ => Err(anyhow!("Unexpected response type for tools/call")),
	}
}

async fn fulfill_input_requests(
	service: &McpService,
	requests: rmcp::model::InputRequests,
) -> Result<InputResponses> {
	let mut responses = InputResponses::new();
	for (key, request) in requests {
		let context = RequestContext::new(
			rmcp::model::NumberOrString::String(Arc::from(key.as_str())),
			service.peer().clone(),
		);
		let value = match request {
			InputRequest::Elicitation(request) => serde_json::to_value(
				service
					.service()
					.create_elicitation(request.params, context)
					.await
					.map_err(|e| anyhow!(e.to_string()))?,
			)?,
			InputRequest::ListRoots(_) => {
				return Err(anyhow!(
					"server requested deprecated roots/list although Octomind did not advertise roots"
				));
			}
			InputRequest::CreateMessage(_) => {
				return Err(anyhow!(
					"server requested sampling/createMessage although Octomind did not advertise sampling"
				));
			}
			_ => return Err(anyhow!("server returned an unsupported MRTR input request")),
		};
		responses.insert(key, value);
	}
	Ok(responses)
}

async fn sleep_or_cancel(
	duration: Duration,
	cancellation_token: &mut Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<()> {
	tokio::select! {
		_ = tokio::time::sleep(duration) => Ok(()),
		_ = wait_cancelled(cancellation_token) => {
			Err(anyhow::Error::new(crate::session::cancellation::Cancelled))
		}
	}
}

async fn drive_task(
	service: &McpService,
	server: &McpServerConfig,
	task: rmcp::model::CreateTaskResult,
	cancellation_token: &mut Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<rmcp::model::CallToolResult> {
	let task_id = task.task.task_id.clone();
	let mut poll_interval_ms = task.task.poll_interval_ms.unwrap_or(500).max(50);
	loop {
		if cancellation_token.as_ref().is_some_and(|rx| *rx.borrow()) {
			let _ = tokio::time::timeout(
				Duration::from_secs(server.timeout_seconds()),
				service
					.peer()
					.cancel_task(CancelTaskParams::new(task_id.clone())),
			)
			.await;
			return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
		}
		if let Err(error) =
			sleep_or_cancel(Duration::from_millis(poll_interval_ms), cancellation_token).await
		{
			if crate::session::cancellation::is_cancelled(&error) {
				let _ = tokio::time::timeout(
					Duration::from_secs(server.timeout_seconds()),
					service
						.peer()
						.cancel_task(CancelTaskParams::new(task_id.clone())),
				)
				.await;
			}
			return Err(error);
		}
		let state = tokio::select! {
			result = tokio::time::timeout(
				Duration::from_secs(server.timeout_seconds()),
				service.peer().get_task(GetTaskParams::new(task_id.clone())),
			) => result
				.map_err(|_| anyhow!("tasks/get timed out for task '{task_id}'"))?
				.map_err(|e| anyhow!("tasks/get failed for task '{task_id}': {e}"))?,
			_ = wait_cancelled(cancellation_token) => {
				let _ = tokio::time::timeout(
					Duration::from_secs(server.timeout_seconds()),
					service.peer().cancel_task(CancelTaskParams::new(task_id.clone())),
				).await;
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}
		};
		poll_interval_ms = state
			.task
			.task
			.poll_interval_ms
			.unwrap_or(poll_interval_ms)
			.max(50);
		match state.task.payload {
			TaskPayload::Working => {}
			TaskPayload::InputRequired { input_requests } => {
				let responses = fulfill_input_requests(service, input_requests).await?;
				tokio::time::timeout(
					Duration::from_secs(server.timeout_seconds()),
					service
						.peer()
						.update_task(UpdateTaskParams::new(task_id.clone(), responses)),
				)
				.await
				.map_err(|_| anyhow!("tasks/update timed out for task '{task_id}'"))?
				.map_err(|e| anyhow!("tasks/update failed for task '{task_id}': {e}"))?;
			}
			TaskPayload::Completed { result } => {
				return serde_json::from_value(serde_json::Value::Object(result))
					.map_err(|e| anyhow!("invalid completed result for task '{task_id}': {e}"));
			}
			TaskPayload::Failed { error } => {
				return Err(anyhow!(
					"MCP task '{task_id}' failed: {}",
					serde_json::Value::Object(error)
				));
			}
			TaskPayload::Cancelled => {
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}
			_ => return Err(anyhow!("MCP task '{task_id}' returned an unknown status")),
		}
	}
}

/// Execute a tool call with progress-resetting request timeouts, targeted
/// cancellation, SEP-2322 MRTR follow-up rounds, and SEP-2663 task polling.
pub async fn call_tool(
	server: &McpServerConfig,
	call: &McpToolCall,
	mut cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<rmcp::model::CallToolResult> {
	let service = get_or_connect(server).await?;

	// Progress notifications from this call render as the spinner phase (see the
	// notification drain in the chat loop); the phase must not outlive the call.
	struct PhaseGuard;
	impl Drop for PhaseGuard {
		fn drop(&mut self) {
			crate::session::chat::get_animation_manager().clear_phase();
		}
	}
	let _phase_guard = PhaseGuard;

	let mut params = CallToolRequestParams::new(call.tool_name.clone());
	if let serde_json::Value::Object(arguments) = call.parameters.clone() {
		params = params.with_arguments(arguments);
	}

	let mut state_only_rounds = 0usize;
	for _ in 0..DEFAULT_MRTR_MAX_ROUNDS {
		match call_tool_round(
			&service,
			server,
			params.clone(),
			&call.tool_id,
			&mut cancellation_token,
		)
		.await?
		{
			CallToolResponse::Complete(result) => {
				// Register any resource link the tool advertised (a detached
				// background job) BEFORE returning — synchronously, while the
				// tool's response is in hand and the job cannot have exited yet
				// (its `resources/updated` fires only on exit, which the server
				// sends after this response). Doing it here rather than in the
				// batch's later result-processing closes the race where a fast
				// job completes before its link is registered and the completion
				// is dropped.
				crate::session::shell_jobs::note_watched_from_result(&result);
				return Ok(result);
			}
			CallToolResponse::Task(task) => {
				return drive_task(&service, server, task, &mut cancellation_token).await;
			}
			CallToolResponse::InputRequired(result) => {
				let had_requests = result
					.input_requests
					.as_ref()
					.is_some_and(|requests| !requests.is_empty());
				if !had_requests && result.request_state.is_none() {
					return Err(anyhow!(
						"server returned input_required without inputRequests or requestState"
					));
				}
				let responses =
					fulfill_input_requests(&service, result.input_requests.unwrap_or_default())
						.await?;
				params.input_responses = (!responses.is_empty()).then_some(responses);
				params.request_state = result.request_state;
				if had_requests {
					state_only_rounds = 0;
				} else {
					let delay = (50u64.saturating_mul(1 << state_only_rounds.min(3))).min(250);
					sleep_or_cancel(Duration::from_millis(delay), &mut cancellation_token).await?;
					state_only_rounds += 1;
				}
			}
			_ => return Err(anyhow!("server returned an unsupported tools/call result")),
		}
	}
	Err(anyhow!(
		"Tool '{}' exceeded the MCP input-required round limit ({DEFAULT_MRTR_MAX_ROUNDS})",
		call.tool_name
	))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn advertised_mcp3_capabilities_have_handlers() {
		let info = build_client_info(ProtocolVersion::V_2026_07_28);
		assert!(info.capabilities.supports_tasks());
		assert!(info.capabilities.sampling.is_none());
		let elicitation = info
			.capabilities
			.elicitation
			.expect("elicitation must be advertised");
		assert!(elicitation.form.is_some());
		assert!(elicitation.url.is_some());
		assert!(info.capabilities.roots.is_none());
	}

	#[test]
	fn legacy_handshake_does_not_claim_mcp3_features() {
		let info = build_client_info(ProtocolVersion::V_2025_03_26);
		assert!(!info.capabilities.supports_tasks());
		assert!(info.capabilities.elicitation.is_none());
		assert!(info.capabilities.sampling.is_none());
		assert!(info.capabilities.roots.is_none());
	}

	#[test]
	fn idle_timeout_guidance_is_actionable_and_side_effect_safe() {
		let message = idle_timeout_message("deploy", "operations", 30);
		assert!(message.contains("'deploy'"));
		assert!(message.contains("'operations'"));
		assert!(message.contains("PT30S idle"));
		assert!(message.contains("check for side effects"));
		// An idle timeout means no liveness at all, which a slow-but-healthy
		// command cannot produce — it reports progress while it waits. Steering
		// the model to detach instead is what produced launch-then-poll loops,
		// where each check costs a round-trip and re-sends the conversation.
		assert!(message.contains("hung or wedged"));
		assert!(!message.contains("background task"));
		assert!(!message.contains("timeout_seconds"));
	}

	#[test]
	fn absolute_timeout_guidance_distinguishes_progress_from_completion() {
		let message = absolute_timeout_message("index", "search", 30, 600);
		assert!(message.contains("PT600S total"));
		assert!(message.contains("while reporting progress"));
		assert!(message.contains("idle PT30S"));
		assert!(message.contains("check for side effects"));
		assert!(!message.contains("timeout_seconds"));
		assert!(message.len() < 250);
	}

	#[tokio::test]
	async fn dropped_cancellation_sender_is_terminal() {
		let (sender, receiver) = tokio::sync::watch::channel(false);
		drop(sender);
		let mut token = Some(receiver);
		tokio::time::timeout(Duration::from_millis(100), wait_cancelled(&mut token))
			.await
			.expect("dropped sender must wake cancellation waiter");
	}
}
