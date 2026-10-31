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
	AckPayload, Attachment, AttachmentKind, CancelMessage, ClientMessage, CloseMessage, CommandInfo, CommandMessage,
	ElicitationResponseMessage, ServerMessage, SessionMessage, StatusPayload, UserMessage, COMMANDS_CAPABILITY,
	ELICITATION_CAPABILITY,
};
use super::stream::{SessionStream, StreamSink, Streams};
use crate::config::Config;
use crate::session::cancellation::{CancelHandle, SessionCancellation};
use crate::session::chat::session::{
	execute_api_call_and_process_response, prepare_for_api_call, run_pipe_if_enabled, setup_and_initialize_session,
	setup_system_prompt_and_cache, ChatSession, GenericSessionArgs,
};
use crate::session::chat::{Reach, BUILTIN_COMMANDS, RUN_COMMAND};
use crate::session::output::OutputMode;
use crate::{log_debug, log_error, log_info};
use anyhow::Result;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
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
type SessionLocks = Arc<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>>;

/// Sessions that have a background monitor (see `ensure_session_monitor`).
type Monitors = Arc<std::sync::Mutex<HashSet<String>>>;

pub(super) const MAX_QUEUED_FRAMES: usize = 8192;
pub(super) const MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEFERRED_FRAMES: usize = 256;
const MAX_DEFERRED_BYTES: usize = 10 * 1024 * 1024;
const MAX_LOADED_SESSIONS: usize = 128;
const MAX_CONNECTIONS: usize = 64;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const WRITER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// A client silent for a whole interval after a ping is dropped, so half-open
/// connections cannot hold `MAX_CONNECTIONS` slots forever.
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Pause after a failed accept: errors like EMFILE persist until a connection
/// closes, and retrying at once would spin a core and flood the log.
const ACCEPT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// The answer to any frame on an observer connection.
const OBSERVER_READ_ONLY_ERROR: &str = "observer connections are read-only";
/// The answer to `observe` on a connection that already bound a session: it may
/// own that session's output and answer its questions, which an observer never
/// does, so it would leave them stranded.
const OBSERVE_AFTER_BIND_ERROR: &str = "observe must come before any session bind on a connection";

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
	fn register(server: &ServerState, session_id: &str, cancellation: &SessionCancellation) -> Self {
		let handle = cancellation.handle();
		server
			.turns
			.lock()
			.unwrap()
			.insert(session_id.to_string(), handle.clone());
		server.turn_started.notify_waiters();
		Self {
			turns: Arc::clone(&server.turns),
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

/// Keeps background ownership and the session lock atomic to client admission.
struct BackgroundTurnGuard<'a> {
	turns: Arc<std::sync::Mutex<HashSet<String>>>,
	session_id: String,
	guard: Option<tokio::sync::MutexGuard<'a, ()>>,
}

impl<'a> BackgroundTurnGuard<'a> {
	fn try_lock(server: &ServerState, session_id: &str, lock: &'a Mutex<()>) -> Option<Self> {
		let mut turns = server.background_turns.lock().unwrap();
		let guard = lock.try_lock().ok()?;
		turns.insert(session_id.to_string());
		Some(Self {
			turns: Arc::clone(&server.background_turns),
			session_id: session_id.to_string(),
			guard: Some(guard),
		})
	}
}

impl Drop for BackgroundTurnGuard<'_> {
	fn drop(&mut self) {
		let mut turns = self.turns.lock().unwrap();
		drop(self.guard.take());
		turns.remove(&self.session_id);
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
	background_turns: Arc<std::sync::Mutex<HashSet<String>>>,
	/// Fired as a turn registers: a bind waiting on the session's lock learns the
	/// wait is a turn's ([`lock_for_bind`]).
	turn_started: Arc<tokio::sync::Notify>,
	/// Output streams of the loaded sessions: what each session sends, and to
	/// which connection.
	streams: Streams,
	/// Includes loads and turns while their ChatSession is outside `sessions`.
	residents: Arc<std::sync::Mutex<HashSet<String>>>,
	/// Sessions whose background monitor runs: one each, from the session's
	/// first load until it is unloaded.
	monitors: Monitors,
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
			background_turns: Arc::default(),
			turn_started: Arc::default(),
			streams: Streams::default(),
			residents: Arc::default(),
			monitors: Arc::default(),
			allow_origins: Arc::new(allow_origins),
			token: None,
		}
	}

	fn reserve_session(&self, session_id: &str) -> Result<SessionReservation> {
		let mut residents = self.residents.lock().unwrap();
		let existing = residents.contains(session_id);
		anyhow::ensure!(
			existing || residents.len() < MAX_LOADED_SESSIONS,
			"WebSocket loaded session limit reached; close an idle session before loading another"
		);
		residents.insert(session_id.to_string());
		Ok(SessionReservation {
			residents: Arc::clone(&self.residents),
			session_id: session_id.to_string(),
			committed: existing,
		})
	}
}

/// A failed load releases its admission slot; a loaded session keeps it until
/// an explicit close, so reconnects never discard pending handbacks or state.
struct SessionReservation {
	residents: Arc<std::sync::Mutex<HashSet<String>>>,
	session_id: String,
	committed: bool,
}

impl Drop for SessionReservation {
	fn drop(&mut self) {
		if !self.committed {
			self.residents.lock().unwrap().remove(&self.session_id);
		}
	}
}

/// Per-connection context passed to the message handlers.
#[derive(Clone)]
struct ConnCtx {
	server: ServerState,
	/// Cancelled once the client is gone; a turn the client left keeps running.
	closed: CancellationToken,
}

impl ConnCtx {
	/// The client is gone: no session sends to it any more — its frames are held
	/// for the next bind.
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

		let mut connections = tokio::task::JoinSet::new();
		loop {
			tokio::select! {
				Some(joined) = connections.join_next() => {
					// A panicking handler ends only its own connection.
					if let Err(e) = joined {
						log_error!("Connection handler task failed: {}", e);
					}
				}
				accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
					let (stream, peer_addr) = match accepted {
						Ok(accepted) => accepted,
						Err(e) => {
							log_error!("Failed to accept connection: {}", e);
							tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
							continue;
						}
					};
					log_info!("Connection accepted from {}", peer_addr);

					let state = self.state.clone();
					connections.spawn(async move {
						if let Err(e) = handle_connection(stream, peer_addr, state).await {
							log_error!("Connection handler failed for {}: {}", peer_addr, e);
						}
					});
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

/// Read half of a connection. Pings the client every interval and ends the
/// stream once a whole interval passes without any frame from it (a pong
/// counts), so dead peers release their connection slot.
struct WsReader {
	frames: SplitStream<WebSocketStream<TcpStream>>,
	heartbeat: tokio::time::Interval,
	heard: bool,
	peer_addr: SocketAddr,
}

impl WsReader {
	fn new(
		frames: SplitStream<WebSocketStream<TcpStream>>,
		peer_addr: SocketAddr,
		interval: std::time::Duration,
	) -> Self {
		let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
		heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
		Self {
			frames,
			heartbeat,
			heard: true,
			peer_addr,
		}
	}

	/// Cancel-safe: both awaited futures are, and state changes are synchronous.
	async fn next(&mut self, ws_sender: &WsSender) -> Option<Result<Message, tokio_tungstenite::tungstenite::Error>> {
		loop {
			tokio::select! {
				frame = self.frames.next() => {
					self.heard = true;
					return frame;
				}
				_ = self.heartbeat.tick() => {
					if !std::mem::replace(&mut self.heard, false) {
						log_info!("Client {} missed its heartbeat, closing", self.peer_addr);
						return None;
					}
					if let Err(error) = ws_sender.send(Message::Ping(Default::default())) {
						log_debug!("Failed to ping {}: {}", self.peer_addr, error);
						return None;
					}
				}
			}
		}
	}
}

/// Client-bound half of a connection. A writer task owns the socket sink, so
/// the frame being processed and the reader — which answers `cancel` frames and
/// pings while that frame runs — both send without sharing it. Frames keep their
/// order; a send fails once the socket is gone, as a direct write would.
#[derive(Clone)]
pub(crate) struct WsSender {
	tx: tokio::sync::mpsc::Sender<QueuedFrame>,
	bytes: Arc<tokio::sync::Semaphore>,
	closed: CancellationToken,
}

pub(crate) struct QueuedFrame {
	pub(crate) frame: Message,
	_bytes: tokio::sync::OwnedSemaphorePermit,
}

impl WsSender {
	fn spawn(
		mut sink: futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>,
	) -> (Self, tokio::task::JoinHandle<()>) {
		let (sender, mut rx) = Self::channel();
		let closed = sender.closed.clone();
		let writer = tokio::spawn(async move {
			loop {
				let queued = tokio::select! {
					_ = closed.cancelled() => return,
					queued = rx.recv() => queued,
				};
				let Some(queued) = queued else { break };
				tokio::select! {
					_ = closed.cancelled() => return,
					result = sink.send(queued.frame) => {
						if let Err(e) = result {
							log_debug!("WebSocket write failed, closing the writer: {}", e);
							closed.cancel();
							return;
						}
					}
				}
			}
			if let Err(error) = sink.close().await {
				log_debug!("WebSocket close failed: {}", error);
			}
		});
		(sender, writer)
	}

	pub(crate) fn same_channel(&self, other: &Self) -> bool {
		self.tx.same_channel(&other.tx)
	}

	pub(crate) fn send(&self, frame: Message) -> Result<()> {
		anyhow::ensure!(!self.closed.is_cancelled(), "WebSocket connection closed");
		let permits = u32::try_from(frame.len().max(1))
			.ok()
			.and_then(|size| Arc::clone(&self.bytes).try_acquire_many_owned(size).ok());
		let Some(permits) = permits else {
			self.closed.cancel();
			anyhow::bail!("WebSocket outgoing byte limit exceeded");
		};
		if let Err(error) = self.tx.try_send(QueuedFrame { frame, _bytes: permits }) {
			self.closed.cancel();
			return Err(anyhow::anyhow!("WebSocket outgoing queue unavailable: {}", error));
		}
		Ok(())
	}

	pub(crate) fn channel() -> (Self, tokio::sync::mpsc::Receiver<QueuedFrame>) {
		let (tx, rx) = tokio::sync::mpsc::channel(MAX_QUEUED_FRAMES);
		(
			Self {
				tx,
				bytes: Arc::new(tokio::sync::Semaphore::new(MAX_QUEUED_BYTES)),
				closed: CancellationToken::new(),
			},
			rx,
		)
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

	let ws_stream = tokio::time::timeout(
		HANDSHAKE_TIMEOUT,
		tokio_tungstenite::accept_hdr_async_with_config(
			stream,
			HandshakeGuard {
				origins: Arc::clone(&server.allow_origins),
				token: server.token.clone(),
			},
			Some(ws_config),
		),
	)
	.await??;
	log_info!("WebSocket handshake completed for {}", peer_addr);

	let (sink, frames) = ws_stream.split();
	let mut ws_receiver = WsReader::new(frames, peer_addr, HEARTBEAT_INTERVAL);
	let (ws_sender, writer) = WsSender::spawn(sink);
	let mut writer = tokio_util::task::AbortOnDropHandle::new(writer);

	let result = serve_connection(&mut ws_receiver, &ws_sender, peer_addr, server).await;

	// Let the writer flush what is queued and close the socket.
	drop(ws_sender);
	match tokio::time::timeout(WRITER_SHUTDOWN_TIMEOUT, &mut writer).await {
		Ok(joined) => joined?,
		Err(_) => {
			writer.abort();
			if let Err(error) = writer.await {
				if !error.is_cancelled() {
					return Err(error.into());
				}
			}
		}
	}
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
	// The connection sent `observe`: from then on it only reads.
	let mut observing = false;

	loop {
		let msg = if let Some(frame) = deferred.pop_front() {
			Ok(frame)
		} else if stream_ended {
			break;
		} else {
			match tokio::select! {
				_ = ws_sender.closed.cancelled() => None,
				frame = ws_receiver.next(ws_sender) => frame,
			} {
				Some(msg) => msg,
				None => break, // stream ended
			}
		};

		match msg {
			Ok(Message::Text(text)) => {
				log_debug!("Received message from {}: {} bytes", peer_addr, text.len());
				if observing {
					send_message(ws_sender, &ServerMessage::error(OBSERVER_READ_ONLY_ERROR.to_string())).await?;
					continue;
				}
				let Some(client_msg) = accept_client_frame(&text, ws_sender, peer_addr).await? else {
					continue;
				};
				if let ClientMessage::Observe(observe) = &client_msg {
					if !active_session_ids.is_empty() {
						send_message(ws_sender, &ServerMessage::error(OBSERVE_AFTER_BIND_ERROR.to_string())).await?;
						continue;
					}
					ctx.server.streams.observe(ws_sender, &observe.sessions)?;
					observing = true;
					continue;
				}

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

	// Session notification sinks outlive connections and hold only bounded replay.
	//
	// INVARIANT: Do NOT call stop_all_servers() here. MCP server processes are shared
	// across all active sessions. Killing them on disconnect would break other sessions
	// that are still using the same servers — which is also why a role switch never
	// stops any. stop_all_servers() is only called on process shutdown (main.rs).
	log_info!(
		"Connection closed: {} (cleaned up {} session(s))",
		peer_addr,
		active_session_ids.len()
	);
	Ok(())
}

/// Parse, validate and acknowledge one client text frame. Invalid frames are
/// answered with an error here and yield `None`. A `session` bind is
/// acknowledged by its handler instead, once it knows the session's stream, and
/// an `observe` by the `observing` frame.
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

	if !matches!(client_msg, ClientMessage::Session(_) | ClientMessage::Observe(_)) {
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
	let mut deferred_bytes: usize = deferred.iter().map(Message::len).sum();
	loop {
		tokio::select! {
			result = &mut processing => return result,
			_ = ws_sender.closed.cancelled(), if !*stream_ended => abandon(stream_ended),
			frame = ws_receiver.next(ws_sender), if !*stream_ended => match frame {
				Some(Ok(Message::Text(text))) => {
					let immediate = matches!(
						serde_json::from_str::<ClientMessage>(&text),
						Ok(ClientMessage::Cancel(_) | ClientMessage::RunLog(_) | ClientMessage::ElicitationResponse(_))
					);
					if !immediate {
						if deferred.len() >= MAX_DEFERRED_FRAMES || text.len() > MAX_DEFERRED_BYTES - deferred_bytes {
							let request_id = serde_json::from_str::<ClientMessage>(&text)
								.ok().and_then(|msg| msg.request_id().map(ToOwned::to_owned));
							let error = ServerMessage::error_for_request("WebSocket pending input limit exceeded".into(), request_id);
							if let Err(error) = send_message(ws_sender, &error).await {
								log_debug!("Failed to reject queued input from {}: {}", peer_addr, error);
								abandon(stream_ended);
							}
						} else {
							deferred_bytes += text.len();
							deferred.push_back(Message::Text(text));
						}
					} else {
						let handled = async {
							if let Some(message) = accept_client_frame(&text, ws_sender, peer_addr).await? {
								match message {
									ClientMessage::Cancel(cancel) => cancel_turn(&ctx.server, &cancel),
									ClientMessage::RunLog(request) => handle_run_log(request, ws_sender).await?,
									ClientMessage::ElicitationResponse(response) => handle_elicitation_response(response, ws_sender).await?,
									_ => unreachable!("immediate control frame"),
								}
							}
							Ok::<(), anyhow::Error>(())
						}.await;
						if let Err(error) = handled {
							log_debug!("WebSocket control response failed for {}: {}", peer_addr, error);
							abandon(stream_ended);
						}
					}
				}
				Some(Ok(Message::Ping(data))) => {
					if let Err(error) = ws_sender.send(Message::Pong(data)) {
						log_debug!("Failed to send pong to {}: {}", peer_addr, error);
						abandon(stream_ended);
					}
				}
				Some(Ok(Message::Pong(_))) => {}
				Some(Ok(Message::Close(_))) => abandon(stream_ended),
				Some(Ok(Message::Binary(_))) => {
					let error = ServerMessage::error("Unsupported WebSocket message type: send JSON text frames".into());
					if let Err(error) = send_message(ws_sender, &error).await {
						log_debug!("Failed to reject binary input from {}: {}", peer_addr, error);
						abandon(stream_ended);
					}
				}
				Some(Ok(_)) => {}
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
		// Boxed: a bind's future is deep, and inlined into the connection's it overflows
		// the compiler's layout depth.
		ClientMessage::Session(msg) => Box::pin(handle_session_message(msg, ws_sender, ctx, active_session_ids)).await,
		ClientMessage::Message(msg) => {
			let session_id = msg.session_id.clone();

			// Acquire per-session lock to prevent concurrent access
			let lock = get_or_create_session_lock(&session_id, &ctx.server.session_locks).await;
			let guard = match lock_for_client(&lock, &ctx.server, &session_id).await {
				Some(guard) => guard,
				None => {
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
			if ctx.server.sessions.lock().await.contains_key(&session_id) {
				active_session_ids.insert(session_id.clone());
			}

			drop(guard);
			wake_inbox_monitor(session_id).await;
			result
		}
		ClientMessage::Command(msg) => {
			let session_id = msg.session_id.clone();

			// Acquire per-session lock to prevent concurrent access
			let lock = get_or_create_session_lock(&session_id, &ctx.server.session_locks).await;
			let guard = match lock_for_client(&lock, &ctx.server, &session_id).await {
				Some(guard) => guard,
				None => {
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
			if ctx.server.sessions.lock().await.contains_key(&session_id) {
				active_session_ids.insert(session_id.clone());
			} else {
				active_session_ids.remove(&session_id);
			}

			drop(guard);
			wake_inbox_monitor(session_id).await;
			result
		}
		ClientMessage::RunLog(msg) => handle_run_log(msg, ws_sender).await,
		// Normally applied by the connection loop at once; this arm covers a frame deferred behind a turn.
		ClientMessage::ElicitationResponse(msg) => handle_elicitation_response(msg, ws_sender).await,
		ClientMessage::Close(msg) => handle_close_message(msg, ws_sender, &ctx.server, active_session_ids).await,
		// Applied by the connection loop the moment it is read, never queued here.
		ClientMessage::Observe(_) => unreachable!("observe is applied by the connection loop"),
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
	server.residents.lock().unwrap().remove(&session_id);
	// A later bind loads the session anew, under a new stream.
	server.streams.close(&session_id);
	cleanup_and_wake_monitor(&session_id).await;
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

/// Drops the session's scope-keyed state and wakes its monitor, which sees the
/// inbox gone and retires now rather than at its next event.
async fn cleanup_and_wake_monitor(session_id: &str) {
	let inbox_notify = crate::session::context::with_session_id(session_id.to_string(), async {
		crate::session::inbox::get_inbox_notify()
	})
	.await;
	crate::session::context::cleanup_session(&session_id.to_string());
	if let Some(notify) = inbox_notify {
		notify.notify_one();
	}
}

/// Get or create a per-session processing lock.
async fn get_or_create_session_lock(session_id: &str, session_locks: &SessionLocks) -> Arc<tokio::sync::Mutex<()>> {
	let mut locks = session_locks.lock().await;
	locks.retain(|_, lock| lock.strong_count() > 0);
	if let Some(lock) = locks.get(session_id).and_then(Weak::upgrade) {
		return lock;
	}
	let lock = Arc::new(tokio::sync::Mutex::new(()));
	locks.insert(session_id.to_string(), Arc::downgrade(&lock));
	lock
}

/// Wait behind inbox work, but refuse a competing foreground request.
async fn lock_for_client<'a>(
	lock: &'a Mutex<()>,
	server: &ServerState,
	session_id: &str,
) -> Option<tokio::sync::MutexGuard<'a, ()>> {
	{
		let background = server.background_turns.lock().unwrap();
		match lock.try_lock() {
			Ok(guard) => return Some(guard),
			Err(_) if !background.contains(session_id) => return None,
			Err(_) => {}
		}
	}
	// FIFO prevents the next background batch from overtaking this request.
	Some(lock.lock().await)
}

/// Re-fire the session's inbox wake-up once a client frame has released the
/// session lock. A handback that lands while the frame runs wakes the monitor,
/// which fails `try_lock` and parks again; the turn's own wake-up
/// (`handle_user_message`) fires while that lock is still held. Without this the
/// result sits queued until something unrelated wakes the monitor — and a client
/// told `pending_work` waits out its cap for a turn that never starts.
async fn wake_inbox_monitor(session_id: String) {
	crate::session::context::with_session_id(session_id, async {
		if crate::session::inbox::has_inbox_messages() || crate::mcp::orchestration::has_pending_idle_schedules() {
			if let Some(notify) = crate::session::inbox::get_inbox_notify() {
				notify.notify_one();
			}
		}
	})
	.await;
}

/// Session monitors currently alive: one per loaded session, owned by the
/// server and never by a connection. The count must fall back to zero as
/// sessions unload — a count that only grows is the leak this exists to
/// catch, and nothing else observes it from outside.
static LIVE_INBOX_MONITORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Decrements the live count however the monitor task ends, including a panic.
/// A monitor that retired itself already freed its registry slot, which a newer
/// monitor may hold by now; any other exit frees it here, so the session is
/// never left without one.
struct MonitorGuard {
	monitors: Monitors,
	session_id: String,
	retired: bool,
}

impl MonitorGuard {
	/// The session was unloaded (`cleanup_session` clears its role and inbox
	/// together): free the slot. Decided under the registry lock, so a load
	/// that follows either finds this monitor still registered and keeps it, or
	/// finds the slot free and starts its own.
	fn retire_if_unloaded(&mut self) -> bool {
		let mut monitors = self.monitors.lock().unwrap();
		let inbox_gone = crate::session::context::CURRENT_SESSION_ID
			.sync_scope(Arc::new(self.session_id.clone()), || {
				crate::session::inbox::get_inbox_notify().is_none()
			});
		let unloaded = inbox_gone || crate::session::context::get_session_role(&self.session_id).is_none();
		if unloaded {
			monitors.remove(&self.session_id);
			self.retired = true;
		}
		unloaded
	}
}

impl Drop for MonitorGuard {
	fn drop(&mut self) {
		LIVE_INBOX_MONITORS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
		if !self.retired {
			self.monitors.lock().unwrap().remove(&self.session_id);
		}
	}
}

/// Start the session's background monitor unless it has one. Called the first
/// time a session enters `server.sessions` by any path — bind, attach, a lazy
/// load on a message or command — so a session works with no client connected:
/// the monitor lives until the session is unloaded, whatever connections come
/// and go.
///
/// Runs independently of user prompts — when a scheduled entry fires or an inbox
/// message arrives, the task takes the session from the map, processes the
/// message through the full AI pipeline, emits the results on the session's
/// stream (which sends them to the output owner and every observer, or only
/// holds them when nobody is connected), and puts the session back.
fn ensure_session_monitor(server: &ServerState, session_id: &str) {
	if !server.monitors.lock().unwrap().insert(session_id.to_string()) {
		return;
	}
	let server = server.clone();
	let session_id = session_id.to_string();
	// Counted and guarded before the task runs, so a task that never starts
	// (runtime shutdown) still gives its count and its registry slot back.
	LIVE_INBOX_MONITORS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
	let mut reaped = MonitorGuard {
		monitors: Arc::clone(&server.monitors),
		session_id: session_id.clone(),
		retired: false,
	};
	tokio::spawn(async move {
		log_debug!("WebSocket: inbox monitor started for session: {}", session_id);
		loop {
			// Process phase: flush due schedules into inbox, then drain.
			crate::session::context::with_session_id(session_id.clone(), async {
				let Some(schedule_role) = crate::session::context::get_session_role(&session_id) else {
					return;
				};
				let schedule_config =
					role_config(&server.config, &server.role_configs, &session_id, &schedule_role).await;
				crate::mcp::orchestration::flush_due_to_inbox(&schedule_config);
				// Unlike messages, idle tool calls start immediately, so do not launch during a parent turn.
				let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
				if let Some(_guard) = BackgroundTurnGuard::try_lock(&server, &session_id, &lock) {
					crate::mcp::orchestration::flush_idle_to_inbox(&schedule_config);
				}

				// Drain inbox while messages remain. Take the SAME per-session lock
				// process_client_message uses, held across take→process→reinsert:
				// otherwise a concurrent client message try_locks it (free, because
				// removing the session from the map is not the lock), misses memory,
				// and reloads a stale copy from disk — two live sessions racing the
				// same file. If the client holds the lock, stop this pass; it fires
				// inbox_notify on return and wakes us from the wait section.
				while crate::session::inbox::has_inbox_messages() {
					let lock = get_or_create_session_lock(&session_id, &server.session_locks).await;
					let guard = match BackgroundTurnGuard::try_lock(&server, &session_id, &lock) {
						Some(guard) => guard,
						None => return,
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
					let mut chat_session = match server.sessions.lock().await.remove(&session_id) {
						Some(s) => s,
						None => {
							// Session genuinely gone (cleanup). Put the batch back and stop.
							for inbox_msg in batch {
								crate::session::inbox::push_inbox_message(inbox_msg);
							}
							drop(guard);
							return;
						}
					};

					let stream = server.streams.open(&session_id);
					let emit = |frame: &ServerMessage| {
						if let Err(e) = stream.emit(frame) {
							log_error!("WS monitor: failed to emit a frame of session {}: {}", session_id, e);
						}
					};
					let config_for_role = session_config(&server.config, &server.role_configs, &chat_session).await;
					let mut cancellation = crate::session::cancellation::SessionCancellation::new();
					let op_rx = cancellation.new_operation();
					let cancel_probe = op_rx.clone();
					let _turn = TurnGuard::register(&server, &session_id, &cancellation);

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
						server.sessions.lock().await.insert(session_id.clone(), chat_session);
						if cancellation.is_cancelled() {
							return;
						}
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
						server.sessions.lock().await.insert(session_id.clone(), chat_session);
						if cancellation.is_cancelled() {
							return;
						}
						continue;
					}

					let ws_sink = StreamSink {
						stream: Arc::clone(&stream),
						pending_work: false,
					};
					crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_sink.clone());

					let result = crate::supervisor::accounting::background(execute_api_call_and_process_response(
						&mut chat_session,
						&config_for_role,
						&server.role,
						op_rx,
						OutputMode::WebSocket,
						ws_sink,
					))
					.await;

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
					server.sessions.lock().await.insert(session_id.clone(), chat_session);
					if cancellation.is_cancelled() {
						return;
					}
				}
			})
			.await;

			// Exit if the session was unloaded (close, /exit, cleanup_session).
			if reaped.retire_if_unloaded() {
				log_debug!("WS monitor: session {} unloaded, exiting", session_id);
				break;
			}

			// Wait for the next event: schedule timer, inbox message, or schedule change.
			// next_schedule_sleep() handles the empty case (waits for schedule-change notify),
			// so no special polling branch is needed.
			crate::session::context::with_session_id(session_id.clone(), async {
				let inbox_notify = crate::session::inbox::get_inbox_notify();
				tokio::select! {
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
	let session_id = msg
		.session_id
		.clone()
		.unwrap_or_else(crate::session::chat::session::generate_session_name);
	let Some(guard) = lock_for_bind(&session_id, &ctx.server).await else {
		return attach_to_running_turn(&msg, session_id, ack, ws_sender, ctx, active_session_ids).await;
	};
	let mut reservation = match ctx.server.reserve_session(&session_id) {
		Ok(reservation) => reservation,
		Err(error) => return refuse_bind(ws_sender, ack, &ServerMessage::error(error.to_string())).await,
	};
	crate::mcp::process::set_notification_sender(
		Some(session_id.clone()),
		StreamSink {
			stream: ctx.server.streams.open(&session_id),
			pending_work: false,
		},
	);
	let bound = bind_session(&msg, cwd, session_id.clone(), ack, ws_sender, ctx, active_session_ids).await;
	if ctx.server.sessions.lock().await.contains_key(&session_id) {
		reservation.committed = true;
	} else if !reservation.committed {
		ctx.server.role_configs.lock().await.remove(&session_id);
		ctx.server.streams.close(&session_id);
		crate::session::context::cleanup_session(&session_id);
	}
	// Bound or refused, nothing sets the session up any more: a frame that drove it
	// meanwhile was answered busy, and an inbox monitor that tried waits to be woken.
	drop(guard);
	wake_inbox_monitor(session_id).await;
	bound
}

/// The session's lock for a bind, or `None` when a turn holds it.
///
/// A bind is serialized with every frame that drives the session (a turn, a
/// command, the inbox drain, a close, another bind): they take the session out of
/// memory while they hold it, and a bind that missed it there loaded a stale copy
/// from disk, set it up again and put it back — over the live session when the turn
/// ended meanwhile, so the next turn ran without the last exchange. Every holder
/// but a turn holds it for a setup at most (a disk load, a role's MCP servers),
/// so the bind waits for it. A turn holds the lock for its whole run, and the
/// session it drives is loaded and set up: the bind attaches.
async fn lock_for_bind(session_id: &str, server: &ServerState) -> Option<tokio::sync::OwnedMutexGuard<()>> {
	let lock = get_or_create_session_lock(session_id, &server.session_locks).await;
	loop {
		// Armed before the check, so a turn registering in between still wakes it.
		let turn_started = server.turn_started.notified();
		tokio::pin!(turn_started);
		turn_started.as_mut().enable();
		if server.turns.lock().unwrap().contains_key(session_id) {
			return None;
		}
		tokio::select! {
			guard = Arc::clone(&lock).lock_owned() => return Some(guard),
			_ = &mut turn_started => {}
		}
	}
}

/// Load or create the session a bind names (a new auto-named one without a name)
/// and bind to it.
async fn bind_session(
	msg: &SessionMessage,
	cwd: Option<PathBuf>,
	requested_id: String,
	ack: AckPayload,
	ws_sender: &WsSender,
	ctx: &ConnCtx,
	active_session_ids: &mut HashSet<String>,
) -> Result<()> {
	let (mut chat_session, config_for_role, session_role, is_new) = {
		let session_id = &requested_id;
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
	};

	let session_id = chat_session.session.info.name.clone();
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
		Ok::<String, anyhow::Error>(status_msg)
	})
	.await;
	let greeting = match setup {
		Ok(status_msg) => ServerMessage::status(status_msg, Some(session_id.clone())),
		Err(e) => {
			if !is_new {
				ctx.server.sessions.lock().await.insert(session_id, chat_session);
			}
			send_message(ws_sender, &ServerMessage::Ack(ack)).await?;
			return Err(e);
		}
	};
	ctx.server
		.sessions
		.lock()
		.await
		.insert(session_id.clone(), chat_session);
	bind_stream(msg, &session_id, ack, &greeting, &config_for_role, ws_sender, ctx)?;

	// Every bind of a loaded session passes here; the registry keeps it to one
	// monitor, however many connections bind it.
	active_session_ids.insert(session_id.clone());
	ensure_session_monitor(&ctx.server, &session_id);

	Ok(())
}

/// Bind to a session a turn is driving: the session is in the turn's hands, set up
/// for its role and directory, so nothing is loaded or set up again — nor is the
/// bind's `cwd` applied, which would move the running turn's directory under it.
/// Only this connection's own wiring is done (its run events; the session's
/// monitor runs already), and it reads the session's stream like any bind.
async fn attach_to_running_turn(
	msg: &SessionMessage,
	session_id: String,
	ack: AckPayload,
	ws_sender: &WsSender,
	ctx: &ConnCtx,
	active_session_ids: &mut HashSet<String>,
) -> Result<()> {
	log_debug!("Binding to session {} while a turn runs", session_id);
	// The turn ended and a close unloaded the session in between: refused like any
	// failed bind, so the client binds again rather than waiting on an ack.
	let Some(role) = crate::session::context::get_session_role(&session_id) else {
		let error = ServerMessage::error(format!("Session {} unloaded", session_id));
		return refuse_bind(ws_sender, ack, &error).await;
	};
	let config_for_role = role_config(&ctx.server.config, &ctx.server.role_configs, &session_id, &role).await;
	let wired = crate::session::context::with_session_id(session_id.clone(), async {
		crate::session::tap_runs::enable_run_events(Some(ws_sender.clone()))
	})
	.await;
	if let Err(e) = wired {
		let error = ServerMessage::error(format!("Session {} unloaded: {}", session_id, e));
		return refuse_bind(ws_sender, ack, &error).await;
	}
	let greeting = ServerMessage::status(format!("Session resumed: {}", session_id), Some(session_id.clone()));
	bind_stream(msg, &session_id, ack, &greeting, &config_for_role, ws_sender, ctx)?;
	active_session_ids.insert(session_id);
	Ok(())
}

/// Ack, resumed frames and greeting reach this connection before any live frame
/// of the session does — including those of a turn already running. Whether it
/// answers MCP elicitations follows output ownership (see `SessionStream::attach`).
fn bind_stream(
	msg: &SessionMessage,
	session_id: &str,
	mut ack: AckPayload,
	greeting: &ServerMessage,
	config_for_role: &Config,
	ws_sender: &WsSender,
	ctx: &ConnCtx,
) -> Result<()> {
	let answers_elicitation = msg.capabilities.iter().any(|c| c == ELICITATION_CAPABILITY);
	if msg.capabilities.iter().any(|c| c == COMMANDS_CAPABILITY) {
		ack.commands = Some(session_commands(config_for_role));
	}
	ctx.server
		.streams
		.open(session_id)
		.attach(ws_sender, ack, msg.resume.as_ref(), greeting, answers_elicitation)
}

/// What a `commands_v1` client's `/` menu lists, in `/help` order: the built-ins
/// a session can run, then the custom commands, named as `/help` lists them
/// under `/run`. Built per bind: custom commands follow the config it runs under.
fn session_commands(config: &Config) -> Vec<CommandInfo> {
	let bare = |name: &str| name.trim_start_matches('/').to_string();
	let builtins = BUILTIN_COMMANDS
		.iter()
		.filter(|&&(.., reach)| reach == Reach::Anywhere)
		.map(|&(name, hint, description, _)| CommandInfo {
			name: bare(name),
			description: description.to_string(),
			hint: (!hint.is_empty()).then(|| hint.to_string()),
		});
	// A custom command takes the rest of the line as its input (`run::handle_run`).
	let custom = config.commands.iter().flatten().map(|c| CommandInfo {
		name: format!("{} {}", bare(RUN_COMMAND), c.name),
		description: c.description.clone(),
		hint: Some("[input]".to_string()),
	});
	builtins.chain(custom).collect()
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
	role_config(base, role_configs, &session.session.info.name, &session.role).await
}

/// [`session_config`] for a session known by id and role, when it is out of reach.
async fn role_config(base: &Config, role_configs: &RoleConfigs, session_id: &str, role: &str) -> Config {
	let pinned = role_configs.lock().await.get(session_id).cloned();
	pinned.as_deref().unwrap_or(base).get_merged_config_for_role(role)
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
	let mut reservation = server.reserve_session(session_id).map_err(|error| error.to_string())?;
	crate::mcp::process::set_notification_sender(
		Some(session_id.to_string()),
		StreamSink {
			stream: server.streams.open(session_id),
			pending_work: false,
		},
	);

	log_debug!("Loading session from disk: {}", session_id);
	let loaded = async {
		let (config, role) = resume_role_config(session_id, server)
			.await
			.map_err(|e| e.to_string())?;
		let mut args = GenericSessionArgs::resume(session_id.to_string(), role);
		args.mode = "websocket".to_string();
		match setup_and_initialize_session(&args, &config, false).await {
			Ok((mut session, config_for_role, session_role, _, _)) => {
				crate::session::context::init_session_services(&session_role);
				crate::session::tap_runs::enable_run_events(None).map_err(|error| error.to_string())?;
				if let Err(e) =
					setup_system_prompt_and_cache(&mut session, &config_for_role, &session_role, false).await
				{
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
	.await;
	if loaded.is_ok() {
		reservation.committed = true;
		ensure_session_monitor(server, session_id);
	} else {
		server.role_configs.lock().await.remove(session_id);
		server.streams.close(session_id);
		crate::session::context::cleanup_session(&session_id.to_string());
	}
	loaded
}

/// The stream a client frame's turn emits on, as that turn begins. The client
/// that sent the frame reads its turn.
fn turn_stream(server: &ServerState, session_id: &str, ws_sender: &WsSender) -> Arc<SessionStream> {
	let stream = server.streams.open(session_id);
	stream.own(ws_sender);
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
	let _turn = TurnGuard::register(server, &session_id, &cancellation);

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
			server.role_configs.lock().await.remove(&session_id);
			server.residents.lock().unwrap().remove(&session_id);
			server.streams.close(&session_id);
			cleanup_and_wake_monitor(&session_id).await;
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
	let _turn = TurnGuard::register(server, &session_id, &cancellation);
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
		crate::mcp::orchestration::flush_due_to_inbox(&config_for_role);
		crate::mcp::orchestration::flush_idle_to_inbox(&config_for_role);
		loop {
			let batch = crate::session::inbox::drain_inbox_batch_before_user();
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
			let sink = StreamSink {
				stream: Arc::clone(&stream),
				pending_work: true,
			};
			let result = execute_api_call_and_process_response(
				&mut chat_session,
				&config_for_role,
				&server.role,
				op_rx,
				OutputMode::WebSocket,
				sink,
			)
			.await;
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

	let ws_sink = StreamSink {
		stream: Arc::clone(&stream),
		pending_work: false,
	};
	crate::mcp::process::set_notification_sender(Some(session_id.clone()), ws_sink.clone());
	let api_result = execute_api_call_and_process_response(
		&mut chat_session,
		&config_for_role,
		&server.role,
		operation_rx.clone(),
		OutputMode::WebSocket,
		ws_sink,
	)
	.await;

	// Save session
	log_debug!("Saving session: {}", session_id);
	let save_result = chat_session.save();
	let spending_stop = chat_session.spending_stop;
	let cancelled = *operation_rx.borrow();
	let mut cost = crate::session::chat::response::session_cost_payload(&mut chat_session, session_id.clone());
	cost.cancelled = cancelled;
	cost.request_id = msg.request_id.clone();
	let cost_msg = ServerMessage::Cost(cost);

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
