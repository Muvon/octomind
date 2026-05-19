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

//! Localhost bridge for `/analyze`.
//!
//! Per session we bind a tiny HTTP server on `127.0.0.1:0` (OS-picked port)
//! with a single endpoint:
//!
//!   GET /session  →  raw JSONL of the session file. Requires
//!                    `X-Bridge-Token: <token>` matching the per-invocation
//!                    random token.
//!
//! CORS is wide open (Allow-Origin: *) — the listener is bound to loopback
//! only, so only same-machine processes can reach it, and the random token
//! gates access. The browser at octomind.run reads the URL we print and
//! fetches the bridge directly; the bytes never traverse the public network.
//!
//! Lifecycle: `start_for_session` aborts any previous bridge for the session
//! and returns a handle describing the new one. Re-invoking `/analyze`
//! supersedes the old listener.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

use crate::session::context::SessionId;
use crate::{log_debug, log_error, log_info};

/// Returned to the caller so /analyze can print the URL it needs.
#[derive(Debug, Clone)]
pub struct BridgeInfo {
	pub port: u16,
	pub token: String,
}

/// Per-session entry in the registry. Only the abort handle is consulted at
/// runtime — its `Drop` is what shuts the listener down when the entry is
/// removed or replaced. We deliberately keep no other state here: port and
/// token are owned by the caller (returned in `BridgeInfo`).
struct BridgeHandle {
	abort: AbortHandle,
}

impl Drop for BridgeHandle {
	fn drop(&mut self) {
		self.abort.abort();
	}
}

static REGISTRY: OnceLock<Mutex<HashMap<SessionId, BridgeHandle>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<SessionId, BridgeHandle>> {
	REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start (or restart) the bridge for the current session, return the new port + token.
pub async fn start_for_session(session_file: PathBuf) -> Result<BridgeInfo> {
	let session_id = crate::session::context::current_session_id()
		.context("no session id in context — bridge needs an active session")?;

	let listener = TcpListener::bind(("127.0.0.1", 0))
		.await
		.context("failed to bind localhost bridge")?;
	let port = listener.local_addr()?.port();
	let token = random_token();
	let token_for_task = token.clone();

	let handle = tokio::spawn(async move {
		if let Err(e) = run_bridge(listener, session_file, token_for_task).await {
			log_error!("/analyze bridge exited: {}", e);
		}
	});

	let bridge = BridgeHandle {
		abort: handle.abort_handle(),
	};

	// Aborting the old handle is the Drop of the replaced value.
	registry().lock().insert(session_id, bridge);

	log_info!("/analyze bridge listening on 127.0.0.1:{}", port);
	Ok(BridgeInfo { port, token })
}

/// Drop the bridge for a session. Called from session cleanup.
pub fn clear_for_session(session_id: &SessionId) {
	registry().lock().remove(session_id);
}

fn random_token() -> String {
	use std::time::{SystemTime, UNIX_EPOCH};
	const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
	// 24 chars from a 62-char alphabet ≈ 142 bits of entropy. Plenty for a
	// localhost-only short-lived bridge.
	let mut seed = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_nanos() as u64)
		.unwrap_or(0)
		^ std::process::id() as u64;
	let mut out = String::with_capacity(24);
	for _ in 0..24 {
		// xorshift64* — fast inline PRNG, not cryptographic but the listener is
		// loopback-only and the token is single-use.
		seed ^= seed << 13;
		seed ^= seed >> 7;
		seed ^= seed << 17;
		let idx = (seed as usize) % ALPHABET.len();
		out.push(ALPHABET[idx] as char);
	}
	out
}

async fn run_bridge(listener: TcpListener, session_file: PathBuf, token: String) -> Result<()> {
	loop {
		let (stream, _) = listener.accept().await?;
		let io = TokioIo::new(stream);
		let session_file = session_file.clone();
		let token = token.clone();
		tokio::spawn(async move {
			let svc = service_fn(move |req: Request<Incoming>| {
				let session_file = session_file.clone();
				let token = token.clone();
				async move { handle(req, &session_file, &token).await }
			});
			if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
				log_debug!("/analyze bridge conn error: {}", e);
			}
		});
	}
}

async fn handle(
	req: Request<Incoming>,
	session_file: &std::path::Path,
	token: &str,
) -> Result<Response<Full<Bytes>>, Infallible> {
	let path = req.uri().path();

	// CORS preflight — browser hits this before the GET because of the
	// custom X-Bridge-Token header.
	if req.method() == Method::OPTIONS {
		return Ok(cors_preflight());
	}

	if req.method() != Method::GET {
		return Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "GET only\n"));
	}

	match path {
		"/health" => Ok(plain(StatusCode::OK, "ok\n")),
		"/session" => Ok(serve_session(req, session_file, token).await),
		_ => Ok(plain(StatusCode::NOT_FOUND, "Not found\n")),
	}
}

async fn serve_session(
	req: Request<Incoming>,
	session_file: &std::path::Path,
	token: &str,
) -> Response<Full<Bytes>> {
	let hdr = req
		.headers()
		.get("x-bridge-token")
		.and_then(|v| v.to_str().ok())
		.unwrap_or("");
	if !constant_time_eq(hdr.as_bytes(), token.as_bytes()) {
		return plain(StatusCode::UNAUTHORIZED, "Bad token\n");
	}

	let bytes = match tokio::fs::read(session_file).await {
		Ok(b) => b,
		Err(e) => {
			log_error!(
				"/analyze bridge: failed to read {}: {}",
				session_file.display(),
				e
			);
			return plain(
				StatusCode::INTERNAL_SERVER_ERROR,
				&format!("Read failed: {}\n", e),
			);
		}
	};

	let mut res = Response::new(Full::new(Bytes::from(bytes)));
	*res.status_mut() = StatusCode::OK;
	let h = res.headers_mut();
	h.insert("content-type", "application/x-ndjson".parse().unwrap());
	h.insert("cache-control", "no-store".parse().unwrap());
	h.insert("access-control-allow-origin", "*".parse().unwrap());
	res
}

fn cors_preflight() -> Response<Full<Bytes>> {
	let mut res = Response::new(Full::new(Bytes::new()));
	*res.status_mut() = StatusCode::NO_CONTENT;
	let h = res.headers_mut();
	h.insert("access-control-allow-origin", "*".parse().unwrap());
	h.insert(
		"access-control-allow-methods",
		"GET, OPTIONS".parse().unwrap(),
	);
	h.insert(
		"access-control-allow-headers",
		"x-bridge-token, content-type".parse().unwrap(),
	);
	h.insert("access-control-max-age", "600".parse().unwrap());
	res
}

fn plain(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
	let mut res = Response::new(Full::new(Bytes::from(body.to_owned())));
	*res.status_mut() = status;
	res.headers_mut()
		.insert("content-type", "text/plain; charset=utf-8".parse().unwrap());
	res.headers_mut()
		.insert("access-control-allow-origin", "*".parse().unwrap());
	res
}

/// Constant-time byte slice comparison — keeps the token check from leaking
/// timing info even though the loopback risk is low.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
	if a.len() != b.len() {
		return false;
	}
	let mut diff: u8 = 0;
	for i in 0..a.len() {
		diff |= a[i] ^ b[i];
	}
	diff == 0
}
