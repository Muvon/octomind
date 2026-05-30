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

//! OctomindAgent — implements the ACP Agent trait over Octomind's session infrastructure.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use agent_client_protocol::{
	AgentCapabilities, AgentSideConnection, AuthenticateRequest, AuthenticateResponse,
	AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, BlobResourceContents,
	CancelNotification, Client, ContentBlock, ContentChunk, EmbeddedResourceResource, ExtRequest,
	ExtResponse, Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest,
	LoadSessionResponse, McpCapabilities, McpServer, NewSessionRequest, NewSessionResponse,
	PromptCapabilities, PromptRequest, PromptResponse, ProtocolVersion, SessionInfoUpdate,
	SessionNotification, SessionUpdate, StopReason, ToolCall, ToolCallStatus, ToolCallUpdate,
	ToolCallUpdateFields, UnstructuredCommandInput,
};

use crate::config::mcp::McpServerConfig;
use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::chat::session::{
	execute_api_call_and_process_response, prepare_for_api_call, run_pipe_if_enabled,
	setup_and_initialize_session, setup_system_prompt_and_cache, ChatSession, GenericSessionArgs,
};
use crate::session::output::{OutputMode, WebSocketSink};
use crate::websocket::ServerMessage;
use crate::{log_debug, log_error, log_info};

/// ACP agent implementation wrapping Octomind's session infrastructure.
///
/// Single-threaded (runs inside a `tokio::task::LocalSet`), so `Rc<RefCell<...>>` is safe.
pub struct OctomindAgent {
	/// Mutable: client-injected MCP servers are merged in on first new_session/load_session.
	config: RefCell<Config>,
	role: String,
	/// Active sessions keyed by ACP session_id, paired with their working directory.
	sessions: Rc<RefCell<HashMap<String, (ChatSession, PathBuf)>>>,
	/// Per-session async exclusion locks. Both `prompt()` and the inbox monitor
	/// acquire this lock before removing the session from `sessions` for exclusive
	/// processing — otherwise a user prompt arriving while the inbox monitor is
	/// mid-API-call (or vice versa) would find the map empty and respond with
	/// `session not found`, which the client interprets as a disconnect.
	session_locks: Rc<RefCell<HashMap<String, Rc<tokio::sync::Mutex<()>>>>>,
	/// Active cancellation handles keyed by ACP session_id
	cancellations: Rc<RefCell<HashMap<String, SessionCancellation>>>,
	/// Connection back to the client — used to send session/update notifications
	conn: Rc<RefCell<Option<Rc<AgentSideConnection>>>>,
	/// Preferred session name for the next `new_session` (consumed once).
	pending_name: RefCell<Option<String>>,
	/// Resume target for the next `new_session` (consumed once).
	pending_resume: RefCell<Option<String>>,
	/// Whether the next `new_session` should resume the most recent session (consumed once).
	pending_resume_recent: RefCell<bool>,
	/// Model override applied to every session (new and loaded).
	model: Option<String>,
	/// Webhook hooks activated for every session (new and loaded).
	hooks: Vec<String>,
}

impl OctomindAgent {
	pub fn new(config: Config, role: String, options: crate::acp::AcpRunOptions) -> Self {
		Self {
			config: RefCell::new(config),
			role,
			sessions: Rc::new(RefCell::new(HashMap::new())),
			session_locks: Rc::new(RefCell::new(HashMap::new())),
			cancellations: Rc::new(RefCell::new(HashMap::new())),
			conn: Rc::new(RefCell::new(None)),
			pending_name: RefCell::new(options.name),
			pending_resume: RefCell::new(options.resume),
			pending_resume_recent: RefCell::new(options.resume_recent),
			model: options.model,
			hooks: options.hooks,
		}
	}

	/// Inject the connection after it's created (chicken-and-egg: agent needs conn, conn needs agent).
	pub fn set_connection(&self, conn: Rc<AgentSideConnection>) {
		*self.conn.borrow_mut() = Some(conn);
	}

	/// Get or create the exclusion lock for a session.
	/// Returns an `Rc<Mutex>` so callers can `lock().await` outside the RefCell borrow.
	fn session_lock(&self, session_id: &str) -> Rc<tokio::sync::Mutex<()>> {
		self.session_locks
			.borrow_mut()
			.entry(session_id.to_string())
			.or_default()
			.clone()
	}

	/// Build session args for a new ACP session, consuming the one-shot CLI overrides.
	fn build_new_session_args(&self) -> GenericSessionArgs {
		GenericSessionArgs {
			role: self.role.clone(),
			mode: "websocket".into(),
			name: self.pending_name.borrow_mut().take(),
			resume: self.pending_resume.borrow_mut().take(),
			resume_recent: std::mem::replace(&mut *self.pending_resume_recent.borrow_mut(), false),
			model: self.model.clone(),
			hooks: self.hooks.clone(),
			..Default::default()
		}
	}

	/// Build session args for an explicit `load_session` (resume by client-supplied id).
	fn build_load_session_args(&self, session_id: String) -> GenericSessionArgs {
		GenericSessionArgs {
			resume: Some(session_id),
			role: self.role.clone(),
			mode: "websocket".into(),
			model: self.model.clone(),
			hooks: self.hooks.clone(),
			..Default::default()
		}
	}
}

/// Convert ACP MCP server list into McpServerConfig entries and inject them into a config snapshot.
///
/// Returns a modified clone of `base_config` with the injected servers merged in.
/// `self.config` is never mutated — injected servers are scoped to the session only.
fn build_config_with_injected_servers(
	base_config: &Config,
	role: &str,
	servers: &[McpServer],
) -> Config {
	let mut config = base_config.clone();
	for server in servers {
		let server_config = match server {
			McpServer::Stdio(s) => {
				let args: Vec<String> = s.args.iter().map(|a| a.to_string()).collect();
				McpServerConfig::stdin(
					&s.name,
					s.command.to_string_lossy().as_ref(),
					args,
					30,
					vec![],
				)
			}
			McpServer::Http(s) => McpServerConfig::http(&s.name, &s.url, 30, vec![]),
			McpServer::Sse(s) => {
				// SSE is not a supported transport in our MCP stack — skip
				log_info!("ACP: skipping SSE MCP server '{}' (not supported)", s.name);
				continue;
			}
			_ => {
				log_info!("ACP: skipping unknown MCP server transport (not supported)");
				continue;
			}
		};
		let name = server_config.name().to_string();
		if !config.mcp.servers.iter().any(|s| s.name() == name) {
			config.mcp.servers.push(server_config);
		}
		if let Some(role_entry) = config.role_map.get_mut(role) {
			if !role_entry.mcp.server_refs.contains(&name) {
				role_entry.mcp.server_refs.push(name);
			}
		}
	}
	config
}

/// Translate an internal `ServerMessage` (the protocol shared with the WebSocket
/// server) into an ACP `SessionUpdate` for forwarding to the client. Returns
/// `None` for messages that have no ACP equivalent (e.g. Cost — sent separately).
fn translate_server_message_to_acp(msg: ServerMessage) -> Option<SessionUpdate> {
	match msg {
		ServerMessage::Assistant(p) => Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
			p.content.into(),
		))),
		ServerMessage::Thinking(p) => Some(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
			p.content.into(),
		))),
		ServerMessage::ToolUse(p) => {
			let tc = ToolCall::new(p.tool_id.clone(), p.tool.clone())
				.status(ToolCallStatus::InProgress)
				.raw_input(p.params.clone());
			Some(SessionUpdate::ToolCall(tc))
		}
		ServerMessage::ToolResult(p) => {
			let status = if p.success {
				ToolCallStatus::Completed
			} else {
				ToolCallStatus::Failed
			};
			let upd = ToolCallUpdate::new(
				p.tool_id.clone(),
				ToolCallUpdateFields::new().status(status).raw_output(
					serde_json::from_str::<serde_json::Value>(&p.content)
						.unwrap_or(serde_json::Value::String(p.content)),
				),
			);
			Some(SessionUpdate::ToolCallUpdate(upd))
		}
		_ => None,
	}
}

/// Build the list of available slash commands to advertise to ACP clients.
///
/// Command names are sent WITHOUT the leading `/` — the client prepends it when displaying.
fn build_available_commands() -> Vec<AvailableCommand> {
	let unstructured =
		|hint: &str| AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(hint));

	vec![
		AvailableCommand::new("help", "Show available commands"),
		AvailableCommand::new("role", "View or change current role")
			.input(unstructured("<role_name>")),
		AvailableCommand::new("model", "View or change current AI model")
			.input(unstructured("<provider:model>")),
		AvailableCommand::new(
			"done",
			"Finalize task with memorization, summarization, and auto-commit",
		),
		AvailableCommand::new("info", "Display token and cost breakdown for this session"),
		AvailableCommand::new("clear", "Clear the screen"),
		AvailableCommand::new("copy", "Copy last response to clipboard"),
		AvailableCommand::new("context", "Display session context")
			.input(unstructured("[all|assistant|user|tool|large]")),
		AvailableCommand::new("list", "List all available sessions").input(unstructured("[page]")),
		AvailableCommand::new("session", "Switch to or create a session")
			.input(unstructured("[session_name]")),
		AvailableCommand::new("run", "Execute a command layer")
			.input(unstructured("<command_name>")),
		AvailableCommand::new("workflow", "Execute a workflow")
			.input(unstructured("<workflow_name> [input]")),
		AvailableCommand::new("mcp", "MCP server management")
			.input(unstructured("[info|list|full|health|dump|validate]")),
		AvailableCommand::new("plan", "Display current plan stored in MCP plan tool"),
		AvailableCommand::new("prompt", "Manage prompt templates")
			.input(unstructured("[template_name]")),
		AvailableCommand::new("image", "Attach image to next message")
			.input(unstructured("<path>")),
		AvailableCommand::new("video", "Attach video to next message")
			.input(unstructured("<path>")),
		AvailableCommand::new("loglevel", "Set logging level")
			.input(unstructured("[none|info|debug]")),
		AvailableCommand::new("report", "Generate detailed usage report for this session"),
		AvailableCommand::new("skill", "List, filter, or toggle skills")
			.input(unstructured("[name|pattern|page]")),
		AvailableCommand::new("effort", "View or change reasoning effort level")
			.input(unstructured("[low|medium|high]")),
		AvailableCommand::new(
			"schedule",
			"Schedule a message to be injected at a future time",
		)
		.input(unstructured(
			"[list|add|remove|edit] [<id>] [when=...] [message=...] [every=...]",
		)),
		AvailableCommand::new("exit", "Exit the session"),
	]
}

/// Send the available commands list to the ACP client for the given session.
async fn send_available_commands(conn: Option<std::rc::Rc<AgentSideConnection>>, session_id: &str) {
	if let Some(conn) = conn {
		let update = SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
			build_available_commands(),
		));
		let notif = SessionNotification::new(std::sync::Arc::<str>::from(session_id), update);
		if let Err(e) = conn.session_notification(notif).await {
			log_error!("ACP: failed to send available_commands_update: {}", e);
		}
	}
}

/// Spawn a background task that monitors schedules and inbox for a session.
///
/// Runs independently of user prompts — when a scheduled entry fires or an inbox
/// message arrives, the task takes the session from the map, processes the message
/// through the full AI pipeline, streams results to the ACP client, and puts the
/// session back. Exits when the session is removed from the map.
fn spawn_inbox_monitor(
	session_id: String,
	sessions: Rc<RefCell<HashMap<String, (ChatSession, PathBuf)>>>,
	session_locks: Rc<RefCell<HashMap<String, Rc<tokio::sync::Mutex<()>>>>>,
	cancellations: Rc<RefCell<HashMap<String, SessionCancellation>>>,
	config: RefCell<Config>,
	role: String,
	conn: Rc<RefCell<Option<Rc<AgentSideConnection>>>>,
) {
	tokio::task::spawn_local(async move {
		log_debug!("ACP: inbox monitor started for session: {}", session_id);
		loop {
			// Process phase: flush due schedules into inbox, then drain.
			// Returns true to exit the monitor loop.
			let should_exit = crate::session::context::with_session_id(session_id.clone(), async {
				crate::mcp::core::flush_due_to_inbox();
				// Idle-mode entries fire here too — ACP monitor runs only when nothing
				// is in flight, so flush_idle_to_inbox()'s idle check covers tap/job state.
				crate::mcp::core::flush_idle_to_inbox();

				// Drain inbox while there are messages. Acquire the per-session
				// exclusion lock BEFORE removing the session from the map so a
				// concurrent user prompt waits instead of seeing "session not found".
				while crate::session::inbox::has_inbox_messages()
					&& sessions.borrow().contains_key(&session_id)
				{
					let inbox_msg = match crate::session::inbox::try_pop_inbox_message() {
						Some(msg) => msg,
						None => break,
					};

					log_debug!(
						"ACP monitor: processing inbox message from {:?} for {}",
						inbox_msg.source,
						session_id
					);

					// Acquire exclusion lock for this session. If prompt() is
					// currently holding it (mid-API-call), we wait here until it
					// releases — instead of removing an empty entry and racing.
					let lock = session_locks
						.borrow_mut()
						.entry(session_id.clone())
						.or_default()
						.clone();
					let _guard = lock.lock().await;

					// Take session for exclusive access.
					let entry = sessions.borrow_mut().remove(&session_id);
					let (mut chat_session, session_cwd) = match entry {
						Some(s) => s,
						None => {
							// Session truly gone (cleanup_session). Drop the message.
							crate::session::inbox::push_inbox_message(inbox_msg);
							return false;
						}
					};

					// Restore working directory for tool calls.
					crate::mcp::set_session_working_directory(session_cwd.clone());
					let config_for_role = config.borrow().get_merged_config_for_role(&role);

					let op_rx = cancellations
						.borrow_mut()
						.entry(session_id.clone())
						.or_default()
						.new_operation();

					// Tell the client what's about to drive the AI, before we kick off
					// the API call. Rendered as a user-side chunk so the client UI shows
					// the injected message in the conversation, prefixed with its source.
					let conn_client = conn.borrow().as_ref().cloned();
					if let Some(c) = conn_client {
						let sid_arc: std::sync::Arc<str> = session_id.as_str().into();
						let text = format!(
							"[{}] {}",
							inbox_msg.source.display_label(),
							inbox_msg.content
						);
						let update =
							SessionUpdate::UserMessageChunk(ContentChunk::new(text.into()));
						let notif = SessionNotification::new(sid_arc, update);
						if let Err(e) = c.session_notification(notif).await {
							log_error!(
								"ACP monitor: failed to send injected-message notification: {}",
								e
							);
						}
					}

					if let Err(e) = chat_session.add_user_message(&inbox_msg.content) {
						log_error!("ACP monitor: failed to add inbox message: {}", e);
						sessions
							.borrow_mut()
							.insert(session_id.clone(), (chat_session, session_cwd));
						continue;
					}

					if let Err(e) =
						prepare_for_api_call(&mut chat_session, &config_for_role, op_rx.clone())
							.await
					{
						log_error!("ACP monitor: failed to prepare API call: {}", e);
						sessions
							.borrow_mut()
							.insert(session_id.clone(), (chat_session, session_cwd));
						continue;
					}

					// Stream results to the ACP client via a forwarding task.
					let (ws_tx, mut ws_rx) =
						tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
					let ws_sink = WebSocketSink::new(ws_tx.clone());

					crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_tx);

					let sid_arc: std::sync::Arc<str> = session_id.as_str().into();
					let conn_for_fwd = conn.borrow().as_ref().cloned();
					let forward_task = tokio::task::spawn_local(async move {
						while let Some(msg) = ws_rx.recv().await {
							if let (Some(update), Some(c)) =
								(translate_server_message_to_acp(msg), conn_for_fwd.as_ref())
							{
								let notif = SessionNotification::new(sid_arc.clone(), update);
								if let Err(e) = c.session_notification(notif).await {
									log_error!("ACP monitor: failed to send notification: {}", e);
								}
							}
						}
					});

					let result = execute_api_call_and_process_response(
						&mut chat_session,
						&config_for_role,
						&role,
						op_rx,
						OutputMode::WebSocket,
						ws_sink,
					)
					.await;

					crate::mcp::process::clear_notification_sender(Some(session_id.clone()));
					let _ = forward_task.await;

					if let Err(e) = result {
						log_debug!("ACP monitor: error processing inbox message: {}", e);
					}

					// Put session back.
					sessions
						.borrow_mut()
						.insert(session_id.clone(), (chat_session, session_cwd));
				}

				false // don't exit
			})
			.await;

			if should_exit {
				break;
			}

			// Exit if session inbox was destroyed (session truly gone via cleanup_session).
			let inbox_gone = crate::session::context::with_session_id(session_id.clone(), async {
				crate::session::inbox::get_inbox_notify().is_none()
			})
			.await;
			if inbox_gone {
				log_debug!("ACP monitor: inbox cleared for {}, exiting", session_id);
				break;
			}

			// Wait for the next event: schedule timer, inbox message, or schedule change.
			// next_schedule_sleep() handles the empty case (waits for schedule-change notify),
			// so no special polling branch is needed.
			crate::session::context::with_session_id(session_id.clone(), async {
				let inbox_notify = crate::session::inbox::get_inbox_notify();
				tokio::select! {
					_ = crate::mcp::core::next_schedule_sleep() => {}
					_ = async {
						if let Some(notify) = inbox_notify {
							notify.notified().await;
						} else {
							std::future::pending::<()>().await;
						}
					} => {}
				}
			})
			.await;
		}
		log_debug!("ACP: inbox monitor exited for session: {}", session_id);
	});
}

#[async_trait::async_trait(?Send)]
impl agent_client_protocol::Agent for OctomindAgent {
	async fn initialize(
		&self,
		args: InitializeRequest,
	) -> agent_client_protocol::Result<InitializeResponse> {
		log_debug!("ACP: initialize from {:?}", args.client_info);

		// Advertise extension capabilities in _meta per ACP spec
		let mut meta = agent_client_protocol::Meta::new();
		meta.insert(
			"octomind.dev".to_string(),
			serde_json::json!({
				"commands": true
			}),
		);

		let response = InitializeResponse::new(ProtocolVersion::LATEST)
			.agent_capabilities(
				AgentCapabilities::default()
					.load_session(true)
					// Advertise HTTP MCP transport support so clients offer us HTTP servers.
					// SSE is not supported — we skip those servers silently in inject_acp_mcp_servers.
					.mcp_capabilities(McpCapabilities::new().http(true))
					.prompt_capabilities(
						PromptCapabilities::default()
							.image(true)
							.embedded_context(true),
					)
					.meta(meta),
			)
			.agent_info(Implementation::new("octomind", env!("CARGO_PKG_VERSION")));
		Ok(response)
	}

	async fn authenticate(
		&self,
		_args: AuthenticateRequest,
	) -> agent_client_protocol::Result<AuthenticateResponse> {
		Ok(AuthenticateResponse::default())
	}

	async fn new_session(
		&self,
		args: NewSessionRequest,
	) -> agent_client_protocol::Result<NewSessionResponse> {
		// Set per-session working directory via thread-local (safe: single-threaded LocalSet)
		crate::mcp::set_session_working_directory(args.cwd.clone());
		let session_cwd = args.cwd.clone();

		// Build a per-session config snapshot with injected servers merged in.
		// self.config is never mutated — injected servers are scoped to this session only.
		let config_snapshot = build_config_with_injected_servers(
			&self.config.borrow(),
			&self.role,
			&args.mcp_servers,
		);

		// Start any newly injected servers and register their tools in the tool map.
		// initialize_mcp_for_role is idempotent: already-running servers and already-registered
		// tools are skipped via config-hash and is_server_already_running_with_config checks.
		crate::mcp::initialize_mcp_for_role(&self.role, &config_snapshot)
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		let session_args = self.build_new_session_args();
		let (mut chat_session, config_for_role, session_role, _, _) =
			setup_and_initialize_session(&session_args, &config_snapshot)
				.await
				.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &session_role, false)
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		let session_id = chat_session.session.info.name.clone();
		log_debug!("ACP: new_session created: {}", session_id);

		// Initialize session-scoped inbox and job manager inside the session context
		// so schedule/inbox storage is keyed to this session ID.
		let role_for_pool = self.role.clone();
		let session_id_for_restore = session_id.clone();
		crate::session::context::with_session_id(session_id.clone(), async move {
			crate::session::context::init_session_services(&role_for_pool);
			crate::mcp::core::plan::core::restore_plan_for_session(&session_id_for_restore);
			crate::mcp::core::schedule::core::restore_schedule_for_session(&session_id_for_restore);
		})
		.await;

		self.sessions
			.borrow_mut()
			.insert(session_id.clone(), (chat_session, session_cwd));
		self.cancellations
			.borrow_mut()
			.insert(session_id.clone(), SessionCancellation::new());

		// Load env skills after session is stored (needs &mut ChatSession).
		// Extract from map, load, put back — can't hold RefCell borrow across await.
		{
			let entry = self.sessions.borrow_mut().remove(&session_id);
			if let Some((mut session, cwd)) = entry {
				let sid = session_id.clone();
				crate::session::context::with_session_id(sid, async {
					crate::mcp::core::skill_auto::load_env_skills(&mut session).await;
					crate::mcp::core::capability::load_env_capabilities(&config_for_role, None)
						.await;
				})
				.await;
				self.sessions
					.borrow_mut()
					.insert(session_id.clone(), (session, cwd));
			}
		}

		let conn = self.conn.borrow().clone();
		send_available_commands(conn, &session_id).await;

		// Spawn independent background task that monitors schedules/inbox
		// and processes messages automatically without waiting for user prompts.
		spawn_inbox_monitor(
			session_id.clone(),
			Rc::clone(&self.sessions),
			Rc::clone(&self.session_locks),
			Rc::clone(&self.cancellations),
			RefCell::new(self.config.borrow().clone()),
			self.role.clone(),
			Rc::clone(&self.conn),
		);

		Ok(NewSessionResponse::new(session_id))
	}

	async fn prompt(&self, args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
		let session_id = args.session_id.to_string();

		// Extract text, images, and videos from prompt content blocks
		let mut text_parts = Vec::new();
		let mut images = Vec::new();
		let mut videos = Vec::new();
		for block in &args.prompt {
			match block {
				ContentBlock::Text(t) => text_parts.push(t.text.as_str()),
				ContentBlock::Image(img) => {
					images.push(crate::session::image::ImageAttachment {
						data: crate::session::image::ImageData::Base64(img.data.clone()),
						media_type: img.mime_type.clone(),
						source_type: crate::session::image::SourceType::Url, // ACP images are inline data, closest match
						dimensions: None,
						size_bytes: None,
					});
				}
				ContentBlock::Resource(res) => {
					// Extract video from embedded blob resources (ACP has no native video block)
					if let EmbeddedResourceResource::BlobResourceContents(BlobResourceContents {
						blob,
						mime_type: Some(mime),
						..
					}) = &res.resource
					{
						if mime.starts_with("video/") {
							videos.push(crate::session::video::VideoAttachment {
								data: crate::session::video::VideoData::Base64(blob.clone()),
								media_type: mime.clone(),
								source_type: crate::session::video::SourceType::Url,
								dimensions: None,
								size_bytes: None,
								duration_secs: None,
							});
						}
					}
				}
				_ => {} // Skip audio, resource links, etc.
			}
		}
		let mut input: String = text_parts.join("\n");

		if input.trim().is_empty() && images.is_empty() && videos.is_empty() {
			return Ok(PromptResponse::new(StopReason::EndTurn));
		}

		// Wrap in session context so all session-scoped registries (schedule store,
		// inbox, plan storage, etc.) route to this session's state.
		crate::session::context::with_session_id(session_id.clone(), async {
			// Acquire per-session exclusion lock for the duration of this prompt.
			// The inbox monitor takes the same lock before processing scheduled
			// or inbox messages, so the two never race on `sessions.remove()`.
			// Without this, a monitor-driven API call in flight would leave the
			// map empty and any concurrent user prompt would get
			// `Invalid params: "session not found: ..."` (which the client then
			// surfaces as a disconnect).
			let lock = self.session_lock(&session_id);
			let _guard = lock.lock().await;

			// /done <instructions>: compress then process instructions as a user message.
			// Must be intercepted before the slash-command block because we need to
			// fall through to the user-message pipeline after compression.
			let done_instructions: Option<String> = if input.trim().starts_with(crate::session::chat::DONE_COMMAND) {
				input.trim()
					.strip_prefix(crate::session::chat::DONE_COMMAND)
					.map(|s| s.trim())
					.filter(|s| !s.is_empty())
					.map(|s| s.to_owned())
			} else {
				None
			};
			if input.trim() == crate::session::chat::DONE_COMMAND || done_instructions.is_some() {
				let (mut chat_session, session_cwd) =
					match self.sessions.borrow_mut().remove(&session_id) {
						Some(s) => s,
						None => {
							return Err(agent_client_protocol::Error::invalid_params()
								.data(format!("session not found: {session_id}")));
						}
					};
				crate::mcp::set_session_working_directory(session_cwd.clone());
				let operation_rx = self
					.cancellations
					.borrow_mut()
					.entry(session_id.clone())
					.or_default()
					.new_operation();
				let config_for_role = self.config.borrow().get_merged_config_for_role(&self.role);
				let status_text = match crate::session::chat::session::commands::handle_done(
					&mut chat_session,
					&config_for_role,
					operation_rx,
				)
				.await
				{
					Ok(crate::session::chat::session::commands::DoneOutcome::Compressed) => {
						"✅ Conversation compressed.".to_string()
					}
					Ok(crate::session::chat::session::commands::DoneOutcome::NothingToCompress) => {
						"ℹ️ Nothing to compress.".to_string()
					}
					Ok(crate::session::chat::session::commands::DoneOutcome::Failed(e)) => {
						format!("❌ Compression failed: {e}")
					}
					Err(e) => format!("❌ Compression failed: {e}"),
				};
				if let Some(instructions) = done_instructions {
					// Send compression status then fall through to user-message processing.
					let conn = self.conn.borrow().clone();
					if let Some(conn) = conn {
						let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(status_text.into()));
						let notif = SessionNotification::new(
							std::sync::Arc::<str>::from(session_id.as_str()),
							update,
						);
						if let Err(e) = conn.session_notification(notif).await {
							crate::log_error!("ACP: failed to send /done status: {}", e);
						}
					}
					// Put session back so the user-message code path can remove it again.
					if let Err(e) = chat_session.save() {
						crate::log_debug!("session save failed: {}", e);
					}
					self.sessions
						.borrow_mut()
						.insert(session_id.clone(), (chat_session, session_cwd));
					// Rewrite input to the trailing instructions so the user-message
					// block below processes them as the new prompt.
					input = instructions;
				} else {
					// Plain /done: compress, send status, return.
					if let Err(e) = chat_session.save() {
						crate::log_debug!("session save failed: {}", e);
					}
					self.sessions
						.borrow_mut()
						.insert(session_id.clone(), (chat_session, session_cwd));
					let conn = self.conn.borrow().clone();
					if let Some(conn) = conn {
						let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(status_text.into()));
						let notif = SessionNotification::new(
							std::sync::Arc::<str>::from(session_id.as_str()),
							update,
						);
						if let Err(e) = conn.session_notification(notif).await {
							crate::log_error!("ACP: failed to send /done status: {}", e);
						}
					}
					return Ok(PromptResponse::new(StopReason::EndTurn));
				}
			}

			// Slash commands are sent as regular prompts per the ACP spec.
			// Intercept them here before the AI pipeline, execute via process_command,
			// and stream the result back as an AgentMessageChunk.
			if input.trim_start().starts_with('/') {
				let (mut chat_session, session_cwd) =
					match self.sessions.borrow_mut().remove(&session_id) {
						Some(s) => s,
						None => {
							return Err(agent_client_protocol::Error::invalid_params()
								.data(format!("session not found: {session_id}")));
						}
					};

				crate::mcp::set_session_working_directory(session_cwd.clone());

				let operation_rx = self
					.cancellations
					.borrow_mut()
					.entry(session_id.clone())
					.or_default()
					.new_operation();

				let mut config = self.config.borrow().clone();

				// /done is now intercepted above; this branch handles all other slash commands.
				let result = crate::session::chat::session::commands::process_command(
					&mut chat_session,
					input.trim(),
					&mut config,
					&self.role,
					operation_rx,
				)
				.await;
				// Write back any config mutations (e.g. model/role changes)
				*self.config.borrow_mut() = config;

				self.sessions
					.borrow_mut()
					.insert(session_id.clone(), (chat_session, session_cwd));

				let text = match result {
					Ok(
						crate::session::chat::session::commands::CommandResult::HandledWithOutput(
							output,
						),
					) => serde_json::to_string_pretty(&output.to_json())
						.unwrap_or_else(|_| "Command executed.".to_string()),
					Ok(crate::session::chat::session::commands::CommandResult::Handled) => {
						"Command executed.".to_string()
					}
					Ok(crate::session::chat::session::commands::CommandResult::Exit) => {
						"Session exit requested.".to_string()
					}
					Ok(
						crate::session::chat::session::commands::CommandResult::TreatAsUserInput,
					) => {
						let available: Vec<&str> = crate::session::chat::COMMANDS.to_vec();

						format!(
						"The {} command is not supported by Octomind.\n\nAvailable commands: {}",
						input.trim(),
						available.join(", ")
					)
					}
					Err(e) => format!("Command failed: {e}"),
				};

				let conn = self.conn.borrow().clone();
				if let Some(conn) = conn {
					let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into()));
					let notif = SessionNotification::new(
						std::sync::Arc::<str>::from(session_id.as_str()),
						update,
					);
					if let Err(e) = conn.session_notification(notif).await {
						log_error!("ACP: failed to send command result: {}", e);
					}
				}

				return Ok(PromptResponse::new(StopReason::EndTurn));
			}

			// Take session out of map for exclusive access
			let (mut chat_session, session_cwd) =
				match self.sessions.borrow_mut().remove(&session_id) {
					Some(s) => s,
					None => {
						return Err(agent_client_protocol::Error::invalid_params()
							.data(format!("session not found: {session_id}")));
					}
				};

			// Restore this session's working directory for tool calls
			crate::mcp::set_session_working_directory(session_cwd.clone());

			let config_for_role = self.config.borrow().get_merged_config_for_role(&self.role);
			let current_dir = session_cwd.clone();

			// Get or create cancellation for this session
			let mut cancellation = self
				.cancellations
				.borrow_mut()
				.remove(&session_id)
				.unwrap_or_default();
			cancellation.reset();
			let operation_rx = cancellation.new_operation();
			// Re-insert cancellation so cancel() can find it during prompt execution
			self.cancellations
				.borrow_mut()
				.insert(session_id.clone(), cancellation);

			// Flush any due schedule entries and process inbox messages that arrived
			// before this user prompt (background agents, scheduled entries, skills).
			{
				crate::mcp::core::flush_due_to_inbox();
				while let Some(inbox_msg) = crate::session::inbox::try_pop_inbox_message() {
					log_debug!(
						"ACP pre-user: processing inbox message from {:?}",
						inbox_msg.source
					);
					// Surface the injected message to the client as a user-side chunk so
					// the user sees what triggered the AI's upcoming response.
					let conn_client = self.conn.borrow().as_ref().cloned();
					if let Some(c) = conn_client {
						let sid_arc: std::sync::Arc<str> = session_id.as_str().into();
						let text = format!(
							"[{}] {}",
							inbox_msg.source.display_label(),
							inbox_msg.content
						);
						let update =
							SessionUpdate::UserMessageChunk(ContentChunk::new(text.into()));
						let notif = SessionNotification::new(sid_arc, update);
						if let Err(e) = c.session_notification(notif).await {
							log_error!("ACP: failed to send injected-message notification: {}", e);
						}
					}
					if let Err(e) = chat_session.add_user_message(&inbox_msg.content) {
						log_error!("ACP: failed to add inbox message: {}", e);
						continue;
					}
					let op_rx = self
						.cancellations
						.borrow_mut()
						.entry(session_id.to_string())
						.or_default()
						.new_operation();
					if let Err(e) =
						prepare_for_api_call(&mut chat_session, &config_for_role, op_rx.clone())
							.await
					{
						log_error!("ACP: failed to prepare inbox API call: {}", e);
						continue;
					}

					// Stream the AI's response to this inbox message back to the client,
					// just like a normal user prompt. Without this forwarding the
					// receiver would drop and all chunks would silently disappear.
					let (ws_tx, mut ws_rx) =
						tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
					let sink = WebSocketSink::new(ws_tx.clone());
					crate::mcp::process::set_notification_sender(
						Some(session_id.to_string()),
						ws_tx,
					);

					let sid_arc: std::sync::Arc<str> = session_id.as_str().into();
					let conn_for_fwd = self.conn.borrow().as_ref().cloned();
					let forward_task = tokio::task::spawn_local(async move {
						while let Some(msg) = ws_rx.recv().await {
							if let (Some(update), Some(c)) =
								(translate_server_message_to_acp(msg), conn_for_fwd.as_ref())
							{
								let notif = SessionNotification::new(sid_arc.clone(), update);
								if let Err(e) = c.session_notification(notif).await {
									log_error!("ACP pre-user: failed to send notification: {}", e);
								}
							}
						}
					});

					let result = execute_api_call_and_process_response(
						&mut chat_session,
						&config_for_role,
						&self.role,
						op_rx,
						OutputMode::WebSocket,
						sink,
					)
					.await;

					crate::mcp::process::clear_notification_sender(Some(session_id.to_string()));
					let _ = forward_task.await;

					if let Err(e) = result {
						log_debug!("ACP: error processing pre-user inbox message: {}", e);
					}

					// Persist updated session.info (tokens/cost accumulated via
					// add_assistant_message) so `/info` and resume show real stats.
					if let Err(e) = chat_session.save() {
						log_debug!("ACP: failed to save session after inbox message: {}", e);
					}
				}
			}

			// Pipe pre-processing (runs matching [[pipe]] from guardrails before the main model).
			// On error we MUST re-insert chat_session before returning — otherwise the
			// session is permanently lost from self.sessions and every subsequent
			// prompt to this session_id fails with "session not found".
			let first_message_processed = !chat_session.session.messages.is_empty();
			let pipe_result =
				run_pipe_if_enabled(&input, &self.role, first_message_processed).await;
			let processed_input = match pipe_result {
				Ok(v) => v,
				Err(e) => {
					self.sessions
						.borrow_mut()
						.insert(session_id.clone(), (chat_session, session_cwd));
					return Err(agent_client_protocol::Error::internal_error().data(e.to_string()));
				}
			};

			// Attach ACP images/videos as pending so add_user_message picks them up
			if let Some(first_image) = images.into_iter().next() {
				chat_session.pending_image = Some(first_image);
			}
			if let Some(first_video) = videos.into_iter().next() {
				chat_session.pending_video = Some(first_video);
			}

			// Add user message
			let final_input = crate::session::chat::session::utils::append_constraints_if_exists(
				&processed_input,
				&config_for_role.custom_constraints_file_name,
				&current_dir,
			);
			if let Err(e) = chat_session.add_user_message(&final_input) {
				self.sessions
					.borrow_mut()
					.insert(session_id.clone(), (chat_session, session_cwd));
				return Err(agent_client_protocol::Error::internal_error().data(e.to_string()));
			}

			// Prepare for API call
			if let Err(e) =
				prepare_for_api_call(&mut chat_session, &config_for_role, operation_rx.clone())
					.await
			{
				self.sessions
					.borrow_mut()
					.insert(session_id.clone(), (chat_session, session_cwd));
				return Err(agent_client_protocol::Error::internal_error().data(e.to_string()));
			}

			// Channel-based sink: session pipeline emits ServerMessages, we forward them as ACP notifications
			let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
			let ws_sink = WebSocketSink::new(ws_tx.clone());

			// Forward MCP server notifications through the same channel.
			// Safe: prompt() holds exclusive access to the session (removed from map above),
			// so no two prompts for the same session can race on this global sender.
			crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_tx);

			// Spawn a local task to stream notifications to the client in real-time
			// while the API call runs concurrently. The channel closes when ws_sink drops.
			// Use Arc<str> so each SessionNotification::new() call clones the Arc pointer
			// rather than allocating a new String per notification.
			let session_id_for_task: std::sync::Arc<str> = session_id.as_str().into();
			let conn_for_task = self.conn.borrow().as_ref().cloned();
			let forward_task = tokio::task::spawn_local(async move {
				while let Some(msg) = ws_rx.recv().await {
					let update = match msg {
						ServerMessage::Assistant(p) => Some(SessionUpdate::AgentMessageChunk(
							ContentChunk::new(p.content.into()),
						)),
						ServerMessage::Thinking(p) => Some(SessionUpdate::AgentThoughtChunk(
							ContentChunk::new(p.content.into()),
						)),
						ServerMessage::ToolUse(p) => {
							let tool_call = ToolCall::new(p.tool_id.clone(), p.tool.clone())
								.status(ToolCallStatus::InProgress)
								.raw_input(p.params.clone());
							Some(SessionUpdate::ToolCall(tool_call))
						}
						ServerMessage::ToolResult(p) => {
							let status = if p.success {
								ToolCallStatus::Completed
							} else {
								ToolCallStatus::Failed
							};
							let update = ToolCallUpdate::new(
								p.tool_id.clone(),
								ToolCallUpdateFields::new().status(status).raw_output(
									serde_json::from_str::<serde_json::Value>(&p.content)
										.unwrap_or(serde_json::Value::String(p.content)),
								),
							);
							Some(SessionUpdate::ToolCallUpdate(update))
						}
						// Forward cost / token usage as a SessionInfoUpdate carrying an
						// `octomind.usage` block in `_meta`. We don't gate on the unstable
						// `UsageUpdate` variant — `_meta` is the spec-blessed extensibility
						// channel and works on all 0.10.x clients that pass `_meta` through.
						// Side-channel: we send the notification ourselves here (bypassing
						// the `update` pattern below) because we need to attach `meta`.
						ServerMessage::Cost(p) => {
							if let Some(conn) = conn_for_task.as_ref() {
								let mut meta = serde_json::Map::new();
								meta.insert(
									"octomind.usage".to_string(),
									serde_json::json!({
										"session_tokens":     p.session_tokens,
										"session_cost":       p.session_cost,
										"input_tokens":       p.input_tokens,
										"output_tokens":      p.output_tokens,
										"cache_read_tokens":  p.cache_read_tokens,
										"cache_write_tokens": p.cache_write_tokens,
										"reasoning_tokens":   p.reasoning_tokens,
									}),
								);
								let notif = SessionNotification::new(
									session_id_for_task.clone(),
									SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new()),
								)
								.meta(meta);
								if let Err(e) = conn.session_notification(notif).await {
									log_error!("ACP: failed to send usage notification: {}", e);
								}
							}
							None
						}
						_ => None,
					};
					if let (Some(update), Some(conn)) = (update, conn_for_task.as_ref()) {
						let notif = SessionNotification::new(session_id_for_task.clone(), update);
						if let Err(e) = conn.session_notification(notif).await {
							log_error!("ACP: failed to send session notification: {}", e);
						}
					}
				}
			});

			// Execute the AI call
			let api_result = execute_api_call_and_process_response(
				&mut chat_session,
				&config_for_role,
				&self.role,
				operation_rx.clone(),
				// Reuse WebSocket output mode — ACP and WebSocket both use the same
				// channel-based ServerMessage sink; the transport layer differs, not the pipeline.
				OutputMode::WebSocket,
				ws_sink,
			)
			.await;

			// Clear the global notification sender so the channel can close.
			// Without this, forward_task.await hangs forever because NOTIFICATION_SENDER
			// holds a clone of ws_tx, preventing the channel from closing.
			crate::mcp::process::clear_notification_sender(Some(session_id.clone()));

			// Wait for the forwarding task to drain any remaining messages
			let _ = forward_task.await;

			// Persist updated session.info (tokens/cost accumulated via
			// add_assistant_message during the API call) so `/info` and
			// resume show real stats. Mirrors the WebSocket server pattern.
			if let Err(e) = chat_session.save() {
				log_debug!("ACP: failed to save session after prompt: {}", e);
			}

			// Put session back and wake inbox monitor if it has pending messages.
			self.sessions
				.borrow_mut()
				.insert(session_id.to_string(), (chat_session, session_cwd));
			if let Some(notify) = crate::session::inbox::get_inbox_notify() {
				notify.notify_one();
			}

			match api_result {
				Ok(_) => {
					if *operation_rx.borrow() {
						Ok(PromptResponse::new(StopReason::Cancelled))
					} else {
						Ok(PromptResponse::new(StopReason::EndTurn))
					}
				}
				Err(e) => {
					log_error!("ACP: prompt API call failed: {}", e);
					Err(agent_client_protocol::Error::internal_error().data(e.to_string()))
				}
			}
		})
		.await
	}

	async fn cancel(&self, args: CancelNotification) -> agent_client_protocol::Result<()> {
		let session_id = args.session_id.to_string();
		log_debug!("ACP: cancel requested for session: {}", session_id);
		// Safe to borrow while prompt() may be awaiting: we run inside a LocalSet
		// (single-threaded), so cancel() only executes when prompt() yields at an await
		// point — the RefCell is never doubly borrowed on the same call stack.
		if let Some(cancellation) = self.cancellations.borrow().get(&session_id) {
			cancellation.shutdown();
		}
		Ok(())
	}

	async fn load_session(
		&self,
		args: LoadSessionRequest,
	) -> agent_client_protocol::Result<LoadSessionResponse> {
		let session_id = args.session_id.to_string();
		log_debug!("ACP: load_session requested: {}", session_id);

		// Set per-session working directory via thread-local
		crate::mcp::set_session_working_directory(args.cwd.clone());
		let session_cwd = args.cwd.clone();

		// Build a per-session config snapshot with injected servers merged in.
		// self.config is never mutated — injected servers are scoped to this session only.
		let config_snapshot = build_config_with_injected_servers(
			&self.config.borrow(),
			&self.role,
			&args.mcp_servers,
		);
		crate::mcp::initialize_mcp_for_role(&self.role, &config_snapshot)
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		// Resume the existing session from disk by its ID
		let session_args = self.build_load_session_args(session_id.clone());
		let (mut chat_session, config_for_role, session_role, _, _) =
			setup_and_initialize_session(&session_args, &config_snapshot)
				.await
				.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &session_role, false)
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		let actual_session_id = chat_session.session.info.name.clone();

		// Initialize session-scoped inbox, job manager, and skill pool inside the session context
		// so schedule/inbox storage is keyed to this session ID.
		let role_for_pool = self.role.clone();
		let session_id_for_restore = actual_session_id.clone();
		crate::session::context::with_session_id(actual_session_id.clone(), async move {
			crate::session::context::init_session_services(&role_for_pool);
			crate::mcp::core::plan::core::restore_plan_for_session(&session_id_for_restore);
			crate::mcp::core::schedule::core::restore_schedule_for_session(&session_id_for_restore);
		})
		.await;

		self.sessions
			.borrow_mut()
			.insert(session_id.clone(), (chat_session, session_cwd));
		self.cancellations
			.borrow_mut()
			.insert(session_id.clone(), SessionCancellation::new());

		// Load env skills after session is stored
		{
			let entry = self.sessions.borrow_mut().remove(&session_id);
			if let Some((mut session, cwd)) = entry {
				let sid = actual_session_id.clone();
				crate::session::context::with_session_id(sid, async {
					crate::mcp::core::skill_auto::load_env_skills(&mut session).await;
					crate::mcp::core::capability::load_env_capabilities(&config_for_role, None)
						.await;
				})
				.await;
				self.sessions
					.borrow_mut()
					.insert(session_id.clone(), (session, cwd));
			}
		}

		let conn = self.conn.borrow().clone();
		send_available_commands(conn, &session_id).await;

		// Spawn independent background task that monitors schedules/inbox
		// and processes messages automatically without waiting for user prompts.
		spawn_inbox_monitor(
			session_id.clone(),
			Rc::clone(&self.sessions),
			Rc::clone(&self.session_locks),
			Rc::clone(&self.cancellations),
			RefCell::new(self.config.borrow().clone()),
			self.role.clone(),
			Rc::clone(&self.conn),
		);

		Ok(LoadSessionResponse::new())
	}

	async fn ext_method(&self, args: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
		super::commands::handle_ext_method(
			args,
			&self.sessions,
			&self.config,
			&self.role,
			&self.cancellations,
		)
		.await
	}
}
