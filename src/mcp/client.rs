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
	CallToolRequest, CallToolRequestParams, CancelledNotificationParam, ClientCapabilities,
	ClientInfo, ClientRequest, Implementation, ProtocolVersion, ServerResult,
};
use rmcp::service::{
	ClientLifecycleMode, ClientServiceExt, NotificationContext, PeerRequestOptions, RunningService,
};
use rmcp::{ClientHandler, RoleClient};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub type McpService = RunningService<RoleClient, OctoClientHandler>;

lazy_static::lazy_static! {
	/// Active MCP client connections, keyed by server name.
	static ref CLIENTS: RwLock<HashMap<String, Arc<McpService>>> = RwLock::new(HashMap::new());
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
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/progress", &value);
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
}

/// Remove and cancel every client connection (program shutdown).
pub fn disconnect_all() {
	let services: Vec<_> = CLIENTS.write().unwrap().drain().collect();
	for (_, service) in services {
		service.cancellation_token().cancel();
	}
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

/// Single stdio connection attempt. The spawned child is owned by the rmcp
/// transport and killed when the transport drops (including on failure).
async fn connect_stdio_once(server: &McpServerConfig, legacy: bool) -> Result<Arc<McpService>> {
	let (command, args) = match server {
		McpServerConfig::Stdin { command, args, .. } => (command.clone(), args.clone()),
		_ => return Err(anyhow!("connect_stdio requires a stdio server config")),
	};
	let server_name = server.name().to_string();

	let mut cmd = tokio::process::Command::new(&command);
	cmd.args(&args);
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

	// Authentication via MCP Authorization Discovery (RFC 9728), same flow as before.
	let mut auth_token: Option<String> = None;
	match oauth::discover_oauth_from_mcp_server(&url, &server_name).await {
		Ok(discovered) => match oauth::get_access_token(&discovered, &server_name, false).await {
			Ok(Some(token)) => auth_token = Some(token),
			Ok(None) => crate::log_error!(
				"OAuth authentication was cancelled for server '{}'",
				server_name
			),
			Err(e) => crate::log_error!(
				"Failed to get OAuth access token for server '{}': {}",
				server_name,
				e
			),
		},
		Err(e) => crate::log_debug!(
			"MCP Authorization discovery failed for server '{}': {}",
			server_name,
			e
		),
	}

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

	Ok(register(&server_name, service))
}

/// Get the existing connection for a server or establish a new one.
/// Stdio servers go through process-management bookkeeping in `process.rs`.
pub async fn get_or_connect(server: &McpServerConfig) -> Result<Arc<McpService>> {
	if let Some(service) = get(server.name()) {
		if !service.is_closed() {
			return Ok(service);
		}
		disconnect(server.name());
	}
	match server.connection_type() {
		McpConnectionType::Http => connect_http(server).await,
		McpConnectionType::Stdin => {
			super::process::ensure_server_running(server).await?;
			get(server.name())
				.ok_or_else(|| anyhow!("Server '{}' started but not registered", server.name()))
		}
		McpConnectionType::Builtin => Err(anyhow!("Builtin servers have no MCP client")),
	}
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

/// Wait until the cancellation token fires. Mirrors the previous semantics:
/// a dropped sender counts as cancellation.
async fn wait_cancelled(token: Option<tokio::sync::watch::Receiver<bool>>) {
	match token {
		Some(mut rx) => {
			while !*rx.borrow() {
				if rx.changed().await.is_err() {
					break;
				}
			}
		}
		None => std::future::pending::<()>().await,
	}
}

/// Execute a tool call with timeout and cancellation. On cancellation the
/// server is sent `notifications/cancelled` so it can stop the work.
pub async fn call_tool(
	server: &McpServerConfig,
	call: &McpToolCall,
	cancellation_token: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<rmcp::model::CallToolResult> {
	let service = get_or_connect(server).await?;

	let mut params = CallToolRequestParams::new(call.tool_name.clone());
	if let serde_json::Value::Object(arguments) = call.parameters.clone() {
		params = params.with_arguments(arguments);
	}

	let options = PeerRequestOptions::with_timeout(Duration::from_secs(server.timeout_seconds()))
		.reset_timeout_on_progress();
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

	let result = tokio::select! {
		result = handle.await_response() => result,
		_ = wait_cancelled(cancellation_token) => {
			let _ = peer
				.notify_cancelled(CancelledNotificationParam::new(
					Some(request_id),
					Some("cancelled by user".to_string()),
				))
				.await;
			return Err(anyhow!("Tool execution cancelled"));
		}
	};

	match result {
		Ok(ServerResult::CallToolResult(result)) => Ok(result),
		Ok(ServerResult::InputRequiredResult(_)) => Err(anyhow!(
			"Tool '{}' requires interactive input (MRTR), which octomind does not support",
			call.tool_name
		)),
		Ok(_) => Err(anyhow!(
			"Unexpected response type for tools/call '{}'",
			call.tool_name
		)),
		Err(e) => Err(anyhow!(
			"Tool '{}' failed on server '{}': {}",
			call.tool_name,
			server.name(),
			e
		)),
	}
}
