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

// WebSocket server implementation

use super::protocol::{
	ClientMessage, CommandMessage, CostPayload, ServerMessage, SessionMessage, StatusPayload,
	UserMessage,
};
use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::chat::session::{
	execute_api_call_and_process_response, prepare_for_api_call, process_layers_if_enabled,
	setup_and_initialize_session, setup_system_prompt_and_cache, ChatSession, GenericSessionArgs,
};
use crate::session::output::{OutputMode, WebSocketSink};
use crate::{log_debug, log_error, log_info};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Per-session processing locks to prevent concurrent access to the same session.
type SessionLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

/// WebSocket server for handling AI sessions
pub struct WebSocketServer {
	addr: SocketAddr,
	config: Arc<Config>,
	role: String,
}

impl WebSocketServer {
	/// Create a new WebSocket server
	pub fn new(host: &str, port: u16, config: Config, role: String) -> Result<Self> {
		let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
		Ok(Self {
			addr,
			config: Arc::new(config),
			role,
		})
	}

	/// Start the WebSocket server
	pub async fn start(&self) -> Result<()> {
		let listener = TcpListener::bind(&self.addr).await?;
		log_info!("WebSocket server listening on ws://{}", self.addr);
		println!("🚀 WebSocket server started on ws://{}", self.addr);
		println!("Press Ctrl+C to stop the server");

		// Active sessions map (session_id -> ChatSession)
		let sessions: Arc<Mutex<HashMap<String, ChatSession>>> =
			Arc::new(Mutex::new(HashMap::new()));

		// Per-session processing locks — prevents concurrent access to the same session
		// from different connections. The lock is held during the entire message processing.
		let session_locks: SessionLocks = Arc::new(Mutex::new(HashMap::new()));

		loop {
			match listener.accept().await {
				Ok((stream, peer_addr)) => {
					log_info!("Connection accepted from {}", peer_addr);

					let config = Arc::clone(&self.config);
					let role = self.role.clone();
					let sessions = Arc::clone(&sessions);
					let session_locks = Arc::clone(&session_locks);

					tokio::spawn(async move {
						if let Err(e) = handle_connection(
							stream,
							peer_addr,
							config,
							role,
							sessions,
							session_locks,
						)
						.await
						{
							log_error!("Connection handler failed for {}: {}", peer_addr, e);
						}
					});
				}
				Err(e) => {
					log_error!("Failed to accept connection: {}", e);
				}
			}
		}
	}
}

/// Handle a single WebSocket connection
async fn handle_connection(
	stream: TcpStream,
	peer_addr: SocketAddr,
	config: Arc<Config>,
	role: String,
	sessions: Arc<Mutex<HashMap<String, ChatSession>>>,
	session_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
) -> Result<()> {
	// Accept WebSocket connection with compression enabled
	let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
		.max_message_size(Some(10 * 1024 * 1024)) // 10MB max message size
		.max_frame_size(Some(10 * 1024 * 1024)) // 10MB max frame size
		.accept_unmasked_frames(false);

	let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(ws_config)).await?;
	log_info!("WebSocket handshake completed for {}", peer_addr);

	let (mut ws_sender, mut ws_receiver) = ws_stream.split();

	// Track session IDs used on this connection for cleanup on disconnect
	let mut active_session_ids: HashSet<String> = HashSet::new();

	// Send welcome message
	let welcome = ServerMessage::status(
		format!("Connected to Octomind WebSocket server (role: {})", role),
		None,
	);
	send_message(&mut ws_sender, &welcome).await?;

	// Channel for background tasks (schedule monitors) to send messages to the client.
	// Background tasks can't access ws_sender directly (not Send), so they push
	// ServerMessages here and the connection loop forwards them.
	let (bg_tx, mut bg_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();

	// Process messages from both WebSocket and background tasks
	loop {
		tokio::select! {
			ws_msg = ws_receiver.next() => {
				let msg = match ws_msg {
					Some(msg) => msg,
					None => break, // stream ended
				};
				match msg {
					Ok(Message::Text(text)) => {
						log_debug!("Received message from {}: {} bytes", peer_addr, text.len());

						// Parse client message
						let client_msg = match serde_json::from_str::<ClientMessage>(&text) {
							Ok(msg) => {
								log_debug!("Parsed message: {:?}", msg);
								msg
							}
							Err(e) => {
								log_error!("Invalid JSON from {}: {}", peer_addr, e);
								let error = ServerMessage::error(format!("Invalid JSON: {}", e));
								send_message(&mut ws_sender, &error).await?;
								continue;
							}
						};

						// Validate message
						if let Err(e) = client_msg.validate() {
							log_error!("Message validation failed from {}: {}", peer_addr, e);
							let error = ServerMessage::error(e);
							send_message(&mut ws_sender, &error).await?;
							continue;
						}

						// Process the message
						if let Err(e) = process_client_message(
							client_msg,
							&mut ws_sender,
							&config,
							&role,
							&sessions,
							&session_locks,
							&mut active_session_ids,
							&bg_tx,
						)
						.await
						{
							log_error!("Message processing failed for {}: {}", peer_addr, e);
							let error = ServerMessage::error(format!("Internal error: {}", e));
							send_message(&mut ws_sender, &error).await?;
						}
					}
					Ok(Message::Close(_)) => {
						log_info!("Client {} closed connection", peer_addr);
						break;
					}
					Ok(Message::Ping(data)) => {
						log_debug!("Ping received from {}", peer_addr);
						// Respond to ping with pong
						if let Err(e) = ws_sender.send(Message::Pong(data)).await {
							log_error!("Failed to send pong to {}: {}", peer_addr, e);
							break;
						}
					}
					Ok(_) => {
						// Ignore other message types (binary, pong, etc.)
					}
					Err(e) => {
						log_error!("WebSocket protocol error from {}: {}", peer_addr, e);
						break;
					}
				}
			}
			// Forward background messages (from schedule/inbox monitors) to the client.
			bg_msg = bg_rx.recv() => {
				if let Some(msg) = bg_msg {
					if let Err(e) = send_message(&mut ws_sender, &msg).await {
						log_error!("Failed to forward background message: {}", e);
						break;
					}
				}
			}
		}
	}

	// Clean up notification senders for sessions used on this connection.
	// Sessions themselves persist (can be resumed on another connection).
	//
	// INVARIANT: Do NOT call stop_all_servers() here. MCP server processes are shared
	// across all active sessions. Killing them on disconnect would break other sessions
	// that are still using the same servers. stop_all_servers() is only called on
	// process shutdown (main.rs) or role switch (CLI only). Use release_server() for
	// per-session server teardown when reference counting is needed.
	for sid in &active_session_ids {
		crate::session::context::clear_notification_sender_for_session(sid);
	}
	log_info!(
		"Connection closed: {} (cleaned up {} session(s))",
		peer_addr,
		active_session_ids.len()
	);
	Ok(())
}

/// Process a client message and send responses
#[allow(clippy::too_many_arguments)]
async fn process_client_message(
	client_msg: ClientMessage,
	ws_sender: &mut futures_util::stream::SplitSink<
		WebSocketStream<TcpStream>,
		tokio_tungstenite::tungstenite::Message,
	>,
	config: &Config,
	role: &str,
	sessions: &Arc<Mutex<HashMap<String, ChatSession>>>,
	session_locks: &SessionLocks,
	active_session_ids: &mut HashSet<String>,
	bg_tx: &tokio::sync::mpsc::UnboundedSender<ServerMessage>,
) -> Result<()> {
	match client_msg {
		ClientMessage::Session(msg) => {
			handle_session_message(
				msg,
				ws_sender,
				config,
				role,
				sessions,
				active_session_ids,
				bg_tx,
			)
			.await
		}
		ClientMessage::Message(msg) => {
			let session_id = msg.session_id.clone();
			active_session_ids.insert(session_id.clone());

			// Acquire per-session lock to prevent concurrent access
			let lock = get_or_create_session_lock(&session_id, session_locks).await;
			let guard = match lock.try_lock() {
				Ok(guard) => guard,
				Err(_) => {
					let error = ServerMessage::error(format!(
						"Session '{}' is busy processing another request. Please wait.",
						session_id
					));
					send_message(ws_sender, &error).await?;
					return Ok(());
				}
			};

			let result = crate::session::context::with_session_id(session_id, async {
				handle_user_message(msg, ws_sender, config, role, sessions).await
			})
			.await;

			drop(guard);
			result
		}
		ClientMessage::Command(msg) => {
			let session_id = msg.session_id.clone();
			active_session_ids.insert(session_id.clone());

			// Acquire per-session lock to prevent concurrent access
			let lock = get_or_create_session_lock(&session_id, session_locks).await;
			let guard = match lock.try_lock() {
				Ok(guard) => guard,
				Err(_) => {
					let error = ServerMessage::error(format!(
						"Session '{}' is busy processing another request. Please wait.",
						session_id
					));
					send_message(ws_sender, &error).await?;
					return Ok(());
				}
			};

			let result = crate::session::context::with_session_id(session_id, async {
				handle_command_message(msg, ws_sender, config, role, sessions).await
			})
			.await;

			drop(guard);
			result
		}
	}
}

/// Get or create a per-session processing lock.
async fn get_or_create_session_lock(
	session_id: &str,
	session_locks: &SessionLocks,
) -> Arc<tokio::sync::Mutex<()>> {
	let mut locks = session_locks.lock().await;
	locks
		.entry(session_id.to_string())
		.or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
		.clone()
}

/// Spawn a background task that monitors schedules and inbox for a WebSocket session.
///
/// Runs independently of user prompts — when a scheduled entry fires or an inbox
/// message arrives, the task takes the session from the map, processes the message
/// through the full AI pipeline, sends results through `bg_tx` for the connection
/// loop to forward to the client, and puts the session back.
/// Exits when the session is removed from the map or the bg_tx channel is closed.
fn spawn_ws_inbox_monitor(
	session_id: String,
	sessions: Arc<Mutex<HashMap<String, ChatSession>>>,
	config: Config,
	role: String,
	bg_tx: tokio::sync::mpsc::UnboundedSender<ServerMessage>,
) {
	tokio::spawn(async move {
		log_debug!(
			"WebSocket: inbox monitor started for session: {}",
			session_id
		);
		loop {
			// Process phase: flush due schedules into inbox, then drain.
			// Returns true to exit the monitor loop.
			let should_exit = crate::session::context::with_session_id(session_id.clone(), async {
				crate::mcp::core::flush_due_to_inbox();
				crate::mcp::core::flush_idle_to_inbox();

				// Drain inbox only when session is available.
				// If held by handle_user_message(), skip — it fires inbox_notify when done,
				// which wakes us from the wait section to retry.
				while crate::session::inbox::has_inbox_messages()
					&& sessions.lock().await.contains_key(&session_id)
				{
					let inbox_msg = match crate::session::inbox::try_pop_inbox_message() {
						Some(msg) => msg,
						None => break,
					};

					log_debug!(
						"WS monitor: processing inbox message from {:?} for {}",
						inbox_msg.source,
						session_id
					);

					// Take session for exclusive access.
					let mut chat_session = match sessions.lock().await.remove(&session_id) {
						Some(s) => s,
						None => {
							// Taken between check and remove. Put message back —
							// handle_user_message() will fire inbox_notify when it returns the session.
							crate::session::inbox::push_inbox_message(inbox_msg);
							return false;
						}
					};

					let config_for_role = config.get_merged_config_for_role(&role);
					let mut cancellation = crate::session::cancellation::SessionCancellation::new();
					let op_rx = cancellation.new_operation();

					// Notify the client what's about to drive the AI, before we kick off
					// the API call. Mirrors `display_injected_input` in CLI mode.
					let _ = bg_tx.send(ServerMessage::Injected(
						crate::websocket::protocol::InjectedPayload {
							source_kind: inbox_msg.source.display_kind().to_string(),
							source_label: inbox_msg.source.display_label(),
							content: inbox_msg.content.clone(),
							session_id: session_id.clone(),
						},
					));

					if let Err(e) = chat_session.add_user_message(&inbox_msg.content) {
						log_error!("WS monitor: failed to add inbox message: {}", e);
						sessions
							.lock()
							.await
							.insert(session_id.clone(), chat_session);
						continue;
					}

					if let Err(e) =
						prepare_for_api_call(&mut chat_session, &config_for_role, op_rx.clone())
							.await
					{
						log_error!("WS monitor: failed to prepare API call: {}", e);
						sessions
							.lock()
							.await
							.insert(session_id.clone(), chat_session);
						continue;
					}

					// Stream results through a channel that feeds into bg_tx.
					let (ws_tx, mut ws_rx) =
						tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
					let ws_sink = WebSocketSink::new(ws_tx.clone());

					crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_tx);

					let bg_tx_fwd = bg_tx.clone();
					let forward_task = tokio::spawn(async move {
						while let Some(msg) = ws_rx.recv().await {
							if bg_tx_fwd.send(msg).is_err() {
								break; // connection closed
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
						log_debug!("WS monitor: error processing inbox message: {}", e);
					}

					// Send cost update after processing.
					let total_tokens = chat_session.session.info.input_tokens
						+ chat_session.session.info.output_tokens
						+ chat_session.session.info.cache_read_tokens
						+ chat_session.session.info.cache_write_tokens
						+ chat_session.session.info.reasoning_tokens;
					let _ = bg_tx.send(ServerMessage::Cost(CostPayload {
						session_tokens: total_tokens,
						session_cost: chat_session.session.info.total_cost,
						input_tokens: chat_session.session.info.input_tokens,
						output_tokens: chat_session.session.info.output_tokens,
						cache_read_tokens: chat_session.session.info.cache_read_tokens,
						cache_write_tokens: chat_session.session.info.cache_write_tokens,
						reasoning_tokens: chat_session.session.info.reasoning_tokens,
						session_id: session_id.clone(),
					}));

					// Save and put session back.
					if let Err(e) = chat_session.save() {
						log_error!("WS monitor: failed to save session: {}", e);
					}
					sessions
						.lock()
						.await
						.insert(session_id.clone(), chat_session);
				}

				false // don't exit
			})
			.await;

			if should_exit || bg_tx.is_closed() {
				break;
			}

			// Exit if session inbox was destroyed (session truly gone via cleanup_session).
			let inbox_gone = crate::session::context::with_session_id(session_id.clone(), async {
				crate::session::inbox::get_inbox_notify().is_none()
			})
			.await;
			if inbox_gone {
				log_debug!("WS monitor: inbox cleared for {}, exiting", session_id);
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
		log_debug!(
			"WebSocket: inbox monitor exited for session: {}",
			session_id
		);
	});
}

/// Handle a "session" type message: create new or resume existing session.
/// No AI call is made — just session setup. Responds with session_id.
async fn handle_session_message(
	msg: SessionMessage,
	ws_sender: &mut futures_util::stream::SplitSink<
		WebSocketStream<TcpStream>,
		tokio_tungstenite::tungstenite::Message,
	>,
	config: &Config,
	role: &str,
	sessions: &Arc<Mutex<HashMap<String, ChatSession>>>,
	active_session_ids: &mut HashSet<String>,
	bg_tx: &tokio::sync::mpsc::UnboundedSender<ServerMessage>,
) -> Result<()> {
	log_debug!("Handling session message: session_id={:?}", msg.session_id);

	let (mut chat_session, config_for_role, session_role, is_new) = match &msg.session_id {
		Some(session_id) => {
			// session_id present: create-or-resume
			// Check memory first
			let existing = sessions.lock().await.remove(session_id);
			if let Some(session) = existing {
				log_debug!("Resumed session from memory: {}", session_id);
				let cfg = config.get_merged_config_for_role(role);
				let role_name = role.to_string();
				(session, cfg, role_name, false)
			} else {
				// Try disk: resume if exists, create with this name if not
				let args = if crate::session::get_sessions_dir()
					.map(|d| d.join(format!("{}.jsonl", session_id)).exists())
					.unwrap_or(false)
				{
					log_debug!("Resuming session from disk: {}", session_id);
					GenericSessionArgs {
						resume: Some(session_id.clone()),
						role: role.to_string(),
						mode: "websocket".into(),
						..Default::default()
					}
				} else {
					log_debug!("Creating named session: {}", session_id);
					GenericSessionArgs {
						name: Some(session_id.clone()),
						role: role.to_string(),
						mode: "websocket".into(),
						..Default::default()
					}
				};

				match setup_and_initialize_session(&args, config).await {
					Ok((session, cfg, role_name, _, _)) => {
						let is_new = !session.was_resumed;
						(session, cfg, role_name, is_new)
					}
					Err(e) => {
						let error =
							ServerMessage::error(format!("Failed to initialize session: {}", e));
						send_message(ws_sender, &error).await?;
						return Ok(());
					}
				}
			}
		}
		None => {
			// No session_id: create new auto-named session
			log_debug!("Creating new auto-named session with role: {}", role);
			let args = GenericSessionArgs {
				role: role.to_string(),
				mode: "websocket".into(),
				..Default::default()
			};
			match setup_and_initialize_session(&args, config).await {
				Ok((session, cfg, role_name, _, _)) => (session, cfg, role_name, true),
				Err(e) => {
					let error = ServerMessage::error(format!("Failed to create session: {}", e));
					send_message(ws_sender, &error).await?;
					return Ok(());
				}
			}
		}
	};

	let session_id = chat_session.session.info.name.clone();
	active_session_ids.insert(session_id.clone());

	// Wrap in session context so all session-scoped registries route correctly
	let role_for_pool = session_role.clone();
	crate::session::context::with_session_id(session_id.clone(), async {
		// Initialize session-scoped inbox, job manager, and skill pool so
		// schedule/inbox/skill storage is keyed to this session ID.
		crate::session::context::init_session_services(&role_for_pool);
		crate::mcp::core::plan::core::restore_plan_for_session(&session_id);
		crate::mcp::core::schedule::core::restore_schedule_for_session(&session_id);
		crate::mcp::core::skill_auto::load_env_skills(&mut chat_session).await;
		crate::mcp::core::capability::load_env_capabilities(&config_for_role, None).await;

		setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &session_role, false)
			.await?;

		let status_msg = if is_new {
			format!("Session created: {}", session_id)
		} else {
			format!("Session resumed: {}", session_id)
		};

		log_info!("{}", status_msg);

		chat_session.save()?;
		sessions
			.lock()
			.await
			.insert(session_id.clone(), chat_session);

		send_message(
			ws_sender,
			&ServerMessage::status(status_msg, Some(session_id.clone())),
		)
		.await?;
		Ok::<(), anyhow::Error>(())
	})
	.await?;

	// Spawn independent background task that monitors schedules/inbox
	// and processes messages automatically without waiting for user prompts.
	spawn_ws_inbox_monitor(
		session_id,
		Arc::clone(sessions),
		config.clone(),
		role.to_string(),
		bg_tx.clone(),
	);

	Ok(())
}

/// Look up an existing session: memory first, then disk. Never auto-create.
/// Returns the session or a ServerMessage error suitable for sending to the client.
async fn lookup_session(
	session_id: &str,
	sessions: &Arc<Mutex<HashMap<String, ChatSession>>>,
	config: &Config,
	role: &str,
) -> std::result::Result<ChatSession, ServerMessage> {
	let existing = sessions.lock().await.remove(session_id);
	if let Some(session) = existing {
		log_debug!("Resumed session from memory: {}", session_id);
		return Ok(session);
	}

	log_debug!("Loading session from disk: {}", session_id);
	let mut args = GenericSessionArgs::resume(session_id.to_string(), role.to_string());
	args.mode = "websocket".to_string();
	match setup_and_initialize_session(&args, config).await {
		Ok((mut session, config_for_role, session_role, _, _)) => {
			if let Err(e) =
				setup_system_prompt_and_cache(&mut session, &config_for_role, &session_role, false)
					.await
			{
				return Err(ServerMessage::error(format!(
					"Failed to setup session {}: {}",
					session_id, e
				)));
			}
			log_info!("Session loaded from disk: {}", session_id);
			Ok(session)
		}
		Err(_) => Err(ServerMessage::error(format!(
			"Session not found: {}. Send a \"session\" message first to create or resume a session.",
			session_id
		))),
	}
}

/// Handle a "command" type message: execute a session command (e.g. /info, /model, /mcp list).
/// Requires session_id and command. Args are optional.
/// The command is assembled into the CLI slash-command format and routed through process_command.
async fn handle_command_message(
	msg: CommandMessage,
	ws_sender: &mut futures_util::stream::SplitSink<
		WebSocketStream<TcpStream>,
		tokio_tungstenite::tungstenite::Message,
	>,
	config: &Config,
	role: &str,
	sessions: &Arc<Mutex<HashMap<String, ChatSession>>>,
) -> Result<()> {
	let session_id = msg.session_id.as_str();
	let command_name = msg.command.trim();
	let args = msg.args.as_slice();

	// Build the slash-command string exactly as the CLI would receive it
	let slash_command = if args.is_empty() {
		format!("/{}", command_name)
	} else {
		format!("/{} {}", command_name, args.join(" "))
	};

	log_debug!(
		"Handling command message: session_id={}, command={}",
		session_id,
		slash_command
	);

	let mut chat_session = match lookup_session(session_id, sessions, config, role).await {
		Ok(s) => s,
		Err(error) => {
			send_message(ws_sender, &error).await?;
			return Ok(());
		}
	};

	let session_id = session_id.to_string();
	let config_for_role = config.get_merged_config_for_role(role);
	let mut cancellation = SessionCancellation::new();
	let operation_rx = cancellation.new_operation();

	// Handle /done specially — it hits unreachable!() in process_command
	// because the CLI intercepts it before routing. We handle it here directly.
	if command_name == "done" {
		use crate::session::chat::session::commands::handle_done;
		match handle_done(&mut chat_session, &config_for_role, operation_rx).await {
			Ok(_) => {
				let status = ServerMessage::status(
					"Conversation compressed".to_string(),
					Some(session_id.clone()),
				);
				send_message(ws_sender, &status).await?;
			}
			Err(e) => {
				let error = ServerMessage::error(format!("Compression failed: {}", e));
				send_message(ws_sender, &error).await?;
			}
		}
		chat_session.save()?;
		sessions.lock().await.insert(session_id, chat_session);
		return Ok(());
	}

	use crate::session::chat::session::commands::CommandResult;
	let command_result = chat_session
		.process_command(
			&slash_command,
			&mut config_for_role.clone(),
			role,
			operation_rx,
		)
		.await?;

	match command_result {
		CommandResult::Handled => {
			log_debug!("Command '{}' executed successfully", slash_command);
			let status = ServerMessage::status(
				format!("Command '{}' executed successfully", slash_command),
				Some(session_id.clone()),
			);
			send_message(ws_sender, &status).await?;
		}
		CommandResult::HandledWithOutput(command_output) => {
			log_debug!(
				"Command '{}' executed with structured output",
				slash_command
			);
			let response = ServerMessage::Status(StatusPayload {
				message: format!("Command '{}' executed successfully", slash_command),
				session_id: Some(session_id.clone()),
				data: Some(command_output.to_json()),
			});
			send_message(ws_sender, &response).await?;
		}
		CommandResult::Exit => {
			log_info!("Session ended by command '{}'", slash_command);
			let status =
				ServerMessage::status("Session ended".to_string(), Some(session_id.clone()));
			send_message(ws_sender, &status).await?;
			// Don't store session back — it's ended
			return Ok(());
		}
		CommandResult::TreatAsUserInput => {
			// Command not recognised — return error rather than silently treating as AI input
			let error = ServerMessage::error(format!(
				"Unknown command: '{}'. Use type \"message\" to send user input.",
				slash_command
			));
			send_message(ws_sender, &error).await?;
		}
	}

	chat_session.save()?;
	sessions.lock().await.insert(session_id, chat_session);
	Ok(())
}

/// Handle a "message" type message: send content to an existing session and get AI response.
/// session_id must refer to an already-established session (from a prior "session" message).
async fn handle_user_message(
	msg: UserMessage,
	ws_sender: &mut futures_util::stream::SplitSink<
		WebSocketStream<TcpStream>,
		tokio_tungstenite::tungstenite::Message,
	>,
	config: &Config,
	role: &str,
	sessions: &Arc<Mutex<HashMap<String, ChatSession>>>,
) -> Result<()> {
	let session_id = msg.session_id.as_str();
	let input = msg.content.clone();

	log_debug!(
		"Handling user message: session_id={}, content_len={}",
		session_id,
		input.len()
	);

	let mut chat_session = match lookup_session(session_id, sessions, config, role).await {
		Ok(s) => s,
		Err(error) => {
			send_message(ws_sender, &error).await?;
			return Ok(());
		}
	};

	let session_id = session_id.to_string();

	// Get current directory for file operations
	let current_dir = crate::mcp::get_thread_working_directory();
	let config_for_role = config.get_merged_config_for_role(role);
	let mut cancellation = SessionCancellation::new();
	let operation_rx = cancellation.new_operation();

	// Process any inbox messages that arrived before this user message
	// (background agents, scheduled entries, skills) — each gets its own
	// full AI turn so the model actually responds to them, not just sees them
	// silently prepended to the conversation.
	{
		// Flush due schedule entries first.
		crate::mcp::core::flush_due_to_inbox();
		crate::mcp::core::flush_idle_to_inbox();
		while let Some(inbox_msg) = crate::session::inbox::try_pop_inbox_message() {
			log_debug!(
				"WebSocket pre-user: processing inbox message from {:?}",
				inbox_msg.source
			);
			// Tell the client what's being injected before the AI responds to it.
			send_message(
				ws_sender,
				&ServerMessage::Injected(crate::websocket::protocol::InjectedPayload {
					source_kind: inbox_msg.source.display_kind().to_string(),
					source_label: inbox_msg.source.display_label(),
					content: inbox_msg.content.clone(),
					session_id: session_id.clone(),
				}),
			)
			.await?;
			chat_session.add_user_message(&inbox_msg.content)?;
			let op_rx = cancellation.new_operation();
			prepare_for_api_call(&mut chat_session, &config_for_role, op_rx.clone()).await?;
			let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
			let sink = WebSocketSink::new(tx);
			let result = execute_api_call_and_process_response(
				&mut chat_session,
				&config_for_role,
				role,
				op_rx,
				OutputMode::WebSocket,
				sink,
			)
			.await;
			while let Ok(msg) = rx.try_recv() {
				send_message(ws_sender, &msg).await?;
			}
			if let Err(e) = result {
				log_debug!("Error processing pre-user inbox message (websocket): {}", e);
			}
		}
	}

	// Process through layers if enabled (first message)
	let first_message_processed = !chat_session.session.messages.is_empty();
	log_debug!(
		"Processing input through layers: first_message={}",
		!first_message_processed
	);

	let (processed_input, layers_modified_session, _layer_cancelled) = process_layers_if_enabled(
		&input,
		&mut chat_session,
		&config_for_role,
		role,
		first_message_processed,
		operation_rx.clone(),
	)
	.await?;

	// Conversation compression: check if AI should compress older exchanges.
	// Runs BEFORE user message is added to avoid breaking the new request.
	let _compression_occurred =
		match crate::session::chat::conversation_compression::check_and_compress_conversation(
			&mut chat_session,
			&config_for_role,
			operation_rx.clone(),
			crate::session::chat::conversation_compression::CompressionTrigger::Automatic,
		)
		.await
		{
			Ok(compressed) => compressed,
			Err(e) => {
				log_debug!(
					"Conversation compression failed: {}. Continuing session.",
					e
				);
				false
			}
		};

	// Add user message if layers didn't modify session
	if !layers_modified_session {
		let final_input_with_constraints =
			crate::session::chat::session::utils::append_constraints_if_exists(
				&processed_input,
				&config_for_role.custom_constraints_file_name,
				&current_dir,
			);
		chat_session.add_user_message(&final_input_with_constraints)?;
	}

	// Prepare for API call
	prepare_for_api_call(&mut chat_session, &config_for_role, operation_rx.clone()).await?;

	// Create channel for WebSocket sink to stream messages
	let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel();
	let ws_sink = WebSocketSink::new(ws_tx.clone());

	// Forward MCP server notifications through the WebSocket channel
	crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_tx);

	// Execute API call — events stream in real-time via WebSocketSink
	let api_result = execute_api_call_and_process_response(
		&mut chat_session,
		&config_for_role,
		role,
		operation_rx.clone(),
		OutputMode::WebSocket,
		ws_sink,
	)
	.await;

	// Drain any remaining queued stream messages after API completion
	while let Ok(msg) = ws_rx.try_recv() {
		send_message(ws_sender, &msg).await?;
	}

	match api_result {
		Ok(_) => {
			// Cost message (events already emitted via sink — no reconstruction needed)
			let total_tokens = chat_session.session.info.input_tokens
				+ chat_session.session.info.output_tokens
				+ chat_session.session.info.cache_read_tokens
				+ chat_session.session.info.cache_write_tokens
				+ chat_session.session.info.reasoning_tokens;
			let cost_msg = ServerMessage::Cost(CostPayload {
				session_tokens: total_tokens,
				session_cost: chat_session.session.info.total_cost,
				input_tokens: chat_session.session.info.input_tokens,
				output_tokens: chat_session.session.info.output_tokens,
				cache_read_tokens: chat_session.session.info.cache_read_tokens,
				cache_write_tokens: chat_session.session.info.cache_write_tokens,
				reasoning_tokens: chat_session.session.info.reasoning_tokens,
				session_id: session_id.clone(),
			});
			send_message(ws_sender, &cost_msg).await?;
		}
		Err(e) => {
			log_error!("API call failed: {}", e);
			let error = ServerMessage::error(format!("Error: {}", e));
			send_message(ws_sender, &error).await?;
		}
	}

	// Save session
	log_debug!("Saving session: {}", session_id);
	chat_session.save()?;

	// Clear the notification sender now that this request is done
	crate::mcp::process::clear_notification_sender(Some(session_id.clone()));

	// Store session back and wake inbox monitor if it has pending messages.
	sessions
		.lock()
		.await
		.insert(session_id.clone(), chat_session);
	if let Some(notify) = crate::session::inbox::get_inbox_notify() {
		notify.notify_one();
	}
	log_debug!("Session stored back in memory: {}", session_id);

	Ok(())
}

/// Send a server message through WebSocket
async fn send_message(
	ws_sender: &mut futures_util::stream::SplitSink<
		WebSocketStream<TcpStream>,
		tokio_tungstenite::tungstenite::Message,
	>,
	msg: &ServerMessage,
) -> Result<()> {
	let json = serde_json::to_string(msg)?;
	log_debug!(
		"Sending message: type={:?}, size={} bytes",
		std::mem::discriminant(msg),
		json.len()
	);
	ws_sender.send(Message::text(json)).await?;
	Ok(())
}
