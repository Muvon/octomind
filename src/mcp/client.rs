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
	CacheScope, CallToolRequest, CallToolRequestParams, CallToolResponse, CancelTaskParams, CancelledNotificationParam,
	ClientCapabilities, ClientConfig, ClientRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
	ElicitationCapability, ExtensionCapabilities, FormElicitationCapability, GetTaskParams, Implementation,
	InputRequest, InputResponses, ListToolsRequest, PaginatedRequestParams, ProgressToken, ProtocolVersion, RequestId,
	RequestMetaObject, ServerNotification, ServerResult, SubscriptionFilter, TaskPayload, UpdateTaskParams,
	UrlElicitationCapability, DEFAULT_MRTR_MAX_ROUNDS, TASKS_EXTENSION_ID,
};
use rmcp::service::{
	ClientLifecycleMode, ClientServiceExt, InboundStreamOrigin, NotificationContext, PeerRequestOptions,
	RequestContext, RunningService, ServiceError,
};
use rmcp::{ClientHandler, Peer, RoleClient};
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

	/// Every in-flight `tools/call` round, keyed by server and request id: both
	/// request ids and progress tokens are only unique per connection. Progress
	/// rendering, the round's timers and elicitation routing all look it up.
	static ref IN_FLIGHT_CALLS: RwLock<HashMap<(String, RequestId), InFlightCall>> = RwLock::new(HashMap::new());
}

#[derive(Clone)]
struct InFlightCall {
	server: String,
	progress_token: ProgressToken,
	tool_id: String,
	/// Session that issued the call; `None` outside a session.
	session_id: Option<String>,
	/// Whether a person may be asked on the call's behalf: false for subagents
	/// and detached work.
	interactive: bool,
	/// Woken on every progress notification for the call.
	activity: Arc<tokio::sync::Notify>,
	/// Questions open on the call's behalf; its timers hold while above zero.
	awaiting_user: Arc<tokio::sync::watch::Sender<usize>>,
	/// Cancelled when the round ends, however it ends: a question still open for
	/// it has nobody waiting on the answer.
	ended: tokio_util::sync::CancellationToken,
}

/// Registers a round in `IN_FLIGHT_CALLS` for its lifetime, covering the
/// timeout, cancellation and `?` exits.
struct InFlightCallBinding {
	key: (String, RequestId),
	call: InFlightCall,
}

impl InFlightCallBinding {
	fn new(server: &str, request_id: &RequestId, progress_token: &ProgressToken, tool_id: &str) -> Self {
		let call = InFlightCall {
			server: server.to_string(),
			progress_token: progress_token.clone(),
			tool_id: tool_id.to_string(),
			session_id: crate::session::context::current_session_id(),
			interactive: !crate::agent::inputs::in_non_interactive_scope(),
			activity: Arc::new(tokio::sync::Notify::new()),
			awaiting_user: Arc::new(tokio::sync::watch::Sender::new(0)),
			ended: tokio_util::sync::CancellationToken::new(),
		};
		let key = (server.to_string(), request_id.clone());
		IN_FLIGHT_CALLS.write().unwrap().insert(key.clone(), call.clone());
		Self { key, call }
	}
}

impl Drop for InFlightCallBinding {
	fn drop(&mut self) {
		IN_FLIGHT_CALLS.write().unwrap().remove(&self.key);
		self.call.ended.cancel();
	}
}

/// Holds the timers of the calls a question is asked for until it is
/// answered: the user's answering time is not server time.
struct AwaitingUser(Vec<InFlightCall>);

impl AwaitingUser {
	fn hold(calls: Vec<InFlightCall>) -> Self {
		for call in &calls {
			call.awaiting_user.send_modify(|count| *count += 1);
		}
		Self(calls)
	}
}

impl Drop for AwaitingUser {
	fn drop(&mut self) {
		for call in &self.0 {
			call.awaiting_user.send_modify(|count| *count -= 1);
		}
	}
}

/// The in-flight calls an incoming elicitation belongs to (all from one
/// session, none where nobody can answer), or why none can be chosen.
fn calls_awaiting_elicitation(
	server: &str,
	context: &RequestContext<RoleClient>,
) -> std::result::Result<Vec<InFlightCall>, &'static str> {
	let in_flight = IN_FLIGHT_CALLS.read().unwrap();
	let calls: Vec<InFlightCall> = match context.extensions.get::<InboundStreamOrigin>() {
		// Streamable HTTP delivers it on the response stream of its call.
		Some(InboundStreamOrigin::OutboundRequest(request_id)) => in_flight
			.get(&(server.to_string(), request_id.clone()))
			.cloned()
			.into_iter()
			.collect(),
		Some(InboundStreamOrigin::Unassociated) => return Err("it arrived outside any tool call"),
		// Stdio cannot tell which call it belongs to: SEP-2260 has no wire field.
		None => in_flight
			.values()
			.filter(|call| call.server == server)
			.cloned()
			.collect(),
	};
	let Some(first) = calls.first() else {
		return Err("no tool call on its server is waiting for it");
	};
	if calls.iter().any(|call| call.session_id != first.session_id) {
		return Err("tool calls from several sessions share its server, so its session is ambiguous");
	}
	if calls.iter().any(|call| !call.interactive) {
		return Err("its tool call runs where nobody can answer");
	}
	Ok(calls)
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
		super::process::emit_notification(&self.server_name, method, params, self.session_id.as_deref(), None);
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
	fn get_info(&self) -> ClientConfig {
		build_client_info(if self.legacy {
			ProtocolVersion::V_2025_11_25
		} else {
			ProtocolVersion::V_2026_07_28
		})
	}

	async fn on_progress(
		&self,
		params: rmcp::model::ProgressNotificationParam,
		_context: NotificationContext<RoleClient>,
	) {
		let call = IN_FLIGHT_CALLS
			.read()
			.unwrap()
			.values()
			.find(|call| call.server == self.server_name && call.progress_token == params.progress_token)
			.cloned();
		if let Some(call) = &call {
			call.activity.notify_one();
		}
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit_progress(&value, call.map(|call| call.tool_id));
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

	async fn on_cancelled(&self, params: CancelledNotificationParam, _context: NotificationContext<RoleClient>) {
		let value = serde_json::to_value(&params).unwrap_or(serde_json::Value::Null);
		self.emit("notifications/cancelled", &value);
	}

	async fn on_resource_updated(
		&self,
		params: rmcp::model::ResourceUpdatedNotificationParam,
		context: NotificationContext<RoleClient>,
	) {
		deliver_resource_update(
			&self.server_name,
			self.session_id.as_deref(),
			params.uri,
			context.peer.clone(),
		)
		.await;
	}

	async fn on_resource_list_changed(&self, _context: NotificationContext<RoleClient>) {
		self.emit("notifications/resources/list_changed", &serde_json::Value::Null);
	}

	async fn on_prompt_list_changed(&self, _context: NotificationContext<RoleClient>) {
		self.emit("notifications/prompts/list_changed", &serde_json::Value::Null);
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

	/// A server pushes elicitation while one of its tool calls runs. It goes to
	/// whoever answers for that call's session (`crate::mcp::elicitation`); when
	/// the call cannot be told apart it is declined, never guessed.
	async fn create_elicitation(
		&self,
		request: ElicitRequestParams,
		context: RequestContext<RoleClient>,
	) -> std::result::Result<ElicitResult, rmcp::ErrorData> {
		let calls = match calls_awaiting_elicitation(&self.server_name, &context) {
			Ok(calls) => calls,
			Err(reason) => {
				return Ok(crate::mcp::elicitation::decline(
					&self.server_name,
					self.session_id.as_deref(),
					&request,
					reason,
				))
			}
		};
		let session_id = calls[0].session_id.clone();
		// Several calls of one session can share a stdio connection; the
		// question then belongs to the session, not to one of its calls.
		let tool_id = match calls.as_slice() {
			[call] => Some(call.tool_id.clone()),
			_ => None,
		};
		let ended: Vec<_> = calls.iter().map(|call| call.ended.clone()).collect();
		let _held = AwaitingUser::hold(calls);
		tokio::select! {
			result = crate::mcp::elicitation::ask(&self.server_name, session_id.as_deref(), tool_id.as_deref(), request) => result,
			// The server withdrew the question, e.g. its tool call was cancelled.
			_ = context.ct.cancelled() => Ok(ElicitResult::new(ElicitationAction::Cancel)),
			// Every call it was asked for is over, even if the server never said so.
			_ = futures::future::join_all(ended.iter().map(|ended| ended.cancelled())) => {
				Ok(ElicitResult::new(ElicitationAction::Cancel))
			}
		}
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

/// Deliver one `resources/updated` event for a resource we are following
/// (octofs fires this when a detached shell job exits). Recognition is by
/// membership in the watched set — a resource link a tool handed back — not
/// by any URI scheme, so this stays generic across MCP servers. Read the
/// resource and inject its contents so the run loop wakes the model with the
/// result: event-driven, no polling. The read + push runs in a detached task
/// so it neither blocks the caller nor re-enters the receive loop. Shared by
/// both delivery paths: the unsolicited push (legacy servers) and the
/// `subscriptions/listen` stream (2026-07-28).
async fn deliver_resource_update(server_name: &str, session_id: Option<&str>, uri: String, peer: Peer<RoleClient>) {
	let param = rmcp::model::ResourceUpdatedNotificationParam::new(uri.clone());
	let value = serde_json::to_value(&param).unwrap_or(serde_json::Value::Null);
	super::process::emit_notification(server_name, "notifications/resources/updated", &value, session_id, None);

	// Owned copy: the delivery task outlives this call, and a borrowed
	// &str cannot cross `tokio::spawn`. The caller's session id is only a hint:
	// the unsolicited-push path carries a connect-time snapshot that is empty
	// whenever the connection was established outside a session task, which
	// silently dropped every completion it delivered. The registry owns the
	// mapping, so fall back to it before giving up.
	let session_id = match session_id
		.filter(|id| crate::session::shell_jobs::is_watched_for_session(id, &uri))
		.map(str::to_string)
		.or_else(|| crate::session::shell_jobs::find_session_for_uri(&uri))
	{
		Some(session_id) => session_id,
		None => {
			crate::log_debug!("resource {uri} updated but no session watches it; dropping completion");
			return;
		}
	};
	if !crate::session::shell_jobs::begin_delivery_for_session(&session_id, &uri) {
		crate::log_debug!("resource {uri} update ignored: delivery already in flight");
		return;
	}
	tokio::spawn(async move {
		let body = match peer
			.read_resource(rmcp::model::ReadResourceRequestParams::new(uri.clone()))
			.await
		{
			Ok(result) => result
				.contents
				.into_iter()
				.filter_map(|content| match content {
					rmcp::model::ResourceContents::TextResourceContents { text, .. } => Some(text),
					_ => None,
				})
				.collect::<Vec<_>>()
				.join("\n"),
			Err(error) => {
				format!("resource {uri} updated, but reading it failed: {error}")
			}
		};
		let content =
			crate::session::shell_jobs::delivery_message(&format!("<background_job resource=\"{uri}\">"), &body);
		crate::session::inbox::push_inbox_message_for_session(
			&session_id,
			crate::session::inbox::InboxMessage {
				source: crate::session::inbox::InboxSource::BackgroundJob { id: uri.clone() },
				content,
			},
		);
		crate::session::shell_jobs::complete_for_session(&session_id, &uri, &body);
	});
}

/// SEP-1724 extension telling a server that this client follows the resource
/// links in tool results and surfaces their `resources/updated` to the model
/// (`watch_resource_links` + `on_resource_updated`).
const WATCH_RESOURCE_LINKS_EXTENSION: &str = "io.muvon/watch-resource-links";

/// Client identity + capabilities sent on every request (modern) or during
/// the initialize handshake (legacy). The octomind session context rides in
/// the experimental capabilities, same as before.
fn build_client_info(protocol_version: ProtocolVersion) -> ClientConfig {
	let (role, spec, project, session_id, workdir) = super::process::get_session_context();
	let git = octolib::utils::is_git_repo(std::path::Path::new(&workdir));
	let mut session = serde_json::json!({
		"role": role,
		"spec": spec,
		"project": project,
		"session_id": session_id,
		"workdir": workdir,
		"git": git,
	});
	// Only per-request capabilities can track the epoch: a legacy handshake
	// would pin the value it had at connect time.
	if protocol_version >= ProtocolVersion::V_2026_07_28 {
		session["context_epoch"] = crate::session::context::context_epoch().into();
	}

	let mut capabilities = ClientCapabilities::default();
	if let serde_json::Value::Object(map) = session {
		capabilities.experimental = Some([("session".to_string(), map)].into());
	}

	// Advertise only capabilities that this handler actually services. Roots
	// and Sampling are deprecated by SEP-2577 and are intentionally absent.
	if protocol_version >= ProtocolVersion::V_2026_07_28 {
		let mut extensions = ExtensionCapabilities::new();
		extensions.insert(TASKS_EXTENSION_ID.to_string(), serde_json::Map::new());
		extensions.insert(WATCH_RESOURCE_LINKS_EXTENSION.to_string(), serde_json::Map::new());
		capabilities.extensions = Some(extensions);
	}
	// Offered only where someone may answer. Per-request capabilities speak for
	// the calling session; a legacy handshake speaks for the shared connection.
	let answerable = if protocol_version >= ProtocolVersion::V_2026_07_28 {
		!crate::agent::inputs::in_non_interactive_scope()
			&& crate::session::context::current_session_id()
				.is_some_and(|session_id| crate::mcp::elicitation::answerer(&session_id).is_some())
	} else {
		crate::mcp::elicitation::accepts_elicitations()
	};
	if answerable {
		capabilities.elicitation = Some(
			ElicitationCapability::new()
				.with_form(FormElicitationCapability::default())
				.with_url(UrlElicitationCapability::default()),
		);
	}
	ClientConfig::new(capabilities, Implementation::new("octomind", env!("CARGO_PKG_VERSION")))
		.with_protocol_version(protocol_version)
}

fn lifecycle() -> ClientLifecycleMode {
	ClientLifecycleMode::Auto {
		preferred_versions: vec![ProtocolVersion::V_2026_07_28],
		// Version advertised in the initialize handshake when the server
		// proves to be legacy — the newest pre-2026 revision, so legacy
		// servers can elicit (form mode since 2025-06-18, URL mode since 2025-11-25).
		legacy_version: Some(ProtocolVersion::V_2025_11_25),
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

/// Read all text contents for one resource from an already-connected MCP
/// server. URI schemes are intentionally opaque: ownership comes from the
/// server that returned the `ResourceLink`, not from parsing the URI.
pub async fn read_resource_text(server_name: &str, uri: &str) -> Result<String> {
	let service = get(server_name)
		.filter(|service| !service.is_closed())
		.ok_or_else(|| anyhow!("MCP connection '{server_name}' is not active"))?;
	let result = service
		.peer()
		.read_resource(rmcp::model::ReadResourceRequestParams::new(uri.to_string()))
		.await
		.map_err(|error| anyhow!("resources/read failed for '{uri}': {error}"))?;
	Ok(result
		.contents
		.into_iter()
		.filter_map(|content| match content {
			rmcp::model::ResourceContents::TextResourceContents { text, .. } => Some(text),
			_ => None,
		})
		.collect::<Vec<_>>()
		.join("\n"))
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
	connect_stdio_once(server, true).await.map_err(|legacy_err| {
		anyhow!(
			"Failed to initialize MCP server '{}' (modern: {}; legacy: {})",
			server.name(),
			modern_err,
			legacy_err
		)
	})
}

/// Collect `{{ENV:KEY}}` placeholders from a server's command, args, env, and
/// HTTP header values that reference unset or empty environment variables.
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
	if let Some(headers) = server.headers() {
		for value in headers.values() {
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

#[derive(Debug, PartialEq)]
enum HttpAuthSource {
	StaticHeader,
	OAuthDiscovery,
}

fn http_auth_source(server: &McpServerConfig) -> HttpAuthSource {
	let has_authorization = server
		.headers()
		.is_some_and(|headers| headers.keys().any(|name| name.eq_ignore_ascii_case("authorization")));
	if has_authorization {
		HttpAuthSource::StaticHeader
	} else {
		HttpAuthSource::OAuthDiscovery
	}
}

fn resolve_http_headers(server: &McpServerConfig) -> Result<reqwest::header::HeaderMap> {
	let mut resolved = reqwest::header::HeaderMap::new();
	let Some(headers) = server.headers() else {
		return Ok(resolved);
	};
	for (name, value) in headers {
		let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
			anyhow!(
				"Invalid HTTP header name '{}' for MCP server '{}': {}",
				name,
				server.name(),
				e
			)
		})?;
		let value = resolve_env_placeholders(value);
		let header_value = reqwest::header::HeaderValue::from_str(&value).map_err(|e| {
			anyhow!(
				"Invalid value for HTTP header '{}' on MCP server '{}': {}",
				name,
				server.name(),
				e
			)
		})?;
		resolved.insert(header_name, header_value);
	}
	Ok(resolved)
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
		McpServerConfig::Stdin { command, args, cwd, .. } => {
			// Resolve {{ENV:KEY}} in command and args from the parent environment.
			let cmd = resolve_env_placeholders(command);
			let resolved_args: Vec<String> = args.iter().map(|a| resolve_env_placeholders(a)).collect();
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
	let missing = missing_env_keys(server);
	if !missing.is_empty() {
		return Err(anyhow!(
			"MCP server '{}' requires env vars: {} — set them before connecting",
			server_name,
			missing.join(", ")
		));
	}
	let headers = resolve_http_headers(server)?;
	let http_client = reqwest::Client::builder()
		.default_headers(headers)
		.build()
		.map_err(|e| anyhow!("Failed to build HTTP client for MCP server '{}': {}", server_name, e))?;

	let auth_token = match http_auth_source(server) {
		HttpAuthSource::StaticHeader => None,
		HttpAuthSource::OAuthDiscovery => fetch_http_token(&url, &server_name).await,
	};

	let make_transport = || {
		let mut config =
			rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.clone());
		if let Some(token) = &auth_token {
			config = config.auth_header(token.clone());
		}
		rmcp::transport::StreamableHttpClientTransport::with_client(http_client.clone(), config)
	};

	let handler = OctoClientHandler::new(&server_name);
	let connect_timeout = Duration::from_secs(server.timeout_seconds());
	let service = match tokio::time::timeout(
		connect_timeout,
		handler.clone().serve_with_lifecycle(make_transport(), lifecycle()),
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
				legacy_handler.serve_with_lifecycle(make_transport(), ClientLifecycleMode::Initialize),
			)
			.await
			.map_err(|_| anyhow!("Timed out connecting to MCP server '{}'", server_name))?
			.map_err(|e| {
				let message = format!(
					"Failed to initialize MCP server '{}' (modern: {}; legacy: {})",
					server_name, modern_err, e
				);
				// Keep typed failures available to the health monitor; names and
				// URLs in the diagnostic are not evidence of an HTTP auth status.
				anyhow::Error::new(rmcp::service::ClientInitializeError::LegacyFallbackFailed {
					discover: Box::new(modern_err),
					fallback: Box::new(e),
				})
				.context(message)
			})?
		}
		Err(_) => return Err(anyhow!("Timed out connecting to MCP server '{}'", server_name)),
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
				crate::log_error!("OAuth authentication was cancelled for server '{}'", server_name);
				None
			}
			Err(e) => {
				crate::log_error!("Failed to get OAuth access token for server '{}': {}", server_name, e);
				None
			}
		},
		Err(e) => {
			crate::log_debug!("MCP Authorization discovery failed for server '{}': {}", server_name, e);
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
				if !service.is_closed() && super::process::is_stdio_process_alive(server.name()).unwrap_or(true) {
					return Ok(service);
				}
				disconnect(server.name());
			}
			super::process::ensure_server_running(server).await?;
			get(server.name()).ok_or_else(|| anyhow!("Server '{}' started but not registered", server.name()))
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
	if http_auth_source(server) == HttpAuthSource::StaticHeader {
		return true;
	}
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

/// Request metadata naming the calling session. A connection is shared by every
/// session, but its connect-time capabilities belong to one of them.
fn session_request_meta() -> RequestMetaObject {
	let mut meta = RequestMetaObject::default();
	meta.set_client_capabilities(build_client_info(ProtocolVersion::V_2026_07_28).capabilities);
	meta
}

/// A server's tool list as served to the calling session.
#[derive(Debug)]
pub struct ToolList {
	pub tools: Vec<rmcp::model::Tool>,
	/// The server marked the list `private`: it depends on the requesting
	/// session's context, so it must not be reused for another session.
	pub session_scoped: bool,
}

/// List all tools from a server for the calling session.
///
/// Sent through request options rather than `Peer::list_tools`: rmcp overwrites
/// `params._meta` with the connect-time capabilities, and its response cache is
/// keyed by cursor alone, so it would serve one session's list to another.
pub async fn list_tools(server: &McpServerConfig) -> Result<ToolList> {
	let service = get_or_connect(server).await?;
	let peer = service.peer();
	let pages = async {
		let mut list = ToolList {
			tools: Vec::new(),
			session_scoped: false,
		};
		let mut cursor = None;
		loop {
			let request = ListToolsRequest::with_param(PaginatedRequestParams::default().with_cursor(cursor));
			let response = peer
				.send_request_with_option(
					ClientRequest::ListToolsRequest(request),
					PeerRequestOptions::default().with_meta(session_request_meta()),
				)
				.await?
				.await_response()
				.await?;
			let ServerResult::ListToolsResult(page) = response else {
				return Err(anyhow!("Unexpected response type for tools/list"));
			};
			list.session_scoped |= page.cache_scope == Some(CacheScope::Private);
			list.tools.extend(page.tools);
			cursor = page.next_cursor;
			if cursor.is_none() {
				return Ok(list);
			}
		}
	};
	tokio::time::timeout(Duration::from_secs(server.timeout_seconds()), pages)
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
	let idle_timeout = Duration::from_secs(idle_timeout_seconds);
	// The idle timeout is enforced here, not by rmcp: only server progress can
	// restart rmcp's timer, but a call waiting on the user to answer an
	// elicitation is not idle.
	let options = PeerRequestOptions::no_options().with_meta(session_request_meta());
	// Progress keeps a call alive but must not extend it forever: a runaway
	// command that streams output resets the idle timeout on every line, so
	// without a ceiling one spinning tool stalls the whole session until an
	// external kill. The cap scales with the server's configured timeout, so
	// deliberately long-tool servers keep their headroom.
	const PROGRESS_EXTENSION_CAP: u64 = 20;
	let absolute_cap = Duration::from_secs(idle_timeout_seconds.saturating_mul(PROGRESS_EXTENSION_CAP));
	let tool_name = params.name.clone();
	let handle = service
		.peer()
		.send_cancellable_request(ClientRequest::CallToolRequest(CallToolRequest::new(params)), options)
		.await
		.map_err(|e| anyhow!("Failed to send tools/call to '{}': {}", server.name(), e))?;
	let request_id = handle.id.clone();
	let peer = handle.peer.clone();
	let in_flight = InFlightCallBinding::new(server.name(), &request_id, &handle.progress_token, tool_id);
	let mut awaiting_user = in_flight.call.awaiting_user.subscribe();
	let response = handle.await_response();
	tokio::pin!(response);
	let absolute_deadline = tokio::time::sleep(absolute_cap);
	tokio::pin!(absolute_deadline);
	let mut idle_deadline = tokio::time::Instant::now() + idle_timeout;
	let mut awaiting_since = None;
	let result = loop {
		let awaiting = *awaiting_user.borrow_and_update() > 0;
		tokio::select! {
			biased;
			result = &mut response => break result?,
			_ = in_flight.call.activity.notified() => idle_deadline = tokio::time::Instant::now() + idle_timeout,
			// Both timers hold while the user answers a question for this call:
			// the idle window restarts when one opens or closes, and the total
			// cap moves out by the time it stayed open.
			_ = awaiting_user.changed() => {
				let now = tokio::time::Instant::now();
				idle_deadline = now + idle_timeout;
				match (awaiting_since, *awaiting_user.borrow() > 0) {
					(None, true) => awaiting_since = Some(now),
					(Some(since), false) => {
						let extended = absolute_deadline.deadline() + (now - since);
						absolute_deadline.as_mut().reset(extended);
						awaiting_since = None;
					}
					_ => {}
				}
			}
			_ = tokio::time::sleep_until(idle_deadline), if !awaiting => {
				let _ = peer
					.notify_cancelled(CancelledNotificationParam::new(
						Some(request_id.clone()),
						Some("idle timeout exceeded".to_string()),
					))
					.await;
				return Err(anyhow!(idle_timeout_message(
					&tool_name,
					server.name(),
					idle_timeout_seconds,
				)));
			}
			_ = &mut absolute_deadline, if !awaiting => {
				let _ = peer
					.notify_cancelled(CancelledNotificationParam::new(
						Some(request_id.clone()),
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
						Some(request_id.clone()),
						Some("cancelled by user".to_string()),
					))
					.await;
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}
		}
	};
	match result {
		ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
		ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
		ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
		_ => Err(anyhow!("Unexpected response type for tools/call")),
	}
}

/// Answer a round's input requests. Asked between rounds, so the question is
/// already in the call's own session and no round timer is running.
async fn fulfill_input_requests(
	server: &str,
	tool_id: Option<&str>,
	requests: rmcp::model::InputRequests,
	cancellation_token: &mut Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<InputResponses> {
	let session_id = crate::session::context::current_session_id();
	let mut responses = InputResponses::new();
	for (key, request) in requests {
		let value = match request {
			InputRequest::Elicitation(request) if crate::agent::inputs::in_non_interactive_scope() => {
				serde_json::to_value(crate::mcp::elicitation::decline(
					server,
					session_id.as_deref(),
					&request.params,
					"its tool call runs where nobody can answer",
				))?
			}
			InputRequest::Elicitation(request) => {
				let result = tokio::select! {
					result = crate::mcp::elicitation::ask(server, session_id.as_deref(), tool_id, request.params) => {
						result.map_err(|e| anyhow!(e.to_string()))?
					}
					_ = wait_cancelled(cancellation_token) => {
						return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
					}
				};
				serde_json::to_value(result)?
			}
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

/// Longest wait between `tasks/get` attempts while the server cannot be reached.
const MAX_TASK_POLL_BACKOFF: Duration = Duration::from_secs(60);

/// Poll an MCP task to its end. `on_status` sees the task's state on every
/// `tasks/get`, so a caller can surface its status message while it runs.
///
/// Only the server ends a task: a terminal state, its id no longer known
/// (`-32602`, e.g. after a restart), or another error it returns. A poll that
/// times out or loses the transport says nothing about the task, so polling
/// continues with backoff, reconnecting a closed connection; the caller's
/// cancellation (`monitor` stop, session teardown) bounds the wait.
async fn drive_task(
	mut service: Arc<McpService>,
	server: &McpServerConfig,
	task: rmcp::model::CreateTaskResult,
	cancellation_token: &mut Option<tokio::sync::watch::Receiver<bool>>,
	mut on_status: impl FnMut(&rmcp::model::Task),
) -> Result<rmcp::model::CallToolResult> {
	let task_id = task.task.task_id.clone();
	let mut poll_interval_ms = task.task.poll_interval_ms.unwrap_or(500).max(50);
	// Polls in a row that got no answer; the backoff grows with them.
	let mut failed_polls = 0u32;
	loop {
		if cancellation_token.as_ref().is_some_and(|rx| *rx.borrow()) {
			let _ = tokio::time::timeout(
				Duration::from_secs(server.timeout_seconds()),
				service.peer().cancel_task(CancelTaskParams::new(task_id.clone())),
			)
			.await;
			return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
		}
		let delay = Duration::from_millis(poll_interval_ms.saturating_mul(1u64 << failed_polls.min(10)))
			.min(MAX_TASK_POLL_BACKOFF.max(Duration::from_millis(poll_interval_ms)));
		if let Err(error) = sleep_or_cancel(delay, cancellation_token).await {
			if crate::session::cancellation::is_cancelled(&error) {
				let _ = tokio::time::timeout(
					Duration::from_secs(server.timeout_seconds()),
					service.peer().cancel_task(CancelTaskParams::new(task_id.clone())),
				)
				.await;
			}
			return Err(error);
		}
		let polled = tokio::select! {
			result = tokio::time::timeout(
				Duration::from_secs(server.timeout_seconds()),
				service.peer().get_task(GetTaskParams::new(task_id.clone())),
			) => result,
			_ = wait_cancelled(cancellation_token) => {
				let _ = tokio::time::timeout(
					Duration::from_secs(server.timeout_seconds()),
					service.peer().cancel_task(CancelTaskParams::new(task_id.clone())),
				).await;
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}
		};
		let state = match polled {
			Ok(Ok(state)) => state,
			Ok(Err(ServiceError::McpError(error))) if error.code == rmcp::model::ErrorCode::INVALID_PARAMS => {
				return Err(anyhow!(
					"the server no longer knows MCP task '{task_id}' (it may have restarted): {}",
					error.message
				));
			}
			Ok(Err(
				ServiceError::TransportClosed | ServiceError::TransportSend(_) | ServiceError::Cancelled { .. },
			)) => {
				failed_polls += 1;
				// A fresh connection may be a fresh server process; the next poll
				// learns whether it still knows the task.
				// Teardown closes both the transport and the task's stop channel.
				// Cancellation must win that race before reconnect can spawn a server.
				let reconnected = tokio::select! {
					biased;
					_ = wait_cancelled(cancellation_token) => {
						return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
					}
					result = get_or_connect(server) => result,
				};
				match reconnected {
					Ok(fresh) => service = fresh,
					Err(error) => {
						crate::log_debug!(format!("reconnecting for MCP task '{task_id}' failed: {error}"))
					}
				}
				continue;
			}
			Ok(Err(ServiceError::Timeout { .. })) | Err(_) => {
				failed_polls += 1;
				continue;
			}
			Ok(Err(error)) => {
				return Err(anyhow!("tasks/get failed for task '{task_id}': {error}"));
			}
		};
		failed_polls = 0;
		poll_interval_ms = state.task.task.poll_interval_ms.unwrap_or(poll_interval_ms).max(50);
		on_status(&state.task.task);
		match state.task.payload {
			TaskPayload::Working => {}
			TaskPayload::InputRequired { input_requests } => {
				let responses = fulfill_input_requests(server.name(), None, input_requests, cancellation_token).await?;
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

/// Run an MCP task (SEP-2663) detached from the tool call that created it, the
/// way a detached shell job runs: register it as a pending background job, poll
/// it on its own tokio task, and deliver its outcome to the session inbox, which
/// wakes the model. Returns what the model sees for the call now. The registry
/// entry owns the poller's stop signal: `monitor` stop fires it, and session
/// teardown drops it — either way `drive_task` cancels the task on the server.
fn background_task(
	service: Arc<McpService>,
	server: McpServerConfig,
	task: rmcp::model::CreateTaskResult,
	tool_id: &str,
	session_id: String,
) -> rmcp::model::CallToolResult {
	use crate::session::shell_jobs;

	let key = shell_jobs::task_key(tool_id);
	let label = task
		.task
		.status_message
		.clone()
		.filter(|message| !message.is_empty())
		.unwrap_or_else(|| format!("MCP task {}", task.task.task_id));
	let stop = shell_jobs::register_task_for_session(&session_id, server.name(), &key, &label);
	let notice = format!(
		"Still running as background task `{key}` ({label}). Its result arrives on its own when it \
		 finishes. Do only work that doesn't need it; to wait for it, reply with a brief status and \
		 no tool call. To stop it early, call `monitor` with action `stop` and id `{key}`."
	);

	// Task-locals do not cross `tokio::spawn`: re-enter the session so a reconnect
	// while polling carries this session's context to the server. Nobody waits
	// at the terminal for a detached task, so its elicitations are declined
	// rather than prompted over the session's input line.
	let scope = session_id.clone();
	tokio::spawn(crate::session::context::with_session_id(
		scope,
		crate::agent::inputs::with_non_interactive(async move {
			let mut cancellation_token = Some(stop);
			let outcome = drive_task(service, &server, task, &mut cancellation_token, |state| {
				if let Some(status) = state.status_message.as_deref() {
					shell_jobs::set_task_status_for_session(&session_id, &key, status);
				}
			})
			.await;
			let stopped = cancellation_token.as_ref().is_some_and(|stop| *stop.borrow());
			let (body, is_error) = match outcome {
				Ok(result) => (
					result
						.content
						.iter()
						.filter_map(|block| block.as_text().map(|text| text.text.as_str()))
						.collect::<Vec<_>>()
						.join("\n"),
					result.is_error == Some(true),
				),
				Err(_) if stopped => (
					"stopped on request; the server was asked to cancel it".to_string(),
					true,
				),
				Err(error) if crate::session::cancellation::is_cancelled(&error) => {
					("the server cancelled the task".to_string(), true)
				}
				Err(error) => (format!("{error:#}"), true),
			};
			// False once the session was torn down: nobody is left to deliver to.
			if !shell_jobs::begin_delivery_for_session(&session_id, &key) {
				return;
			}
			let error = if is_error { shell_jobs::DELIVERY_ERROR_ATTR } else { "" };
			let content = shell_jobs::delivery_message(&format!("<background_job task=\"{key}\"{error}>"), &body);
			crate::session::inbox::push_inbox_message_for_session(
				&session_id,
				crate::session::inbox::InboxMessage {
					source: crate::session::inbox::InboxSource::BackgroundJob { id: key.clone() },
					content,
				},
			);
			shell_jobs::complete_for_session(&session_id, &key, &body);
		}),
	));

	rmcp::model::CallToolResult::success(vec![rmcp::model::ContentBlock::text(notice)])
}

/// Open a `subscriptions/listen` stream (MCP 2026-07-28) for every resource
/// link a tool result advertised, so the server delivers the job's completion
/// on an acknowledged, contract-clean channel instead of an unsolicited push.
/// Best-effort by design: on any failure — legacy server, no subscribe
/// capability, transport — the unsolicited-push path in `on_resource_updated`
/// still covers delivery, so this only upgrades the channel, never gates the
/// feature.
async fn watch_resource_links(service: &McpService, server_name: &str, result: &rmcp::model::CallToolResult) {
	use crate::session::shell_jobs::WatchEvent;

	let links = crate::session::shell_jobs::resource_links_in(result);
	if links.is_empty() {
		return;
	}
	// Task-locals do not cross `tokio::spawn`; capture the session now.
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	let session_id = session_id.to_string();
	for (uri, _label) in links {
		let mut filter = SubscriptionFilter::new();
		filter.resource_subscriptions = Some(vec![uri.clone()]);
		let mut subscription = match service.peer().listen(filter).await {
			Ok(subscription) => subscription,
			Err(error) => {
				// Expected for legacy servers (method unknown) — the
				// unsolicited push remains the delivery path.
				crate::log_debug!(format!("subscriptions/listen for {uri} unavailable: {error}"));
				continue;
			}
		};
		let accepted = subscription
			.acknowledged()
			.resource_subscriptions
			.as_ref()
			.is_some_and(|uris| uris.contains(&uri));
		if !accepted {
			let _ = subscription.cancel().await;
			continue;
		}
		// The unsolicited push may have won the race with stream establishment
		// and already delivered the update — nothing left to wait for.
		if !crate::session::shell_jobs::is_watched_for_session(&session_id, &uri) {
			let _ = subscription.cancel().await;
			continue;
		}
		let peer = service.peer().clone();
		let mut events = crate::session::shell_jobs::subscribe_events();
		let server_name = server_name.to_string();
		// Per-link copies: each spawned watcher owns its session id, and the
		// loop keeps using the original for the next link.
		let session_id = session_id.clone();
		let uri = uri.clone();
		tokio::spawn(async move {
			loop {
				tokio::select! {
					biased;
					event = events.recv() => {
						match event {
							Ok(WatchEvent::Completed { session_id: s, uri: u })
								if s == session_id && u == uri =>
							{
								break;
								}
							Ok(WatchEvent::Cleared { session_id: s }) if s == session_id => {
								break;
							}
							_ => {
								continue;
							}
						}
					}
					update = subscription.next() => {
						match update {
							Ok(Some(ServerNotification::ResourceUpdatedNotification(update))) => {
								deliver_resource_update(
									&server_name,
									Some(&session_id),
									update.params.uri,
									peer.clone(),
								)
								.await;
								break;
							}
							// The filter admits only resource updates; anything else
							// means the stream ended or misbehaved — drop it and let
							// the watched-set reminder cover the job.
							Ok(Some(_)) | Ok(None) | Err(_) => {
								break;
							}
						}
					}
				}
			}
			let _ = subscription.cancel().await;
		});
	}
}

/// Execute a tool call with progress-resetting request timeouts, targeted
/// cancellation, SEP-2322 MRTR follow-up rounds, and SEP-2663 tasks.
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
		match call_tool_round(&service, server, params.clone(), &call.tool_id, &mut cancellation_token).await? {
			CallToolResponse::Complete(result) => {
				// Register any resource link the tool advertised (a detached
				// background job) BEFORE returning — synchronously, while the
				// tool's response is in hand and the job cannot have exited yet
				// (its `resources/updated` fires only on exit, which the server
				// sends after this response). Doing it here rather than in the
				// batch's later result-processing closes the race where a fast
				// job completes before its link is registered and the completion
				// is dropped.
				crate::session::shell_jobs::note_watched_from_result(server.name(), &result);
				// Upgrade delivery to a subscriptions/listen stream where the
				// server supports it (2026-07-28). Established before the result
				// is returned so the stream is active before the job can exit.
				watch_resource_links(&service, server.name(), &result).await;
				return Ok(result);
			}
			CallToolResponse::Task(task) => {
				// In a session the task runs in the background like a detached shell
				// job, so a long one never holds the turn. Outside one there is no
				// inbox to deliver it to later, so the call waits for it.
				if let Some(session_id) = crate::session::context::current_session_id() {
					return Ok(background_task(
						service,
						server.clone(),
						task,
						&call.tool_id,
						session_id.to_string(),
					));
				}
				return drive_task(service, server, task, &mut cancellation_token, |_| {}).await;
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
				let responses = fulfill_input_requests(
					server.name(),
					Some(&call.tool_id),
					result.input_requests.unwrap_or_default(),
					&mut cancellation_token,
				)
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
#[path = "client_inline_tests.rs"]
mod inline_tests;

#[cfg(test)]
#[path = "client_stdio_tests.rs"]
mod stdio_tests;

#[cfg(test)]
#[path = "client_tests.rs"]
mod client_tests;

#[cfg(test)]
#[path = "client_coverage_tests.rs"]
mod client_coverage_tests;
