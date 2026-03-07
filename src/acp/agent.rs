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

//! OctomindAgent — implements the ACP Agent trait over Octomind's session infrastructure.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;


use agent_client_protocol::{
	AgentCapabilities, AgentSideConnection, AuthenticateRequest, AuthenticateResponse,
	CancelNotification, Client, ContentBlock, ContentChunk, Implementation, InitializeRequest,
	InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
	ProtocolVersion, SessionNotification, SessionUpdate, StopReason, ToolCall, ToolCallUpdate,
	ToolCallUpdateFields,
};

use crate::config::Config;
use crate::session::cancellation::SessionCancellation;
use crate::session::chat::session::{
	execute_api_call_and_process_response, prepare_for_api_call, process_layers_if_enabled,
	setup_and_initialize_session, setup_system_prompt_and_cache, ChatSession, GenericSessionArgs,
};
use crate::session::output::{OutputMode, WebSocketSink};
use crate::websocket::ServerMessage;
use crate::{log_debug, log_error};

/// ACP agent implementation wrapping Octomind's session infrastructure.
///
/// Single-threaded (runs inside a `tokio::task::LocalSet`), so `Rc<RefCell<...>>` is safe.
pub struct OctomindAgent {
	config: Config,
	role: String,
	/// Active sessions keyed by ACP session_id
	sessions: Rc<RefCell<HashMap<String, ChatSession>>>,
	/// Active cancellation handles keyed by ACP session_id
	cancellations: Rc<RefCell<HashMap<String, SessionCancellation>>>,
	/// Connection back to the client — used to send session/update notifications
	conn: Rc<RefCell<Option<Rc<AgentSideConnection>>>>,
}

impl OctomindAgent {
	pub fn new(config: Config, role: String) -> Self {
		Self {
			config,
			role,
			sessions: Rc::new(RefCell::new(HashMap::new())),
			cancellations: Rc::new(RefCell::new(HashMap::new())),
			conn: Rc::new(RefCell::new(None)),
		}
	}

	/// Inject the connection after it's created (chicken-and-egg: agent needs conn, conn needs agent).
	pub fn set_connection(&self, conn: Rc<AgentSideConnection>) {
		*self.conn.borrow_mut() = Some(conn);
	}

}

#[async_trait::async_trait(?Send)]
impl agent_client_protocol::Agent for OctomindAgent {
	async fn initialize(
		&self,
		args: InitializeRequest,
	) -> agent_client_protocol::Result<InitializeResponse> {
		log_debug!("ACP: initialize from {:?}", args.client_info);
		let response = InitializeResponse::new(ProtocolVersion::LATEST)
			.agent_capabilities(AgentCapabilities::default())
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
		// Change working directory to the client's cwd
		if let Err(e) = std::env::set_current_dir(&args.cwd) {
			log_error!("ACP: failed to set cwd to {:?}: {}", args.cwd, e);
		}

		let session_args = GenericSessionArgs::new(self.role.clone());
		let (mut chat_session, config_for_role, session_role, _) =
			setup_and_initialize_session(&session_args, &self.config)
				.await
				.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &session_role, false)
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		let session_id = chat_session.session.info.name.clone();
		log_debug!("ACP: new_session created: {}", session_id);

		self.sessions
			.borrow_mut()
			.insert(session_id.clone(), chat_session);
		self.cancellations
			.borrow_mut()
			.insert(session_id.clone(), SessionCancellation::new());

		Ok(NewSessionResponse::new(session_id))
	}

	async fn prompt(&self, args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
		let session_id = args.session_id.to_string();

		// Extract text from prompt content blocks
		let input: String = args
			.prompt
			.iter()
			.filter_map(|block| {
				if let ContentBlock::Text(t) = block {
					Some(t.text.as_str())
				} else {
					None
				}
			})
			.collect::<Vec<_>>()
			.join("\n");

		if input.trim().is_empty() {
			return Ok(PromptResponse::new(StopReason::EndTurn));
		}

		// Take session out of map for exclusive access
		let mut chat_session = match self.sessions.borrow_mut().remove(&session_id) {
			Some(s) => s,
			None => {
				return Err(agent_client_protocol::Error::invalid_params()
					.data(format!("session not found: {session_id}")));
			}
		};

		let config_for_role = self.config.get_merged_config_for_role(&self.role);
		let current_dir = std::env::current_dir().unwrap_or_default();

		// Get or create cancellation for this session
		let mut cancellation = self
			.cancellations
			.borrow_mut()
			.remove(&session_id)
			.unwrap_or_default();
		cancellation.reset();
		let operation_rx = cancellation.new_operation();

		// Process through layers (pre-processing step)
		let first_message_processed = !chat_session.session.messages.is_empty();
		let (processed_input, layers_modified_session, layer_cancelled) =
			process_layers_if_enabled(
				&input,
				&mut chat_session,
				&config_for_role,
				&self.role,
				first_message_processed,
				operation_rx.clone(),
			)
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		if layer_cancelled {
			self.sessions
				.borrow_mut()
				.insert(session_id.clone(), chat_session);
			self.cancellations
				.borrow_mut()
				.insert(session_id.clone(), cancellation);
			return Ok(PromptResponse::new(StopReason::Cancelled));
		}

		// Add user message if layers didn't modify session
		if !layers_modified_session {
			let final_input = crate::session::chat::session::utils::append_constraints_if_exists(
				&processed_input,
				&config_for_role.custom_constraints_file_name,
				&current_dir,
			);
			chat_session
				.add_user_message(&final_input)
				.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;
		}

		// Prepare for API call
		prepare_for_api_call(&mut chat_session, &config_for_role, operation_rx.clone())
			.await
			.map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()))?;

		// Channel-based sink: session pipeline emits ServerMessages, we forward them as ACP notifications
		let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
		let ws_sink = WebSocketSink::new(ws_tx.clone());

		// Forward MCP server notifications through the same channel
		crate::mcp::process::set_notification_sender(ws_tx);

		// Spawn a local task to stream notifications to the client in real-time
		// while the API call runs concurrently. The channel closes when ws_sink drops.
		let session_id_for_task = session_id.clone();
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
						let tool_call =
							ToolCall::new(p.tool_id.clone(), p.tool.clone()).raw_input(p.params.clone());
						Some(SessionUpdate::ToolCall(tool_call))
					}
					ServerMessage::ToolResult(p) => {
						let update = ToolCallUpdate::new(
							p.tool_id.clone(),
							ToolCallUpdateFields::new()
								.raw_output(serde_json::Value::String(p.content)),
						);
						Some(SessionUpdate::ToolCallUpdate(update))
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
			OutputMode::WebSocket,
			ws_sink,
		)
		.await;

		// Wait for the forwarding task to drain any remaining messages
		let _ = forward_task.await;

		// Put session back
		self.sessions
			.borrow_mut()
			.insert(session_id.clone(), chat_session);
		self.cancellations
			.borrow_mut()
			.insert(session_id.clone(), cancellation);

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
	}

	async fn cancel(&self, args: CancelNotification) -> agent_client_protocol::Result<()> {
		let session_id = args.session_id.to_string();
		log_debug!("ACP: cancel requested for session: {}", session_id);
		if let Some(cancellation) = self.cancellations.borrow().get(&session_id) {
			cancellation.shutdown();
		}
		Ok(())
	}
}
