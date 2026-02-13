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

// WebSocket server implementation

use super::protocol::{ClientMessage, MessageType, ServerMessage};
use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::chat::session::{
	execute_api_call_and_process_response, prepare_for_api_call, process_layers_if_enabled,
	setup_and_initialize_session, setup_system_prompt_and_cache, ChatSession,
};
use crate::{log_debug, log_error, log_info};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

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

		loop {
			match listener.accept().await {
				Ok((stream, peer_addr)) => {
					log_info!("Connection accepted from {}", peer_addr);

					// Handle connection directly (sequential for simplicity)
					// This avoids Send/Sync issues and is fine for initial implementation
					if let Err(e) = handle_connection(
						stream,
						peer_addr,
						Arc::clone(&self.config),
						self.role.clone(),
						Arc::clone(&sessions),
					)
					.await
					{
						log_error!("Connection handler failed for {}: {}", peer_addr, e);
					}
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
) -> Result<()> {
	// Accept WebSocket connection with compression enabled
	let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
		.max_message_size(Some(10 * 1024 * 1024)) // 10MB max message size
		.max_frame_size(Some(10 * 1024 * 1024)) // 10MB max frame size
		.accept_unmasked_frames(false);

	let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(ws_config)).await?;
	log_info!("WebSocket handshake completed for {}", peer_addr);

	let (mut ws_sender, mut ws_receiver) = ws_stream.split();

	// Send welcome message
	let welcome = ServerMessage::status(
		format!("Connected to Octomind WebSocket server (role: {})", role),
		None,
	);
	send_message(&mut ws_sender, &welcome).await?;

	// Process messages sequentially (like terminal)
	while let Some(msg) = ws_receiver.next().await {
		match msg {
			Ok(Message::Text(text)) => {
				log_debug!("Received message from {}: {} bytes", peer_addr, text.len());

				// Parse client message
				let client_msg = match serde_json::from_str::<ClientMessage>(&text) {
					Ok(msg) => {
						log_debug!(
							"Parsed message: session_id={:?}, content_len={}",
							msg.session_id,
							msg.content.len()
						);
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
				if let Err(e) =
					process_client_message(client_msg, &mut ws_sender, &config, &role, &sessions)
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

	log_info!("Connection closed: {}", peer_addr);
	Ok(())
}

/// Process a client message and send responses
async fn process_client_message(
	client_msg: ClientMessage,
	ws_sender: &mut futures_util::stream::SplitSink<
		WebSocketStream<TcpStream>,
		tokio_tungstenite::tungstenite::Message,
	>,
	config: &Config,
	role: &str,
	sessions: &Arc<Mutex<HashMap<String, ChatSession>>>,
) -> Result<()> {
	log_debug!(
		"Processing message: session_id={:?}, content_len={}",
		client_msg.session_id,
		client_msg.content.len()
	);

	// Get or create session
	let mut sessions_lock = sessions.lock().await;

	let (mut chat_session, session_id, is_new_session) = if let Some(session_id) =
		&client_msg.session_id
	{
		// Try to resume existing session from memory first
		if let Some(session) = sessions_lock.remove(session_id) {
			drop(sessions_lock); // CRITICAL: Release lock immediately after removing session
			log_debug!("Resumed session from memory: {}", session_id);
			(session, session_id.clone(), false)
		} else {
			// Try to load from disk
			drop(sessions_lock);
			log_debug!("Loading session from disk: {}", session_id);

			// Use the same session loading logic as CLI
			#[derive(Debug)]
			#[allow(dead_code)]
			struct ResumeArgs {
				name: Option<String>,
				resume: Option<String>,
				resume_recent: bool,
				model: Option<String>,
				max_tokens: Option<u32>,
				temperature: Option<f32>,
				role: String,
				max_retries: Option<u32>,
			}

			let args = ResumeArgs {
				name: None,
				resume: Some(session_id.clone()),
				resume_recent: false,
				model: None,
				max_tokens: None,
				temperature: None,
				role: role.to_string(),
				max_retries: None,
			};

			match setup_and_initialize_session(&args, config).await {
				Ok((session, config_for_role, session_role, _first_message_processed)) => {
					// Setup system prompt and cache
					let mut session = session;
					setup_system_prompt_and_cache(
						&mut session,
						&config_for_role,
						&session_role,
						false,
					)
					.await?;

					log_info!("Session loaded from disk: {}", session_id);
					(session, session_id.clone(), false)
				}
				Err(_) => {
					// Session not found on disk either
					let error = ServerMessage::error(format!(
						"Session not found: {}. Please start a new session by omitting session_id.",
						session_id
					));
					send_message(ws_sender, &error).await?;
					return Ok(());
				}
			}
		}
	} else {
		// Create new session
		drop(sessions_lock); // Release lock before creating session
		log_debug!("Creating new session with role: {}", role);

		// Use the same session initialization as CLI
		// Create a dummy args struct for session initialization
		#[derive(Debug)]
		#[allow(dead_code)] // Fields are read via Debug formatting
		struct DummyArgs {
			name: Option<String>,
			resume: Option<String>,
			resume_recent: bool,
			model: Option<String>,
			max_tokens: Option<u32>,
			temperature: Option<f32>,
			role: String,
			max_retries: Option<u32>,
		}

		let args = DummyArgs {
			name: None,
			resume: None,
			resume_recent: false,
			model: None,
			max_tokens: None,
			temperature: None,
			role: role.to_string(),
			max_retries: None,
		};

		let (session, config_for_role, session_role, _first_message_processed) =
			setup_and_initialize_session(&args, config).await?;

		// Setup system prompt and cache (non-interactive mode)
		let mut session = session;
		setup_system_prompt_and_cache(&mut session, &config_for_role, &session_role, false).await?;

		let session_id = session.session.info.name.clone();
		log_info!("Session created: {}", session_id);

		(session, session_id, true)
	};

	// Lock is already released in both branches above
	// Send session info if new
	if is_new_session {
		let status = ServerMessage::status(
			format!("Session created: {}", session_id),
			Some(session_id.clone()),
		);
		send_message(ws_sender, &status).await?;
	}

	// Get current directory for file operations
	let current_dir = std::env::current_dir()?;

	// Check if this is a command
	let input = client_msg.content.clone();
	if input.starts_with('/') {
		log_debug!(
			"Processing command: {}",
			input.split_whitespace().next().unwrap_or(&input)
		);
		// Handle commands
		let config_for_role = config.get_merged_config_for_role(role);
		let mut cancellation = SessionCancellation::new();
		let operation_rx = cancellation.new_operation();

		let command_result = chat_session
			.process_command(&input, &mut config_for_role.clone(), role, operation_rx)
			.await?;

		use crate::session::chat::session::commands::CommandResult;
		match command_result {
			CommandResult::Handled => {
				log_debug!("Command executed successfully");
				// Command was handled, send status
				let status = ServerMessage::status(
					"Command executed successfully".to_string(),
					Some(session_id.clone()),
				);
				send_message(ws_sender, &status).await?;

				// Save session
				chat_session.save()?;

				// Store session back
				sessions
					.lock()
					.await
					.insert(session_id.clone(), chat_session);
				return Ok(());
			}
			CommandResult::HandledWithOutput(command_output) => {
				log_debug!("Command executed with structured output");
				// Command was handled with structured JSON output, send it to client
				let response = ServerMessage::with_metadata(
					MessageType::Status,
					"Command executed successfully".to_string(),
					command_output.to_json(),
					Some(session_id.clone()),
				);
				send_message(ws_sender, &response).await?;

				// Save session
				chat_session.save()?;

				// Store session back
				sessions
					.lock()
					.await
					.insert(session_id.clone(), chat_session);
				return Ok(());
			}

			CommandResult::Exit => {
				log_info!("Session ended by user command");
				// Exit command - close session
				let status =
					ServerMessage::status("Session ended".to_string(), Some(session_id.clone()));
				send_message(ws_sender, &status).await?;
				return Ok(());
			}
			CommandResult::TreatAsUserInput => {
				// Fall through to normal processing
			}
		}
	}

	// Process through layers if enabled (first message)
	let config_for_role = config.get_merged_config_for_role(role);
	let mut cancellation = SessionCancellation::new();
	let operation_rx = cancellation.new_operation();

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

	// Track message count before API call to identify new messages
	let messages_before = chat_session.session.messages.len();
	log_debug!("Executing API call: messages_before={}", messages_before);

	// Execute API call and process response
	match execute_api_call_and_process_response(
		&mut chat_session,
		&config_for_role,
		role,
		operation_rx.clone(),
		false, // is_interactive = false (like run command)
		None,  // output_callback - WebSocket has its own message handling
	)
	.await
	{
		Ok(_) => {
			let current_message_count = chat_session.session.messages.len();
			let new_message_count = current_message_count.saturating_sub(messages_before);
			log_debug!("API call completed: new_messages={}", new_message_count);

			// Success - extract and send all new messages (tools + assistant response)
			// Only if messages were actually added
			if current_message_count > messages_before {
				let new_messages = &chat_session.session.messages[messages_before..];

				for msg in new_messages {
					match msg.role.as_str() {
						"tool" => {
							// Parse tool message content (stored as MCP JSON format)
							let content = &msg.content;

							// Extract actual text from MCP format
							let actual_content = if let Ok(mcp_result) =
								serde_json::from_str::<serde_json::Value>(content)
							{
								crate::mcp::extract_mcp_content(&mcp_result)
							} else {
								// Fallback to raw content if not valid JSON
								content.clone()
							};

							// Extract tool name from the message's name field (most reliable)
							let tool_name = msg
								.name
								.as_ref()
								.map(|n| n.to_string())
								.unwrap_or_else(|| "unknown".to_string());

							// Extract tool_call_id from the message
							let tool_id = msg
								.tool_call_id
								.as_ref()
								.map(|id| id.to_string())
								.unwrap_or_else(|| "unknown".to_string());

							// Get server name for this tool
							let server_name =
								crate::session::chat::response::get_tool_server_name_async(
									&tool_name, config,
								)
								.await;

							// Check if tool execution was successful (no isError: true in MCP format)
							let success = if let Ok(mcp_result) =
								serde_json::from_str::<serde_json::Value>(content)
							{
								!mcp_result
									.get("isError")
									.and_then(|v| v.as_bool())
									.unwrap_or(false)
							} else {
								true // Assume success if not MCP format
							};

							// Send tool result with clean content
							let tool_msg = ServerMessage::with_metadata(
								MessageType::ToolResult,
								actual_content,
								json!({
									"tool": tool_name,
									"tool_id": tool_id,
									"server": server_name,
									"success": success,
									"duration_ms": 0, // We don't track this currently
								}),
								Some(session_id.clone()),
							);
							send_message(ws_sender, &tool_msg).await?;
						}

						"assistant" => {
							// Check if this assistant message has tool calls
							if let Some(tool_calls_value) = &msg.tool_calls {
								// This is a tool use intent - send tool_use messages
								if let Some(tool_calls_array) = tool_calls_value.as_array() {
									for tool_call in tool_calls_array {
										// Try different formats: OpenAI format, direct format, etc.
										let tool_name = tool_call
											.get("function")
											.and_then(|f| f.get("name"))
											.and_then(|n| n.as_str())
											.or_else(|| {
												tool_call.get("name").and_then(|n| n.as_str())
											})
											.or_else(|| {
												tool_call.get("tool_name").and_then(|n| n.as_str())
											})
											.unwrap_or("unknown");

										// Extract tool_id from tool_call
										let tool_id = tool_call
											.get("id")
											.and_then(|id| id.as_str())
											.unwrap_or("unknown");

										// Get server name for this tool
										let server_name =
										crate::session::chat::response::get_tool_server_name_async(
											tool_name, config,
										)
										.await;

										let tool_params = tool_call
											.get("function")
											.and_then(|f| f.get("arguments"))
											.and_then(|a| {
												// Arguments might be a string or already parsed
												if let Some(s) = a.as_str() {
													serde_json::from_str::<serde_json::Value>(s)
														.ok()
												} else {
													Some(a.clone())
												}
											})
											.or_else(|| tool_call.get("arguments").cloned())
											.or_else(|| tool_call.get("parameters").cloned())
											.unwrap_or_else(|| json!({}));

										// Send tool_use message
										let tool_use_msg = ServerMessage::with_metadata(
											MessageType::ToolUse,
											format!("Executing: {}(...)", tool_name),
											json!({
												"tool": tool_name,
												"tool_id": tool_id,
												"server": server_name,
												"params": tool_params,
											}),
											Some(session_id.clone()),
										);
										send_message(ws_sender, &tool_use_msg).await?;
									}
								}

								// Skip sending the empty assistant message
								continue;
							}

							// Skip empty assistant messages (happens when AI only makes tool calls)
							if msg.content.trim().is_empty() {
								continue;
							}

							// Send assistant response
							let assistant_msg = ServerMessage::new(
								MessageType::Assistant,
								msg.content.clone(),
								Some(session_id.clone()),
							);
							send_message(ws_sender, &assistant_msg).await?;
						}

						_ => {
							// Ignore other message types (user, system)
						}
					}
				}
			} // End of if current_message_count > messages_before

			// Send cost information

			let total_tokens = chat_session.session.info.input_tokens
				+ chat_session.session.info.output_tokens
				+ chat_session.session.info.cache_read_tokens
				+ chat_session.session.info.cache_write_tokens
				+ chat_session.session.info.reasoning_tokens;

			log_debug!(
				"Session stats: tokens={}, cost=${:.4}",
				total_tokens,
				chat_session.session.info.total_cost
			);

			let cost_msg = ServerMessage::with_metadata(
				MessageType::Cost,
				format!(
					"Session: {} tokens (${:.4})",
					total_tokens, chat_session.session.info.total_cost
				),
				json!({
					"session_tokens": total_tokens,
					"session_cost": chat_session.session.info.total_cost,
					"input_tokens": chat_session.session.info.input_tokens,
					"output_tokens": chat_session.session.info.output_tokens,
					"cache_read_tokens": chat_session.session.info.cache_read_tokens,
					"cache_write_tokens": chat_session.session.info.cache_write_tokens,
					"reasoning_tokens": chat_session.session.info.reasoning_tokens,
				}),
				Some(session_id.clone()),
			);

			send_message(ws_sender, &cost_msg).await?;
		}
		Err(e) => {
			log_error!("API call failed: {}", e);
			// Error during processing
			let error = ServerMessage::error(format!("Error: {}", e));
			send_message(ws_sender, &error).await?;
		}
	}

	// Save session
	log_debug!("Saving session: {}", session_id);
	chat_session.save()?;

	// Store session back
	sessions
		.lock()
		.await
		.insert(session_id.clone(), chat_session);
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
		msg.message_type,
		json.len()
	);
	ws_sender.send(Message::text(json)).await?;
	Ok(())
}
