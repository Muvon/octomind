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

//! One model per machine, shared by every octomind process.
//!
//! Candle owns its tensors in heap `Vec`s, so N processes that each load
//! `muvon/octomind-embed` hold N × ~90 MB of private, unshareable weights.
//! mmap/shm cannot fix that at the storage layer. The only real "load once"
//! is one owner process doing inference for the others.
//!
//! Election is a file create race on `<run_dir>/embed.json`: the winner binds
//! a loopback listener, loads the weights and serves; everyone else reads the
//! endpoint and connects, never touching the model. Loopback TCP rather than a
//! Unix socket so Linux/macOS/Windows run one code path.
//!
//! The owner is a normal octomind process. When it exits its weights go with
//! it and the next embed re-elects (one-time ~1 s reload) — no daemon, no
//! lifetime to manage.

use anyhow::{bail, Context, Result};
use octolib::Tokenizer;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Endpoint descriptor written by the owner, read by clients.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
struct Endpoint {
	port: u16,
	token: String,
	/// Guards against a stale file from a different model build.
	model: String,
	/// Informational: which process owns the weights.
	pid: u32,
}

/// Server's first line after a client authenticates: the two model facts a
/// client needs but cannot derive without the weights.
#[derive(Serialize, Deserialize)]
struct Hello {
	revision: String,
	/// Serialized `tokenizer.json` — a few hundred KB, no weights, so clients
	/// chunk text exactly as the owner's model does.
	tokenizer: String,
}

#[derive(Serialize, Deserialize)]
struct Auth {
	token: String,
}

#[derive(Serialize, Deserialize)]
struct Request {
	texts: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Response {
	vectors: Option<Vec<Vec<f32>>>,
	error: Option<String>,
}

fn endpoint_path() -> Result<PathBuf> {
	Ok(crate::directories::get_run_dir()?.join("embed.json"))
}

/// Handle to the owner's service. Cheap to clone; one short-lived connection
/// per request, so a dead owner surfaces as a connect error instead of a
/// silently broken pooled socket.
#[derive(Clone)]
pub(super) struct Client {
	addr: SocketAddr,
	token: String,
}

impl Client {
	async fn connect(&self) -> Result<(BufReader<TcpStream>, Hello)> {
		let stream = TcpStream::connect(self.addr).await?;
		stream.set_nodelay(true)?;
		let mut reader = BufReader::new(stream);
		let auth = serde_json::to_string(&Auth {
			token: self.token.clone(),
		})?;
		reader.get_mut().write_all(auth.as_bytes()).await?;
		reader.get_mut().write_all(b"\n").await?;
		let mut line = String::new();
		if reader.read_line(&mut line).await? == 0 {
			bail!("embedding service closed the connection during handshake");
		}
		let hello: Hello = serde_json::from_str(line.trim())?;
		Ok((reader, hello))
	}

	pub(super) async fn embed_many(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
		let (mut reader, _) = self.connect().await?;
		let req = serde_json::to_string(&Request {
			texts: texts.to_vec(),
		})?;
		reader.get_mut().write_all(req.as_bytes()).await?;
		reader.get_mut().write_all(b"\n").await?;
		let mut line = String::new();
		if reader.read_line(&mut line).await? == 0 {
			bail!("embedding service closed the connection before responding");
		}
		let resp: Response = serde_json::from_str(line.trim())?;
		match (resp.vectors, resp.error) {
			(Some(v), _) => Ok(v),
			(None, Some(e)) => bail!("embedding service error: {e}"),
			(None, None) => bail!("embedding service returned an empty response"),
		}
	}
}

/// Outcome of the election.
pub(super) enum Role {
	/// This process owns the weights and answers other processes.
	Owner,
	/// Another process owns the weights; use its service.
	Client(Client),
}

pub(super) struct Elected {
	pub(super) role: Role,
	pub(super) revision: String,
	pub(super) tokenizer: Arc<Tokenizer>,
}

/// Join the shared embedding service, becoming its owner if nobody else is.
///
/// Bounded retry: a stale endpoint file (owner killed without cleanup) is
/// removed and the election re-run. Two processes can briefly both own the
/// model if they race the removal; that self-heals on the next start and costs
/// one extra copy, which is strictly better than failing the embed.
pub(super) async fn join() -> Result<Elected> {
	const ATTEMPTS: usize = 3;
	let path = endpoint_path()?;
	let mut last_err = None;

	for _ in 0..ATTEMPTS {
		// Bind before claiming: the file must never advertise a port that
		// isn't listening yet, or a fast client gets a connection refused.
		let listener =
			TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).await?;
		let port = listener.local_addr()?.port();
		let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()).replace('-', "");
		let endpoint = Endpoint {
			port,
			token: token.clone(),
			model: super::MODEL_NAME.to_string(),
			pid: std::process::id(),
		};

		match claim(&path, &endpoint) {
			Ok(true) => {
				// We own it: load the weights, then start answering.
				let model = super::model().await?;
				let tokenizer_json = model
					.tokenizer
					.to_string(false)
					.map_err(|e| anyhow::anyhow!("failed to serialize tokenizer: {e}"))?;
				let hello = Hello {
					revision: model.revision.clone(),
					tokenizer: tokenizer_json,
				};
				serve(listener, token, hello);
				crate::log_debug!(
					"embeddings: elected owner of the shared model on port {}",
					port
				);
				return Ok(Elected {
					role: Role::Owner,
					revision: model.revision.clone(),
					tokenizer: model.tokenizer.clone(),
				});
			}
			Ok(false) => {
				drop(listener);
				let existing = read_endpoint(&path)?;
				let client = Client {
					addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, existing.port)),
					token: existing.token.clone(),
				};
				match client.connect().await {
					Ok((_, hello)) => {
						let tokenizer: Tokenizer = hello.tokenizer.parse().map_err(|e| {
							anyhow::anyhow!(
								"shared embedding service sent an unusable tokenizer: {e}"
							)
						})?;
						crate::log_debug!(
							"embeddings: using shared model owned by pid {} (no local weights loaded)",
							existing.pid
						);
						return Ok(Elected {
							role: Role::Client(client),
							revision: hello.revision,
							tokenizer: Arc::new(tokenizer),
						});
					}
					Err(e) => {
						// Owner is gone. Drop its claim only if nobody has
						// replaced it since we read it, then re-elect.
						remove_if_unchanged(&path, &existing);
						last_err = Some(e);
					}
				}
			}
			Err(e) => {
				drop(listener);
				last_err = Some(e);
			}
		}
	}

	Err(last_err.unwrap_or_else(|| anyhow::anyhow!("embedding service election failed")))
}

/// Atomically claim ownership. `Ok(false)` means someone else holds it.
fn claim(path: &PathBuf, endpoint: &Endpoint) -> Result<bool> {
	use std::io::Write;

	let mut opts = std::fs::OpenOptions::new();
	opts.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		// The token is an auth secret for a loopback port: owner-only.
		opts.mode(0o600);
	}
	match opts.open(path) {
		Ok(mut f) => {
			f.write_all(serde_json::to_string(endpoint)?.as_bytes())?;
			f.flush()?;
			Ok(true)
		}
		Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
		Err(e) => Err(e).context(format!("failed to claim {}", path.display())),
	}
}

fn read_endpoint(path: &PathBuf) -> Result<Endpoint> {
	let raw = std::fs::read_to_string(path)
		.with_context(|| format!("failed to read {}", path.display()))?;
	let endpoint: Endpoint = serde_json::from_str(&raw)
		.with_context(|| format!("malformed endpoint file {}", path.display()))?;
	if endpoint.model != super::MODEL_NAME {
		bail!("endpoint file advertises model {}", endpoint.model);
	}
	Ok(endpoint)
}

/// Delete the endpoint file only if it still holds `expected`, so we never
/// evict an owner that was elected between our read and this call.
fn remove_if_unchanged(path: &PathBuf, expected: &Endpoint) {
	if matches!(read_endpoint(path), Ok(current) if current == *expected) {
		let _ = std::fs::remove_file(path);
	}
}

/// Answer embed requests until the process exits.
fn serve(listener: TcpListener, token: String, hello: Hello) {
	let hello = Arc::new(match serde_json::to_string(&hello) {
		Ok(s) => s,
		Err(e) => {
			crate::log_debug!("embeddings: cannot serve shared model: {}", e);
			return;
		}
	});
	let token = Arc::new(token);
	tokio::spawn(async move {
		loop {
			let Ok((stream, _)) = listener.accept().await else {
				continue;
			};
			let token = token.clone();
			let hello = hello.clone();
			tokio::spawn(async move {
				if let Err(e) = handle(stream, &token, &hello).await {
					crate::log_debug!("embeddings: shared model client dropped: {}", e);
				}
			});
		}
	});
}

async fn handle(stream: TcpStream, token: &str, hello: &str) -> Result<()> {
	stream.set_nodelay(true)?;
	let mut reader = BufReader::new(stream);
	let mut line = String::new();
	if reader.read_line(&mut line).await? == 0 {
		return Ok(());
	}
	let auth: Auth = serde_json::from_str(line.trim())?;
	if auth.token != token {
		bail!("rejected client with a bad token");
	}
	reader.get_mut().write_all(hello.as_bytes()).await?;
	reader.get_mut().write_all(b"\n").await?;

	loop {
		line.clear();
		if reader.read_line(&mut line).await? == 0 {
			return Ok(());
		}
		let req: Request = serde_json::from_str(line.trim())?;
		// Goes through the owner's normal path, so remote requests share the
		// same in-memory and on-disk caches as the owner's own embeds.
		let resp = match super::embed_many(&req.texts).await {
			Ok(vectors) => Response {
				vectors: Some(vectors),
				error: None,
			},
			Err(e) => Response {
				vectors: None,
				error: Some(e.to_string()),
			},
		};
		let body = serde_json::to_string(&resp)?;
		reader.get_mut().write_all(body.as_bytes()).await?;
		reader.get_mut().write_all(b"\n").await?;
	}
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
