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
	AckPayload, Attachment, AttachmentKind, CancelMessage, ClientMessage, CloseMessage, CommandMessage,
	ElicitationResponseMessage, ServerMessage, SessionMessage, StatusPayload, UserMessage, ELICITATION_CAPABILITY,
};
use super::stream::{SessionStream, Streams};
use crate::config::Config;
use crate::session::cancellation::{CancelHandle, SessionCancellation};
use crate::session::chat::session::{
	execute_api_call_and_process_response, prepare_for_api_call, run_pipe_if_enabled, setup_and_initialize_session,
	setup_system_prompt_and_cache, ChatSession, GenericSessionArgs,
};
use crate::session::output::{OutputMode, WebSocketSink};
use crate::{log_debug, log_error, log_info};
use anyhow::Result;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::handshake::server::{Callback, ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{
	header::{AUTHORIZATION, ORIGIN},
	StatusCode,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;

/// Per-session processing locks to prevent concurrent access to the same session.
type SessionLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

/// Configs committed by a `/role` switch or restored from the session log on
/// load, keyed by session id. A tap role's servers exist only in the config its
/// switch resolved.
type RoleConfigs = Arc<Mutex<HashMap<String, Arc<Config>>>>;

/// Cancel triggers of the turns running right now, keyed by session id.
type ActiveTurns = Arc<std::sync::Mutex<HashMap<String, CancelHandle>>>;

/// Registers a running turn's cancel trigger for the turn's lifetime, so a
/// `cancel` frame from any connection reaches it, and deregisters it however
/// the turn ends. Turns of one session never overlap (the session lock), but
/// the removal still checks identity so a late drop never unhooks a newer turn.
struct TurnGuard {
	turns: ActiveTurns,
	session_id: String,
	handle: CancelHandle,
}

impl TurnGuard {
	fn register(turns: &ActiveTurns, session_id: &str, cancellation: &SessionCancellation) -> Self {
		let handle = cancellation.handle();
		turns.lock().unwrap().insert(session_id.to_string(), handle.clone());
		Self {
			turns: Arc::clone(turns),
			session_id: session_id.to_string(),
			handle,
		}
	}
}

impl Drop for TurnGuard {
	fn drop(&mut self) {
		let mut turns = self.turns.lock().unwrap();
		if turns
			.get(&self.session_id)
			.is_some_and(|current| current.same_as(&self.handle))
		{
			turns.remove(&self.session_id);
		}
	}
}

/// Server-wide state every connection shares, cloned into each connection as one
/// value instead of a parameter per field.
#[derive(Clone)]
struct ServerState {
	config: Arc<Config>,
	role: String,
	/// Active sessions (session_id -> ChatSession).
	sessions: Arc<Mutex<HashMap<String, ChatSession>>>,
	/// Held for a message's entire processing, so different connections never
	/// drive the same session concurrently.
	session_locks: SessionLocks,
	role_configs: RoleConfigs,
	turns: ActiveTurns,
	/// Output streams of the loaded sessions: what each session sends, and to
	/// which connection.
	streams: Streams,
	/// Browser origins permitted to open a connection. Empty = refuse every
	/// handshake that carries an `Origin` header.
	allow_origins: Arc<Vec<String>>,
	/// Shared secret every handshake must present as `Authorization: Bearer`.
	/// None = no authentication (the loopback-only default).
	token: Option<Arc<str>>,
}

impl ServerState {
	fn new(config: Arc<Config>, role: String, allow_origins: Vec<String>) -> Self {
		Self {
			config,
			role,
			sessions: Arc::default(),
			session_locks: Arc::default(),
			role_configs: Arc::default(),
			turns: Arc::default(),
			streams: Streams::default(),
			allow_origins: Arc::new(allow_origins),
			token: None,
		}
	}
}

/// Per-connection context passed to the message handlers.
#[derive(Clone)]
struct ConnCtx {
	server: ServerState,
	/// Cancelled once the client is gone. The connection's inbox monitors stop
	/// on it; a turn the client left keeps running.
	closed: CancellationToken,
}

impl ConnCtx {
	/// The client is gone: this connection's monitors stop, and no session
	/// sends to it any more — its frames are held for the next bind.
	fn hang_up(&self, ws_sender: &WsSender) {
		self.closed.cancel();
		self.server.streams.detach(ws_sender);
	}
}

/// Hangs a connection up however serving it ends, an error return included.
struct HangUpOnExit<'a> {
	ctx: &'a ConnCtx,
	ws_sender: &'a WsSender,
}

impl Drop for HangUpOnExit<'_> {
	fn drop(&mut self) {
		self.ctx.hang_up(self.ws_sender);
	}
}

/// WebSocket server for handling AI sessions
pub struct WebSocketServer {
	addr: SocketAddr,
	state: ServerState,
}

impl WebSocketServer {
	/// Create a new WebSocket server
	pub fn new(host: &str, port: u16, config: Config, role: String, allow_origins: Vec<String>) -> Result<Self> {
		let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
		Ok(Self {
			addr,
			state: ServerState::new(Arc::new(config), role, allow_origins),
		})
	}

	/// Require every handshake to carry `Authorization: Bearer <token>`.
	/// `None` (or an empty token) keeps the server unauthenticated.
	pub fn with_token(mut self, token: Option<String>) -> Self {
		self.state.token = token.filter(|t| !t.is_empty()).map(Arc::from);
		self
	}

	/// Start the WebSocket server
	pub async fn start(&self) -> Result<()> {
		let listener = TcpListener::bind(&self.addr).await?;
		// The bound address, not the requested one: `--port 0` picks a free port,
		// and an embedder that spawned us reads it from this line.
		let addr = listener.local_addr()?;
		log_info!("WebSocket server listening on ws://{}", addr);
		println!("🚀 WebSocket server started on ws://{}", addr);
		if self.state.token.is_some() {
			println!("Connections must authenticate (OCTOMIND_SERVER_TOKEN is set)");
		}
		if self.state.allow_origins.is_empty() {
			println!("Browser connections refused (no --allow-origin configured)");
		} else {
			println!("Allowed browser origins: {}", self.state.allow_origins.join(", "));
		}
		println!("Press Ctrl+C to stop the server");

		loop {
			match listener.accept().await {
				Ok((stream, peer_addr)) => {
					log_info!("Connection accepted from {}", peer_addr);

					let state = self.state.clone();
					tokio::spawn(async move {
						if let Err(e) = handle_connection(stream, peer_addr, state).await {
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

/// Vets a handshake before the welcome frame: the `Origin` allowlist, then the
/// shared-secret token when one is configured.
///
/// WebSocket upgrades are covered by neither CORS nor the same-origin policy, so
/// without the origin check any page the operator visits can open a socket to a
/// loopback-bound server and drive the agent with full tool access. Native
/// clients send no `Origin` header and are unaffected; browsers always send one,
/// so a present-but-unlisted origin is refused. The token closes the remaining
/// gap — any other local process — for embedders that spawn the server.
///
/// A named callback type rather than a closure: tungstenite's handshake contract
/// fixes the error type, and carrying it through a signature of our own is what
/// makes an oversized `Err` variant our problem instead of the library's.
struct HandshakeGuard {
	origins: Arc<Vec<String>>,
	token: Option<Arc<str>>,
}

impl Callback for HandshakeGuard {
	fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
		// Absent header — not a browser.
		if let Some(value) = request.headers().get(ORIGIN) {
			let origin = value.to_str().unwrap_or_default();
			if !self.origins.iter().any(|allowed| allowed == origin) {
				log_error!("Rejected WebSocket handshake from origin '{}'", origin);
				return Err(refusal(
					StatusCode::FORBIDDEN,
					"Origin not allowed. Start the server with --allow-origin <ORIGIN> to permit it.",
				));
			}
		}

		if let Some(token) = &self.token {
			let presented = request
				.headers()
				.get(AUTHORIZATION)
				.and_then(|value| value.to_str().ok())
				.and_then(|value| value.strip_prefix("Bearer "))
				.unwrap_or_default();
			if !constant_time_eq(presented.as_bytes(), token.as_bytes()) {
				log_error!("Rejected WebSocket handshake without a valid token");
				return Err(refusal(
					StatusCode::UNAUTHORIZED,
					"Missing or invalid token. Send `Authorization: Bearer <OCTOMIND_SERVER_TOKEN>`.",
				));
			}
		}

		Ok(response)
	}
}

fn refusal(status: StatusCode, body: &str) -> ErrorResponse {
	Response::builder()
		.status(status)
		.body(Some(body.to_string()))
		.expect("static refusal response is always valid")
}

/// Compares without an early exit, so response timing does not reveal how much
/// of a guessed token matched.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
	a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

type WsReader = SplitStream<WebSocketStream<TcpStream>>;

/// Client-bound half of a connection. A writer task owns the socket sink, so
/// the frame being processed and the reader — which answers `cancel` frames and
/// pings while that frame runs — both send without sharing it. Frames keep their
/// order; a send fails once the socket is gone, as a direct write would.
#[derive(Clone)]
pub(crate) struct WsSender(tokio::sync::mpsc::UnboundedSender<Message>);

impl WsSender {
	fn spawn(
		mut sink: futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>,
	) -> (Self, tokio::task::JoinHandle<()>) {
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
		let writer = tokio::spawn(async move {
			while let Some(frame) = rx.recv().await {
				if let Err(e) = sink.send(frame).await {
					log_debug!("WebSocket write failed, closing the writer: {}", e);
					return;
				}
			}
			let _ = sink.close().await;
		});
		(Self(tx), writer)
	}

	pub(crate) fn same_channel(&self, other: &Self) -> bool {
		self.0.same_channel(&other.0)
	}

	pub(crate) fn send(&self, frame: Message) -> Result<()> {
		self.0
			.send(frame)
			.map_err(|_| anyhow::anyhow!("WebSocket connection closed"))
	}
}

#[cfg(test)]
impl WsSender {
	/// A sender whose frames land in the returned receiver instead of a socket.
	pub(crate) fn channel() -> (Self, tokio::sync::mpsc::UnboundedReceiver<Message>) {
		let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
		(Self(tx), rx)
	}
}

struct RunSubscriptions(WsSender);

impl Drop for RunSubscriptions {
	fn drop(&mut self) {
		if let Err(error) = crate::session::tap_runs::unbind_run_events(&self.0) {
			crate::log_error!("Failed to remove WebSocket run subscriptions: {}", error);
		}
	}
}

/// Handle a single WebSocket connection
async fn handle_connection(stream: TcpStream, peer_addr: SocketAddr, server: ServerState) -> Result<()> {
	// Accept WebSocket connection with compression enabled
	let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
		.max_message_size(Some(10 * 1024 * 1024)) // 10MB max message size
		.max_frame_size(Some(10 * 1024 * 1024)) // 10MB max frame size
		.accept_unmasked_frames(false);

	let ws_stream = tokio_tungstenite::accept_hdr_async_with_config(
		stream,
		HandshakeGuard {
			origins: Arc::clone(&server.allow_origins),
			token: server.token.clone(),
		},
		Some(ws_config),
	)
	.await?;
	log_info!("WebSocket handshake completed for {}", peer_addr);

	let (sink, mut ws_receiver) = ws_stream.split();
	let (ws_sender, writer) = WsSender::spawn(sink);

	let result = serve_connection(&mut ws_receiver, &ws_sender, peer_addr, server).await;

	// Let the writer flush what is queued and close the socket.
	drop(ws_sender);
	let _ = tokio::time::timeout(std::time::Duration::from_secs(5), writer).await;
	result
}

async fn serve_connection(
	ws_receiver: &mut WsReader,
	ws_sender: &WsSender,
	peer_addr: SocketAddr,
	server: ServerState,
) -> Result<()> {
	let _run_subscriptions = RunSubscriptions(ws_sender.clone());
	// Track session IDs used on this connection for cleanup on disconnect
	let mut active_session_ids: HashSet<String> = HashSet::new();

	// Send welcome message
	let welcome = ServerMessage::status(
		format!("Connected to Octomind WebSocket server (role: {})", server.role),
		None,
	);
	send_message(ws_sender, &welcome).await?;

	let ctx = ConnCtx {
		server,
		closed: CancellationToken::new(),
	};
	let _hang_up = HangUpOnExit { ctx: &ctx, ws_sender };

	// Frames read while an earlier frame was still processing, in arrival order.
	let mut deferred: VecDeque<Message> = VecDeque::new();
	// The client side ended while a frame was processing.
	let mut stream_ended = false;

	loop {
		let msg = if let Some(frame) = deferred.pop_front() {
			Ok(frame)
		} else if stream_ended {
			break;
		} else {
			match ws_receiver.next().await {
				Some(msg) => msg,
				None => break, // stream ended
			}
		};

		match msg {
			Ok(Message::Text(text)) => {
				log_debug!("Received message from {}: {} bytes", peer_addr, text.len());
				let Some(client_msg) = accept_client_frame(&text, ws_sender, peer_addr).await? else {
					continue;
				};

				match &client_msg {
					ClientMessage::Cancel(cancel) => {
						cancel_turn(&ctx.server, cancel);
						continue;
					}
					ClientMessage::RunLog(request) => {
						handle_run_log(request.clone(), ws_sender).await?;
						continue;
					}
					ClientMessage::ElicitationResponse(response) => {
						handle_elicitation_response(response.clone(), ws_sender).await?;
						continue;
					}
					_ => {}
				}

				let request_id = client_msg.request_id().map(ToOwned::to_owned);
				// Process the message, still reading the socket meanwhile: `cancel`
				// frames act at once, pings are answered, everything else waits its turn.
				let processing = process_client_message(client_msg, ws_sender, &ctx, &mut active_session_ids);
				let result = read_while_processing(
					processing,
					ws_receiver,
					ws_sender,
					&ctx,
					&mut deferred,
					&mut stream_ended,
					peer_addr,
				)
				.await;
				if let Err(e) = result {
					log_error!("Message processing failed for {}: {}", peer_addr, e);
					let error = ServerMessage::error_for_request(format!("Internal error: {}", e), request_id);
					send_message(ws_sender, &error).await?;
				}
			}
			Ok(Message::Binary(_)) => {
				let error =
					ServerMessage::error("Unsupported WebSocket message type: send JSON text frames".to_string());
				send_message(ws_sender, &error).await?;
			}
			Ok(Message::Close(_)) => {
				log_info!("Client {} closed connection", peer_addr);
				break;
			}
			Ok(Message::Ping(data)) => {
				log_debug!("Ping received from {}", peer_addr);
				// Respond to ping with pong
				if let Err(e) = ws_sender.send(Message::Pong(data)) {
					log_error!("Failed to send pong to {}: {}", peer_addr, e);
					break;
				}
			}
			Ok(Message::Pong(_)) => {
				// Ignore pong replies; they are transport-level control frames.
			}
			Ok(_) => {}
			Err(e) => {
				log_error!("WebSocket protocol error from {}: {}", peer_addr, e);
				break;
			}
		}
	}

	// Clean up notification senders for sessions used on this connection.
	// Sessions themselves persist (can be resumed on another connection).
	//
	// INVARIANT: Do NOT call stop_all_servers() here. MCP server processes are shared
	// across all active sessions. Killing them on disconnect would break other sessions
	// that are still using the same servers — which is also why a role switch never
	// stops any. stop_all_servers() is only called on process shutdown (main.rs).
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

/// Parse, validate and acknowledge one client text frame. Invalid frames are
/// answered with an error here and yield `None`. A `session` bind is
/// acknowledged by its handler instead, once it knows the session's stream.
async fn accept_client_frame(text: &str, ws_sender: &WsSender, peer_addr: SocketAddr) -> Result<Option<ClientMessage>> {
	// Parse client message
	let client_msg = match serde_json::from_str::<ClientMessage>(text) {
		Ok(msg) => {
			log_debug!("Parsed message: {:?}", msg);
			msg
		}
		Err(e) => {
			log_error!("Invalid JSON from {}: {}", peer_addr, e);
			let error = ServerMessage::error(format!("Invalid JSON: {}", e));
			send_message(ws_sender, &error).await?;
			return Ok(None);
		}
	};

	// Validate message
	if let Err(e) = client_msg.validate() {
		log_error!("Message validation failed from {}: {}", peer_addr, e);
		let error = match client_msg {
			ClientMessage::RunLog(request) => ServerMessage::RunLog(super::protocol::RunLogPayload::error(request, e)),
			other => ServerMessage::error_for_request(e, other.request_id().map(ToOwned::to_owned)),
		};
		send_message(ws_sender, &error).await?;
		return Ok(None);
	}

	if !matches!(client_msg, ClientMessage::Session(_)) {
		send_message(ws_sender, &ServerMessage::ack(&client_msg)).await?;
	}
	Ok(Some(client_msg))
}

/// Drive one client frame to completion while still reading the socket.
///
/// `cancel`, `run_log` and `elicitation_response` must remain usable during a running turn. Pings are
/// answered so a keepalive never times out behind a long turn. Every other frame is deferred
/// and processed afterwards in arrival order, exactly as if it had been read then.
///
/// If the client closes or drops the connection while the frame runs a turn,
/// the turn keeps running: its frames are held in the session's stream and go
/// to the session's next bind, which can resume reading where this one left
/// off. Until the turn ends, the session's lock answers other frames "busy".
async fn read_while_processing(
	processing: impl std::future::Future<Output = Result<()>>,
	ws_receiver: &mut WsReader,
	ws_sender: &WsSender,
	ctx: &ConnCtx,
	deferred: &mut VecDeque<Message>,
	stream_ended: &mut bool,
	peer_addr: SocketAddr,
) -> Result<()> {
	let abandon = |stream_ended: &mut bool| {
		*stream_ended = true;
		log_info!("Client {} left while a frame was processing", peer_addr);
		ctx.hang_up(ws_sender);
	};
	tokio::pin!(processing);
	loop {
		tokio::select! {
			result = &mut processing => return result,
			frame = ws_receiver.next(), if !*stream_ended => match frame {
				Some(Ok(Message::Text(text))) => {
					let immediate = matches!(
						serde_json::from_str::<ClientMessage>(&text),
						Ok(ClientMessage::Cancel(_) | ClientMessage::RunLog(_) | ClientMessage::ElicitationResponse(_))
					);
					if !immediate {
						deferred.push_back(Message::Text(text));
					} else if let Some(message) = accept_client_frame(&text, ws_sender, peer_addr).await? {
						match message {
							ClientMessage::Cancel(cancel) => cancel_turn(&ctx.server, &cancel),
							ClientMessage::RunLog(request) => handle_run_log(request, ws_sender).await?,
							ClientMessage::ElicitationResponse(response) => {
								handle_elicitation_response(response, ws_sender).await?
							}
							_ => unreachable!("immediate control frame"),
						}
					}
				}
				Some(Ok(Message::Ping(data))) => {
					let _ = ws_sender.send(Message::Pong(data));
				}
				Some(Ok(Message::Pong(_))) => {}
				Some(Ok(Message::Close(_))) => abandon(stream_ended),
				Some(Ok(other)) => deferred.push_back(other),
				Some(Err(e)) => {
					log_error!("WebSocket protocol error from {}: {}", peer_addr, e);
					abandon(stream_ended);
				}
				None => abandon(stream_ended),
			},
		}
	}
}

async fn handle_run_log(request: super::protocol::RunLogMessage, sender: &WsSender) -> Result<()> {
	let reply = match super::run_events::replay(request.clone()).await {
		Ok(log) => ServerMessage::RunLog(log),
		Err(error) => ServerMessage::RunLog(super::protocol::RunLogPayload::error(request, error.to_string())),
	};
	send_message(sender, &reply).await
}

/// Hand the user's answer to the MCP call waiting on it. An id nobody waits on
/// (answered already, or its tool call ended) is the client's error to hear about.
async fn handle_elicitation_response(response: ElicitationResponseMessage, sender: &WsSender) -> Result<()> {
	if !crate::mcp::elicitation::resolve(&response.session_id, &response.id, response.result) {
		send_message(
			sender,
			&ServerMessage::elicitation_gone(&response.id, response.request_id),
		)
		.await?;
	}
	Ok(())
}

/// Stop the session's running turn, wherever it runs: on this connection, on
/// another one, or in the session's background inbox monitor.
fn cancel_turn(server: &ServerState, cancel: &CancelMessage) {
	let session_id = cancel.session_id.as_str();
	let running = server.turns.lock().unwrap().get(session_id).cloned();
	match running {
		Some(handle) => {
			log_info!("Cancelling the running turn of session {}", session_id);
			handle.cancel();
		}
		None => log_debug!("Cancel for session {} with no running turn", session_id),
	}
}

/// Process a client message and send responses
async fn process_client_message(
	client_msg: ClientMessage,
	ws_sender: &WsSender,
	ctx: &ConnCtx,
	active_session_ids: &mut HashSet<String>,
) -> Result<()> {
	match client_msg {
		ClientMessage::Session(msg) => handle_session_message(msg, ws_sender, ctx, active_session_ids).await,
		ClientMessage::Message(msg) => {
			let session_id = msg.session_id.clone();
			active_session_ids.insert(session_id.clone());

			// Acquire per-session lock to prevent concurrent access
			let lock = get_or_create_session_lock(&session_id, &ctx.server.session_locks).await;
			let guard = match lock.try_lock() {
				Ok(guard) => guard,
				Err(_) => {
					let error = ServerMessage::error_for_request(
						format!(
							"Session '{}' is busy processing another request. Please wait.",
							session_id
						),
						msg.request_id.clone(),
					);
					send_message(ws_sender, &error).await?;
					return Ok(());
				}
			};

			let result = crate::session::context::with_session_id(session_id.clone(), async {
				handle_user_message(msg, ws_sender, &ctx.server).await
			})
			.await;

			drop(guard);
			wake_inbox_monitor(session_id).await;
			result
		}
		ClientMessage::Command(msg) => {
			let session_id = msg.session_id.clone();
			active_session_ids.insert(session_id.clone());

			// Acquire per-session lock to prevent concurrent access
			let lock = get_or_create_session_lock(&session_id, &ctx.server.session_locks).await;
			let guard = match lock.try_lock() {
				Ok(guard) => guard,
				Err(_) => {
					let error = ServerMessage::error_for_request(
						format!(
							"Session '{}' is busy processing another request. Please wait.",
							session_id
						),
						msg.request_id.clone(),
					);
					send_message(ws_sender, &error).await?;
					return Ok(());
				}
			};

			let result = crate::session::context::with_session_id(session_id.clone(), async {
				handle_command_message(msg, ws_sender, &ctx.server).await
			})
			.await;

			drop(guard);
			wake_inbox_monitor(session_id).await;
			result
		}
		ClientMessage::RunLog(msg) => handle_run_log(msg, ws_sender).await,
		// Normally applied by the connection loop at once; this arm covers a frame deferred behind a turn.
		ClientMessage::ElicitationResponse(msg) => handle_elicitation_response(msg, ws_sender).await,
		ClientMessage::Close(msg) => handle_close_message(msg, ws_sender, &ctx.server, active_session_ids).await,
		// Applied by the connection loop the moment it is read, never queued here.
		ClientMessage::Cancel(cancel) => {
			cancel_turn(&ctx.server, &cancel);
			Ok(())
		}
	}
}

/// Handle a "close" type message: unload an idle session from memory.
///
/// The session's log stays on disk, so a later bind resumes it. Everything the
/// server holds for it goes — the in-memory session, its pinned role config and
/// every session-scoped registry, which also ends its background work (schedules,
/// monitors, delegated runs). Refused while a turn holds the session.
async fn handle_close_message(
	msg: CloseMessage,
	ws_sender: &WsSender,
	server: &ServerState,
	active_session_ids: &mut HashSet<String>,
) -> Result<()> {
	let session_id = msg.session_id;
	let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
	// The lock entry itself stays: removing it would let a frame that already
	// cloned it and one that creates a fresh entry run the session concurrently.
	let Ok(guard) = lock.try_lock() else {
		let error = ServerMessage::error_for_request(
			format!(
				"Session '{}' is busy processing another request. Please wait.",
				session_id
			),
			msg.request_id,
		);
		send_message(ws_sender, &error).await?;
		return Ok(());
	};

	let was_loaded = server.sessions.lock().await.remove(&session_id).is_some();
	server.role_configs.lock().await.remove(&session_id);
	// A later bind loads the session anew, under a new stream.
	server.streams.close(&session_id);
	let inbox_notify = crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::inbox::get_inbox_notify()
	})
	.await;
	crate::session::context::cleanup_session(&session_id);
	// Wake the session's inbox monitor so it sees the inbox gone and exits now
	// rather than when its connection closes.
	if let Some(notify) = inbox_notify {
		notify.notify_one();
	}
	active_session_ids.remove(&session_id);
	drop(guard);
	log_info!("Session closed: {} (was loaded: {})", session_id, was_loaded);

	send_message(
		ws_sender,
		&ServerMessage::command_status(
			format!("Session closed: {}", session_id),
			Some(session_id),
			serde_json::json!({ "command_type": "close", "was_loaded": was_loaded }),
		),
	)
	.await
}

/// Get or create a per-session processing lock.
async fn get_or_create_session_lock(session_id: &str, session_locks: &SessionLocks) -> Arc<tokio::sync::Mutex<()>> {
	let mut locks = session_locks.lock().await;
	locks
		.entry(session_id.to_string())
		.or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
		.clone()
}

/// Re-fire the session's inbox wake-up once a client frame has released the
/// session lock. A handback that lands while the frame runs wakes the monitor,
/// which fails `try_lock` and parks again; the turn's own wake-up
/// (`handle_user_message`) fires while that lock is still held. Without this the
/// result sits queued until something unrelated wakes the monitor — and a client
/// told `pending_work` waits out its cap for a turn that never starts.
async fn wake_inbox_monitor(session_id: String) {
	crate::session::context::with_session_id(session_id, async {
		if crate::session::inbox::has_inbox_messages() {
			if let Some(notify) = crate::session::inbox::get_inbox_notify() {
				notify.notify_one();
			}
		}
	})
	.await;
}

/// Spawn a background task that monitors schedules and inbox for a WebSocket session.
///
/// Runs independently of user prompts — when a scheduled entry fires or an inbox
/// message arrives, the task takes the session from the map, processes the message
/// through the full AI pipeline, emits the results on the session's stream (which
/// sends them to the session's latest bind), and puts the session back.
/// Inbox monitors currently alive. A monitor belongs to ONE connection, so this
/// must fall back to zero as connections close — a count that only grows is the
/// leak this exists to catch, and nothing else observes it from outside.
static LIVE_INBOX_MONITORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Decrements the live count however the monitor task ends, including a panic.
struct MonitorGuard;

impl Drop for MonitorGuard {
	fn drop(&mut self) {
		LIVE_INBOX_MONITORS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
	}
}

/// Exits when the session is removed from the map or its connection is gone.
fn spawn_ws_inbox_monitor(session_id: String, ctx: ConnCtx) {
	tokio::spawn(async move {
		LIVE_INBOX_MONITORS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		let _reaped = MonitorGuard;
		log_debug!("WebSocket: inbox monitor started for session: {}", session_id);
		loop {
			// This connection is gone. The exit check below runs only AFTER the
			// drain, so a handback that lands now (a tap run finishing long after
			// its turn closed the socket) would be popped off the queue and
			// answered with nobody bound to read it — the only copy of an answer
			// the user is still waiting for. Sessions and their inboxes
			// deliberately outlive a connection, so leave both alone; the next
			// connection spawns its own monitor and picks the message up.
			if ctx.closed.is_cancelled() {
				// Pass the wakeup on: a live connection's monitor can be parked on
				// this same Notify, and notify_one() only ever wakes one waiter.
				crate::session::context::with_session_id(session_id.clone(), async {
					if crate::session::inbox::has_inbox_messages() {
						if let Some(notify) = crate::session::inbox::get_inbox_notify() {
							notify.notify_one();
						}
					}
				})
				.await;
				break;
			}

			// Process phase: flush due schedules into inbox, then drain.
			// Returns true to exit the monitor loop.
			let should_exit = crate::session::context::with_session_id(session_id.clone(), async {
				crate::mcp::orchestration::flush_due_to_inbox();
				crate::mcp::orchestration::flush_idle_to_inbox();

				// Drain inbox while messages remain. Take the SAME per-session lock
				// process_client_message uses, held across take→process→reinsert:
				// otherwise a concurrent client message try_locks it (free, because
				// removing the session from the map is not the lock), misses memory,
				// and reloads a stale copy from disk — two live sessions racing the
				// same file. If the client holds the lock, stop this pass; it fires
				// inbox_notify on return and wakes us from the wait section.
				while crate::session::inbox::has_inbox_messages() {
					let lock = get_or_create_session_lock(&session_id, &ctx.server.session_locks).await;
					let guard = match lock.try_lock() {
						Ok(g) => g,
						Err(_) => return false,
					};

					let batch = crate::session::inbox::drain_inbox_batch();
					if batch.is_empty() {
						break;
					}

					for inbox_msg in &batch {
						log_debug!(
							"WS monitor: processing inbox message from {:?} for {}",
							inbox_msg.source,
							session_id
						);
					}

					// Take session for exclusive access (lock guarantees no client race).
					let mut chat_session = match ctx.server.sessions.lock().await.remove(&session_id) {
						Some(s) => s,
						None => {
							// Session genuinely gone (cleanup). Put the batch back and stop.
							for inbox_msg in batch {
								crate::session::inbox::push_inbox_message(inbox_msg);
							}
							drop(guard);
							return false;
						}
					};

					let stream = ctx.server.streams.open(&session_id);
					stream.begin_turn();
					let emit = |frame: &ServerMessage| {
						if let Err(e) = stream.emit(frame) {
							log_error!("WS monitor: failed to emit a frame of session {}: {}", session_id, e);
						}
					};
					let config_for_role =
						session_config(&ctx.server.config, &ctx.server.role_configs, &chat_session).await;
					let mut cancellation = crate::session::cancellation::SessionCancellation::new();
					let op_rx = cancellation.new_operation();
					let cancel_probe = op_rx.clone();
					let _turn = TurnGuard::register(&ctx.server.turns, &session_id, &cancellation);

					// Notify the client what's about to drive the AI, before we kick off
					// the API call. Mirrors `display_injected_input` in CLI mode.
					for inbox_msg in &batch {
						emit(&ServerMessage::Injected(crate::websocket::protocol::InjectedPayload {
							source_kind: inbox_msg.source.display_kind().to_string(),
							source_label: inbox_msg.source.display_label(),
							content: inbox_msg.content.clone(),
							session_id: session_id.clone(),
						}));
					}

					if let Err(e) = chat_session
						.add_inbox_batch(batch, &config_for_role, op_rx.clone())
						.await
					{
						log_error!("WS monitor: failed to add inbox message: {}", e);
						ctx.server
							.sessions
							.lock()
							.await
							.insert(session_id.clone(), chat_session);
						continue;
					}

					// Per-request spending boundary (see handle_user_message).
					chat_session.start_request_spending_tracking();
					if let Err(e) = crate::supervisor::accounting::background(prepare_for_api_call(
						&mut chat_session,
						&config_for_role,
						op_rx.clone(),
					))
					.await
					{
						log_error!("WS monitor: failed to prepare API call: {}", e);
						ctx.server
							.sessions
							.lock()
							.await
							.insert(session_id.clone(), chat_session);
						continue;
					}

					// Stream results through a channel that feeds the session's stream.
					let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMessage>();
					let ws_sink = WebSocketSink::new(ws_tx.clone());

					crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_tx);

					let forward_stream = Arc::clone(&stream);
					let forward_session = session_id.clone();
					let forward_task = tokio::spawn(async move {
						while let Some(msg) = ws_rx.recv().await {
							if matches!(msg, ServerMessage::Cost(_)) {
								continue;
							}
							if let Err(e) = forward_stream.emit(&msg) {
								log_error!(
									"WS monitor: failed to emit a frame of session {}: {}",
									forward_session,
									e
								);
							}
						}
					});

					let result = crate::supervisor::accounting::background(execute_api_call_and_process_response(
						&mut chat_session,
						&config_for_role,
						&ctx.server.role,
						op_rx,
						OutputMode::WebSocket,
						ws_sink,
					))
					.await;

					crate::mcp::process::clear_notification_sender(Some(session_id.clone()));
					forward_task.await.expect("WebSocket forwarding task must not panic");

					if let Err(e) = result {
						log_debug!("WS monitor: error processing inbox message: {}", e);
						// Say it out loud, exactly as the pre-user drain does. A debug
						// log is invisible at the level machines actually run at, and
						// the Cost frame below reads to every client as an ordinary
						// finished turn — so a handback that failed resolved the chat
						// with "the agent sent no reply" and left no trace anywhere.
						emit(&ServerMessage::error(format!(
							"Error processing injected message: {}",
							e
						)));
					}

					// Send cost update after processing.
					if let Some(stop) = chat_session.spending_stop {
						emit(&ServerMessage::status(
							format!("Stopped: {stop} spending threshold reached"),
							Some(session_id.clone()),
						));
					}
					let mut cost =
						crate::session::chat::response::session_cost_payload(&mut chat_session, session_id.clone());
					cost.cancelled = *cancel_probe.borrow();
					emit(&ServerMessage::Cost(cost));

					// Save and put session back.
					if let Err(e) = chat_session.save() {
						log_error!("WS monitor: failed to save session: {}", e);
					}
					ctx.server
						.sessions
						.lock()
						.await
						.insert(session_id.clone(), chat_session);
				}

				false // don't exit
			})
			.await;

			if should_exit || ctx.closed.is_cancelled() {
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
					// Without this the monitor of a closed connection parks until
					// something else happens to wake it — one leaked task per
					// connection, for the life of the process.
					_ = ctx.closed.cancelled() => {}
					_ = crate::mcp::orchestration::next_schedule_sleep() => {}
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
		log_debug!("WebSocket: inbox monitor exited for session: {}", session_id);
	});
}

/// Handle a "session" type message: create new or resume existing session.
/// No AI call is made — just session setup. Responds with session_id.
///
/// A bind with `resume` takes the session's output over; one without takes it
/// only when no live connection owns it. Its ack is sent once the session is
/// bound, carrying the stream position — and, for a `resume`, whether the
/// frames the client missed follow it.
async fn handle_session_message(
	msg: SessionMessage,
	ws_sender: &WsSender,
	ctx: &ConnCtx,
	active_session_ids: &mut HashSet<String>,
) -> Result<()> {
	log_debug!("Handling session message: session_id={:?}", msg.session_id);
	let ack = AckPayload::bind(&msg);

	// Refused before the session is taken out of memory or created on disk, so a
	// bad directory costs nothing and leaves an existing session untouched.
	let cwd = match msg.cwd.as_deref().map(PathBuf::from) {
		Some(path) if !path.is_dir() => {
			let error = ServerMessage::error(format!("cwd is not a directory: {}", path.display()));
			return refuse_bind(ws_sender, ack, &error).await;
		}
		other => other,
	};

	// A requested role is resolved and pinned under the session's id before the
	// session exists, so an auto-named bind that asks for one is named now.
	let requested_id = match (&msg.session_id, &msg.role) {
		(Some(id), _) => Some(id.clone()),
		(None, Some(_)) => Some(crate::session::chat::session::generate_session_name()),
		(None, None) => None,
	};

	let (mut chat_session, config_for_role, session_role, is_new) = match &requested_id {
		Some(session_id) => {
			// session_id present: create-or-resume
			// Check memory first
			let existing = ctx.server.sessions.lock().await.remove(session_id);
			if let Some(session) = existing {
				log_debug!("Resumed session from memory: {}", session_id);
				let cfg = session_config(&ctx.server.config, &ctx.server.role_configs, &session).await;
				let role = session.role.clone();
				(session, cfg, role, false)
			} else {
				// Try disk: resume if exists, create with this name if not
				let (args, config) = if crate::session::get_sessions_dir()
					.map(|d| d.join(format!("{}.jsonl.zst", session_id)).exists())
					.unwrap_or(false)
				{
					log_debug!("Resuming session from disk: {}", session_id);
					let (config, role) = match resume_role_config(session_id, &ctx.server).await {
						Ok(restored) => restored,
						Err(e) => {
							return refuse_bind(ws_sender, ack, &ServerMessage::error(e.to_string())).await;
						}
					};
					let args = GenericSessionArgs {
						resume: Some(session_id.clone()),
						role,
						mode: "websocket".into(),
						..Default::default()
					};
					(args, config)
				} else {
					log_debug!("Creating named session: {}", session_id);
					let (config, role) = match msg.role.as_deref() {
						Some(role) if role != ctx.server.role => {
							match pin_role_config(session_id, role, &ctx.server).await {
								Ok(resolved) => resolved,
								Err(e) => {
									let error = ServerMessage::error_for_request(
										format!("Failed to resolve role '{}': {}", role, e),
										msg.request_id.clone(),
									);
									return refuse_bind(ws_sender, ack, &error).await;
								}
							}
						}
						_ => (Arc::clone(&ctx.server.config), ctx.server.role.clone()),
					};
					let args = GenericSessionArgs {
						name: Some(session_id.clone()),
						role,
						mode: "websocket".into(),
						..Default::default()
					};
					(args, config)
				};

				match setup_and_initialize_session(&args, &config, false).await {
					Ok((session, cfg, role_name, _, _)) => {
						let is_new = !session.was_resumed;
						(session, cfg, role_name, is_new)
					}
					Err(e) => {
						let error = ServerMessage::error(format!("Failed to initialize session: {}", e));
						return refuse_bind(ws_sender, ack, &error).await;
					}
				}
			}
		}
		None => {
			// No session_id: create new auto-named session
			log_debug!("Creating new auto-named session with role: {}", ctx.server.role);
			let args = GenericSessionArgs {
				role: ctx.server.role.clone(),
				mode: "websocket".into(),
				..Default::default()
			};
			match setup_and_initialize_session(&args, &ctx.server.config, false).await {
				Ok((session, cfg, role_name, _, _)) => (session, cfg, role_name, true),
				Err(e) => {
					let error = ServerMessage::error(format!("Failed to create session: {}", e));
					return refuse_bind(ws_sender, ack, &error).await;
				}
			}
		}
	};

	let session_id = chat_session.session.info.name.clone();
	// A client may bind the same session more than once on one connection
	// (octomind-api re-sends the `session` frame ahead of every turn). Only the
	// FIRST bind gets a monitor: they all serve this one connection, so the
	// extras are pure duplicates that park forever on the session's Notify.
	let first_bind_here = active_session_ids.insert(session_id.clone());

	// Wrap in session context so all session-scoped registries route correctly
	let role_for_pool = session_role.clone();
	let setup = crate::session::context::with_session_id(session_id.clone(), async {
		// Anchor the directory FIRST: guardrails (init_session_services), skills,
		// AGENTS.md (setup_system_prompt_and_cache) and the MCP session context
		// all read it from the session registry, never from the process cwd.
		if let Some(path) = cwd {
			crate::mcp::set_session_working_directory(path);
		}
		// Initialize session-scoped inbox, job manager, and skill pool so
		// schedule/inbox/skill storage is keyed to this session ID.
		crate::session::context::init_session_services(&role_for_pool);
		crate::session::tap_runs::enable_run_events(Some(ws_sender.clone()))?;
		crate::mcp::core::plan::core::restore_plan_for_session(&session_id);
		crate::mcp::orchestration::schedule::core::restore_schedule_for_session(&session_id);
		crate::mcp::runtime::skill_auto::load_env_skills(&mut chat_session).await;
		crate::mcp::runtime::capability::load_env_capabilities(&config_for_role, None).await;

		setup_system_prompt_and_cache(&mut chat_session, &config_for_role, &session_role, false).await?;

		let status_msg = if is_new {
			format!("Session created: {}", session_id)
		} else {
			format!("Session resumed: {}", session_id)
		};

		log_info!("{}", status_msg);

		chat_session.save()?;
		ctx.server
			.sessions
			.lock()
			.await
			.insert(session_id.clone(), chat_session);
		Ok::<String, anyhow::Error>(status_msg)
	})
	.await;
	let greeting = match setup {
		Ok(status_msg) => ServerMessage::status(status_msg, Some(session_id.clone())),
		Err(e) => {
			send_message(ws_sender, &ServerMessage::Ack(ack)).await?;
			return Err(e);
		}
	};

	// Ack, resumed frames and greeting reach this connection before any live
	// frame of the session does — including those of a turn already running.
	// Whether it answers MCP elicitations follows output ownership (see
	// `SessionStream::attach`).
	let answers_elicitation = msg.capabilities.iter().any(|c| c == ELICITATION_CAPABILITY);
	ctx.server
		.streams
		.open(&session_id)
		.attach(ws_sender, ack, msg.resume.as_ref(), &greeting, answers_elicitation)?;

	// Spawn independent background task that monitors schedules/inbox
	// and processes messages automatically without waiting for user prompts.
	if first_bind_here {
		spawn_ws_inbox_monitor(session_id, ctx.clone());
	}

	Ok(())
}

/// A bind that fails is acknowledged like any frame — without a stream, as
/// none was bound — and then answered with why.
async fn refuse_bind(ws_sender: &WsSender, ack: AckPayload, error: &ServerMessage) -> Result<()> {
	send_message(ws_sender, &ServerMessage::Ack(ack)).await?;
	send_message(ws_sender, error).await
}

/// Config a session's turn runs under, merged for the session's own role.
///
/// The MCP tool map and ownership check both key off this config. Merging for
/// the server role instead hands a `/role`-switched session another role's
/// servers, and every call to its own tools is rejected.
async fn session_config(base: &Config, role_configs: &RoleConfigs, session: &ChatSession) -> Config {
	let pinned = role_configs.lock().await.get(&session.session.info.name).cloned();
	pinned
		.as_deref()
		.unwrap_or(base)
		.get_merged_config_for_role(&session.role)
}

/// Role and config a session loaded from disk runs under.
///
/// A `/role` switch is pinned in memory, which a restart loses; the session log
/// keeps it. Loading under the server role instead runs the first turn after a
/// machine wakes — often a connector or routine that sends no `/role` — as
/// another agent with another agent's tools. The restored role's servers and
/// tool map are built in the session's scope before the session loads.
async fn resume_role_config(session_id: &str, server: &ServerState) -> Result<(Arc<Config>, String)> {
	let Some(role) = crate::session::resume_role(session_id).filter(|role| *role != server.role) else {
		return Ok((Arc::clone(&server.config), server.role.clone()));
	};
	pin_role_config(session_id, &role, server)
		.await
		.map_err(|e| anyhow::anyhow!("Failed to restore role '{}' of session {}: {}", role, session_id, e))
}

/// Resolve `role` against the server's unmerged config, start its servers in
/// the session's scope and pin the result to the session, so every later turn
/// runs under that role's tools. Shared by a bind that names a role and by a
/// disk resume that restores one.
async fn pin_role_config(session_id: &str, role: &str, server: &ServerState) -> Result<(Arc<Config>, String)> {
	let (config, role) = crate::agent::resolver::resolve_config_and_role(Some(role), &server.config, None).await?;
	crate::session::context::with_session_id(
		session_id.to_string(),
		crate::mcp::initialize_mcp_for_role(&role, &config),
	)
	.await?;
	let config = Arc::new(config);
	server
		.role_configs
		.lock()
		.await
		.insert(session_id.to_string(), Arc::clone(&config));
	Ok((config, role))
}

/// Look up an existing session: memory first, then disk. Never auto-create.
/// Returns the session or an error message for the client (callers wrap it in
/// `ServerMessage::error` — keeping the Err variant small).
async fn lookup_session(session_id: &str, server: &ServerState) -> std::result::Result<ChatSession, String> {
	let existing = server.sessions.lock().await.remove(session_id);
	if let Some(session) = existing {
		log_debug!("Resumed session from memory: {}", session_id);
		return Ok(session);
	}

	log_debug!("Loading session from disk: {}", session_id);
	let (config, role) = resume_role_config(session_id, server)
		.await
		.map_err(|e| e.to_string())?;
	let mut args = GenericSessionArgs::resume(session_id.to_string(), role);
	args.mode = "websocket".to_string();
	match setup_and_initialize_session(&args, &config, false).await {
		Ok((mut session, config_for_role, session_role, _, _)) => {
			crate::session::context::init_session_services(&session_role);
			crate::session::tap_runs::enable_run_events(None).map_err(|error| error.to_string())?;
			if let Err(e) = setup_system_prompt_and_cache(&mut session, &config_for_role, &session_role, false).await {
				return Err(format!("Failed to setup session {}: {}", session_id, e));
			}
			log_info!("Session loaded from disk: {}", session_id);
			Ok(session)
		}
		Err(_) => Err(format!(
			"Session not found: {}. Send a \"session\" message first to create or resume a session.",
			session_id
		)),
	}
}

/// The stream a client frame's turn emits on, as that turn begins. The client
/// that sent the frame reads its turn.
fn turn_stream(server: &ServerState, session_id: &str, ws_sender: &WsSender) -> Arc<SessionStream> {
	let stream = server.streams.open(session_id);
	stream.own(ws_sender);
	stream.begin_turn();
	stream
}

/// Handle a "command" type message: execute a session command (e.g. /info, /model, /mcp list).
/// Requires session_id and command. Args are optional.
/// The command is assembled into the CLI slash-command format and routed through process_command.
async fn handle_command_message(msg: CommandMessage, ws_sender: &WsSender, server: &ServerState) -> Result<()> {
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

	let mut chat_session = match lookup_session(session_id, server).await {
		Ok(s) => s,
		Err(error) => {
			send_message(ws_sender, &ServerMessage::error(error)).await?;
			return Ok(());
		}
	};

	let session_id = session_id.to_string();
	let stream = turn_stream(server, &session_id, ws_sender);
	let config_for_role = session_config(&server.config, &server.role_configs, &chat_session).await;
	let mut cancellation = SessionCancellation::new();
	let operation_rx = cancellation.new_operation();
	let _turn = TurnGuard::register(&server.turns, &session_id, &cancellation);

	// Handle /done specially — it hits unreachable!() in process_command
	// because the CLI intercepts it before routing. We handle it here directly.
	if command_name == "done" {
		use crate::session::chat::session::commands::{handle_done, DoneOutcome, LessonHandoff};
		let status_msg = match handle_done(&mut chat_session, &config_for_role, operation_rx, LessonHandoff::Task).await
		{
			Ok(DoneOutcome::Compressed) => "Conversation compressed".to_string(),
			Ok(DoneOutcome::NothingToCompress) => "Nothing to compress".to_string(),
			Ok(DoneOutcome::Failed(e)) => {
				let error = ServerMessage::error(format!("Compression failed: {}", e));
				let save_result = chat_session.save();
				server.sessions.lock().await.insert(session_id, chat_session);
				if let Err(save_err) = save_result {
					stream.emit(&ServerMessage::error(format!("Failed to save session: {}", save_err)))?;
					return Ok(());
				}
				stream.emit(&error)?;
				return Ok(());
			}
			Err(e) => {
				let error = ServerMessage::error(format!("Compression failed: {}", e));
				let save_result = chat_session.save();
				server.sessions.lock().await.insert(session_id, chat_session);
				if let Err(save_err) = save_result {
					stream.emit(&ServerMessage::error(format!("Failed to save session: {}", save_err)))?;
					return Ok(());
				}
				stream.emit(&error)?;
				return Ok(());
			}
		};
		// Send compression status as a command result (data-carrying) so clients finalize
		// the turn — a data-less status is indistinguishable from the connection handshake
		// and would leave an interactive session hung on "working".
		let status = ServerMessage::command_status(
			status_msg.clone(),
			Some(session_id.clone()),
			serde_json::json!({ "command_type": "done", "message": status_msg }),
		);
		let save_result = chat_session.save();
		server.sessions.lock().await.insert(session_id.clone(), chat_session);
		if let Err(e) = save_result {
			stream.emit(&ServerMessage::error(format!("Failed to save session: {}", e)))?;
			return Ok(());
		}
		stream.emit(&status)?;

		// If args were provided, process them as a user message immediately after compression.
		if !args.is_empty() {
			let instructions = args.join(" ");
			let user_msg = crate::websocket::protocol::UserMessage {
				// The follow-up turn's cost closes this command frame.
				request_id: msg.request_id.clone(),
				session_id: session_id.clone(),
				content: instructions,
				attachments: Vec::new(),
			};
			return handle_user_message(user_msg, ws_sender, server).await;
		}
		return Ok(());
	}

	use crate::session::chat::session::commands::CommandResult;
	// `/role` resolves against the server's unmerged config, as `octomind server
	// <role>` does at startup: a config merged for the current role has already
	// dropped every server that role lacks.
	let previous_role = chat_session.role.clone();
	let mut command_config = if command_name == "role" {
		(*server.config).clone()
	} else {
		config_for_role.clone()
	};
	let command_result = match chat_session
		.process_command(&slash_command, &mut command_config, &server.role, operation_rx)
		.await
	{
		Ok(result) => result,
		Err(e) => {
			let error = ServerMessage::error(format!("Command failed: {}", e));
			let save_result = chat_session.save();
			server.sessions.lock().await.insert(session_id, chat_session);
			if let Err(save_err) = save_result {
				stream.emit(&ServerMessage::error(format!("Failed to save session: {}", save_err)))?;
				return Ok(());
			}
			stream.emit(&error)?;
			return Ok(());
		}
	};

	// A switch committed the new role's config: pin it so this session's later
	// turns keep that role's tools instead of the server role's.
	if chat_session.role != previous_role {
		server
			.role_configs
			.lock()
			.await
			.insert(session_id.clone(), Arc::new(command_config));
	}

	let terminal = match command_result {
		CommandResult::Handled => {
			log_debug!("Command '{}' executed successfully", slash_command);
			// Data-carrying status (see command_status) so the client finalizes the turn
			// instead of misreading a data-less status as the handshake ack.
			ServerMessage::command_status(
				format!("Command '{}' executed successfully", slash_command),
				Some(session_id.clone()),
				serde_json::json!({ "command_type": command_name }),
			)
		}
		CommandResult::HandledWithOutput(command_output) => {
			log_debug!("Command '{}' executed with structured output", slash_command);
			ServerMessage::Status(StatusPayload {
				message: format!("Command '{}' executed successfully", slash_command),
				session_id: Some(session_id.clone()),
				data: Some(command_output.to_json()),
			})
		}
		CommandResult::Exit | CommandResult::SwitchSession(_) => {
			log_info!("Session ended by command '{}'", slash_command);
			let status = ServerMessage::command_status(
				"Session ended".to_string(),
				Some(session_id.clone()),
				serde_json::json!({ "command_type": command_name, "action": "exit" }),
			);
			// Don't store session back — it's ended
			if let Err(e) = chat_session.save() {
				server.sessions.lock().await.insert(session_id.clone(), chat_session);
				stream.emit(&ServerMessage::error(format!("Failed to save session: {}", e)))?;
				return Ok(());
			}
			stream.emit(&status)?;
			return Ok(());
		}
		CommandResult::TreatAsUserInput => {
			// Command not recognised — return error rather than silently treating as AI input
			ServerMessage::error(format!(
				"Unknown command: '{}'. Use type \"message\" to send user input.",
				slash_command
			))
		}
	};

	let save_result = chat_session.save();
	server.sessions.lock().await.insert(session_id, chat_session);
	if let Err(e) = save_result {
		stream.emit(&ServerMessage::error(format!("Failed to save session: {}", e)))?;
		return Ok(());
	}
	stream.emit(&terminal)?;
	Ok(())
}

#[derive(Debug)]
struct LoadedMessageAttachments {
	images: Vec<crate::session::image::ImageAttachment>,
	videos: Vec<crate::session::video::VideoAttachment>,
}

fn media_root() -> PathBuf {
	std::env::var_os("OCTOMIND_MEDIA_ROOT")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from("/home/octo/.octomind/media"))
}

/// Validate modality support for the whole turn before resolving or opening a
/// file, then load each attachment from its opaque-ID location.
fn load_message_attachments(
	chat_session: &ChatSession,
	attachments: &[Attachment],
	root: &Path,
) -> Result<LoadedMessageAttachments> {
	if attachments
		.iter()
		.any(|attachment| attachment.kind == AttachmentKind::Image)
	{
		chat_session.ensure_model_supports_vision()?;
	}
	if attachments
		.iter()
		.any(|attachment| attachment.kind == AttachmentKind::Video)
	{
		chat_session.ensure_model_supports_video()?;
	}

	let mut loaded = LoadedMessageAttachments {
		images: Vec::new(),
		videos: Vec::new(),
	};
	for attachment in attachments {
		let path = attachment.resolve_path(root).map_err(anyhow::Error::msg)?;
		let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
			anyhow::anyhow!(
				"Attachment '{}' ({}) is missing or unreadable: {}",
				attachment.name,
				attachment.id,
				error
			)
		})?;
		if metadata.file_type().is_symlink() {
			return Err(anyhow::anyhow!(
				"Attachment '{}' ({}) must not be a symbolic link",
				attachment.name,
				attachment.id
			));
		}
		if !metadata.is_file() {
			return Err(anyhow::anyhow!(
				"Attachment '{}' ({}) must be a regular file",
				attachment.name,
				attachment.id
			));
		}

		match attachment.kind {
			AttachmentKind::Image => {
				let image = crate::session::image::ImageProcessor::load_from_path(&path).map_err(|error| {
					anyhow::anyhow!(
						"Failed to load image attachment '{}' ({}): {}",
						attachment.name,
						attachment.id,
						error
					)
				})?;
				loaded.images.push(image);
			}
			AttachmentKind::Video => {
				let video = crate::session::video::VideoProcessor::load_from_path_with_media_type(
					&path,
					&attachment.media_type,
				)
				.map_err(|error| {
					anyhow::anyhow!(
						"Failed to load video attachment '{}' ({}): {}",
						attachment.name,
						attachment.id,
						error
					)
				})?;
				loaded.videos.push(video);
			}
			AttachmentKind::Audio => {
				std::fs::File::open(&path).map_err(|error| {
					anyhow::anyhow!(
						"Audio attachment '{}' ({}) is unreadable: {}",
						attachment.name,
						attachment.id,
						error
					)
				})?;
			}
		}
	}

	Ok(loaded)
}

/// Handle a "message" type message: send content to an existing session and get AI response.
/// session_id must refer to an already-established session (from a prior "session" message).
async fn handle_user_message(msg: UserMessage, ws_sender: &WsSender, server: &ServerState) -> Result<()> {
	let session_id = msg.session_id.as_str();
	let input = msg.content.clone();

	log_debug!(
		"Handling user message: session_id={}, content_len={}, attachments={}",
		session_id,
		input.len(),
		msg.attachments.len()
	);

	let mut chat_session = match lookup_session(session_id, server).await {
		Ok(s) => s,
		Err(error) => {
			send_message(ws_sender, &ServerMessage::error(error)).await?;
			return Ok(());
		}
	};

	let session_id = session_id.to_string();
	let loaded_attachments = match load_message_attachments(&chat_session, &msg.attachments, &media_root()) {
		Ok(attachments) => attachments,
		Err(error) => {
			server.sessions.lock().await.insert(session_id.clone(), chat_session);
			send_message(
				ws_sender,
				&ServerMessage::error_for_request(format!("Error: {}", error), msg.request_id.clone()),
			)
			.await?;
			return Ok(());
		}
	};

	// From here on every frame is the turn's: it goes to the session's stream.
	let stream = turn_stream(server, &session_id, ws_sender);
	let config_for_role = session_config(&server.config, &server.role_configs, &chat_session).await;
	let mut cancellation = SessionCancellation::new();
	// Registered for the whole frame: the handle follows every new_operation()
	// below, so a `cancel` stops whichever turn — injected or the user's — runs.
	let _turn = TurnGuard::register(&server.turns, &session_id, &cancellation);
	// The main-call `operation_rx` is created AFTER the pre-user inbox drain
	// below: the drain calls new_operation() per message, each dropping the
	// previous watch sender, so a receiver made here would be closed by the
	// time the main call runs — which tool execution reads as instant cancel.

	// Process any inbox messages that arrived before this user message
	// (background agents, scheduled entries, skills) — each gets its own
	// full AI turn so the model actually responds to them, not just sees them
	// silently prepended to the conversation.
	let mut drain_stopped = false;
	{
		// Flush due schedule entries first.
		crate::mcp::orchestration::flush_due_to_inbox();
		crate::mcp::orchestration::flush_idle_to_inbox();
		loop {
			let batch = crate::session::inbox::drain_inbox_batch();
			if batch.is_empty() {
				break;
			}
			for inbox_msg in &batch {
				log_debug!(
					"WebSocket pre-user: processing inbox message from {:?}",
					inbox_msg.source
				);
				// Tell the client what's being injected before the AI responds to it.
				stream.emit(&ServerMessage::Injected(crate::websocket::protocol::InjectedPayload {
					source_kind: inbox_msg.source.display_kind().to_string(),
					source_label: inbox_msg.source.display_label(),
					content: inbox_msg.content.clone(),
					session_id: session_id.clone(),
				}))?;
			}
			let op_rx = cancellation.new_operation();
			if let Err(e) = chat_session
				.add_inbox_batch(batch, &config_for_role, op_rx.clone())
				.await
			{
				log_error!("WebSocket pre-user: failed to add inbox message: {}", e);
				stream.emit(&ServerMessage::error(format!(
					"Error processing injected message: {}",
					e
				)))?;
				continue;
			}
			// Per-request spending boundary (see handle_user_message).
			chat_session.start_request_spending_tracking();
			if let Err(e) = prepare_for_api_call(&mut chat_session, &config_for_role, op_rx.clone()).await {
				log_error!("WebSocket pre-user: failed to prepare API call: {}", e);
				stream.emit(&ServerMessage::error(format!(
					"Error processing injected message: {}",
					e
				)))?;
				continue;
			}
			let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
			let sink = WebSocketSink::new(tx);
			let result = execute_api_call_and_process_response(
				&mut chat_session,
				&config_for_role,
				&server.role,
				op_rx,
				OutputMode::WebSocket,
				sink,
			)
			.await;
			while let Ok(mut msg) = rx.try_recv() {
				// This cost closes an injected turn, not the client's: the message
				// that drained it runs next, on this same connection.
				if let ServerMessage::Cost(cost) = &mut msg {
					cost.pending_work = true;
				}
				stream.emit(&msg)?;
			}
			// A `cancel` read while the drain runs stops this frame, not just the
			// injected turn in flight: no further batch, and no user turn below.
			if cancellation.is_cancelled() {
				drain_stopped = true;
				break;
			}
			if let Err(e) = result {
				log_debug!("Error processing pre-user inbox message (websocket): {}", e);
				stream.emit(&ServerMessage::error(format!(
					"Error processing injected message: {}",
					e
				)))?;
				// A failed turn emits no cost of its own; close the injected turn
				// here so clients see it end before the message's own turn starts.
				let mut cost =
					crate::session::chat::response::session_cost_payload(&mut chat_session, session_id.clone());
				cost.pending_work = true;
				stream.emit(&ServerMessage::Cost(cost))?;
			}
		}
	}

	if drain_stopped {
		log_debug!("Message cancelled during the pre-user drain: {}", session_id);
		let save_result = chat_session.save();
		let mut cost = crate::session::chat::response::session_cost_payload(&mut chat_session, session_id.clone());
		cost.cancelled = true;
		cost.request_id = msg.request_id.clone();
		server.sessions.lock().await.insert(session_id.clone(), chat_session);
		if let Err(e) = save_result {
			stream.emit(&ServerMessage::error(format!("Failed to save session: {}", e)))?;
		}
		return stream.emit(&ServerMessage::Cost(cost));
	}

	// Create the main call's cancellation receiver NOW — after the inbox drain,
	// so it points at the live watch channel (see note above).
	let operation_rx = cancellation.new_operation();

	// Run pipe pre-processing if a matching [[pipe]] is configured.
	let first_message_processed = !chat_session.session.messages.is_empty();

	let processed_input = match run_pipe_if_enabled(&input, &server.role, first_message_processed).await {
		Ok(input) => input,
		Err(e) => {
			server.sessions.lock().await.insert(session_id.clone(), chat_session);
			stream.emit(&ServerMessage::error(format!("Error: {}", e)))?;
			return Ok(());
		}
	};

	// Add user message
	if let Err(e) = chat_session.add_user_message_with_attachments(
		&processed_input,
		loaded_attachments.images,
		loaded_attachments.videos,
	) {
		server.sessions.lock().await.insert(session_id.clone(), chat_session);
		stream.emit(&ServerMessage::error(format!("Error: {}", e)))?;
		return Ok(());
	}

	// Reset the per-request spending checkpoint at the request boundary, like the
	// interactive loop (main_loop.rs) — otherwise max_request_spending_threshold
	// measures cost since session start and silently degrades into a session cap.
	chat_session.start_request_spending_tracking();

	// Prepare for API call
	if let Err(e) = prepare_for_api_call(&mut chat_session, &config_for_role, operation_rx.clone()).await {
		server.sessions.lock().await.insert(session_id.clone(), chat_session);
		stream.emit(&ServerMessage::error(format!("Error: {}", e)))?;
		return Ok(());
	}

	// Create channel for WebSocket sink to stream messages
	let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel();
	let ws_sink = WebSocketSink::new(ws_tx.clone());

	// Forward MCP server notifications through the WebSocket channel
	crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_tx);

	// Execute the API call while concurrently forwarding sink events to the
	// session's stream, so chunks stream in real-time. Draining only after the call
	// returned (the previous behavior) buffered a whole multi-minute turn and
	// delivered it as one burst — the "real-time" claim was false. ws_rx.recv()
	// stays open for the call's duration (ws_tx clone + notification sender hold
	// senders), so its branch only yields real messages; the api future is what
	// ends the loop.
	// Scope the future so its `&mut chat_session` borrow is released the moment
	// the loop ends — otherwise the pinned future lives to end-of-fn and blocks
	// every later use of chat_session (cost read, save, move back into the map).
	let api_result = {
		let api_fut = execute_api_call_and_process_response(
			&mut chat_session,
			&config_for_role,
			&server.role,
			operation_rx.clone(),
			OutputMode::WebSocket,
			ws_sink,
		);
		tokio::pin!(api_fut);
		loop {
			tokio::select! {
				res = &mut api_fut => break res,
				Some(msg) = ws_rx.recv() => {
					if matches!(msg, ServerMessage::Cost(_)) { continue; }
					stream.emit(&msg)?;
				}
			}
		}
	};

	// Drain any messages queued between the last poll and completion.
	while let Ok(msg) = ws_rx.try_recv() {
		if matches!(msg, ServerMessage::Cost(_)) {
			continue;
		}
		stream.emit(&msg)?;
	}

	// Save session
	log_debug!("Saving session: {}", session_id);
	let save_result = chat_session.save();
	let spending_stop = chat_session.spending_stop;
	let cancelled = *operation_rx.borrow();
	let mut cost = crate::session::chat::response::session_cost_payload(&mut chat_session, session_id.clone());
	cost.cancelled = cancelled;
	cost.request_id = msg.request_id.clone();
	let cost_msg = ServerMessage::Cost(cost);

	// Clear the notification sender now that this request is done
	crate::mcp::process::clear_notification_sender(Some(session_id.clone()));

	// Store session back and wake inbox monitor if it has pending messages.
	server.sessions.lock().await.insert(session_id.clone(), chat_session);
	if let Some(notify) = crate::session::inbox::get_inbox_notify() {
		notify.notify_one();
	}
	log_debug!("Session stored back in memory: {}", session_id);

	if let Err(e) = save_result {
		stream.emit(&ServerMessage::error(format!("Failed to save session: {}", e)))?;
		stream.emit(&cost_msg)?;
		return Ok(());
	}

	match api_result {
		Ok(_) => {
			if let Some(stop) = spending_stop {
				stream.emit(&ServerMessage::status(
					format!("Stopped: {stop} spending threshold reached"),
					Some(session_id.clone()),
				))?;
			}
		}
		// A cancelled turn is a requested stop, not a failure: the cost frame's
		// `cancelled` flag reports it.
		Err(e) if cancelled && crate::session::cancellation::is_cancelled(&e) => {
			log_debug!("Turn cancelled for session {}", session_id);
		}
		Err(e) => {
			log_error!("API call failed: {}", e);
			let error = ServerMessage::error(format!("Error: {}", e));
			stream.emit(&error)?;
		}
	}
	stream.emit(&cost_msg)?;

	Ok(())
}

/// Send a server message through WebSocket
async fn send_message(ws_sender: &WsSender, msg: &ServerMessage) -> Result<()> {
	let json = serde_json::to_string(msg)?;
	log_debug!(
		"Sending message: type={:?}, size={} bytes",
		std::mem::discriminant(msg),
		json.len()
	);
	ws_sender.send(Message::text(json))?;
	Ok(())
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
