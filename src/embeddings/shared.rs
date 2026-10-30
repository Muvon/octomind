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

//! One model owner per run directory, shared by WebSocket and ACP processes.
//! A file lock serializes election through model initialization and endpoint
//! publication. Clients never load weights while a live owner serves them.

#[cfg(test)]
use crate::local_inference::MAX_SERVICE_LINE_BYTES;
use crate::local_inference::{election_lock, publish, read_line, write_line};
use anyhow::{bail, Context, Result};
use octolib::Tokenizer;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::task::AbortOnDropHandle;

const MAX_SERVICE_CLIENTS: usize = 16;
const MAX_BATCH_TEXTS: usize = 256;
const SERVICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Pause after a failed accept: errors like EMFILE persist until a client
/// disconnects, and retrying at once would spin a core and flood the log.
const ACCEPT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

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

/// Model facts clients cannot derive without loading the weights.
#[derive(Serialize, Deserialize)]
struct Hello {
	revision: String,
	/// Clients chunk with the owner's exact tokenizer, without its weights.
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

/// One short-lived connection per request makes a dead owner observable.
#[derive(Clone)]
pub(super) struct Client {
	addr: SocketAddr,
	token: String,
}

impl Client {
	async fn connect(&self) -> Result<(BufReader<TcpStream>, Hello)> {
		tokio::time::timeout(SERVICE_TIMEOUT, async {
			let stream = TcpStream::connect(self.addr).await?;
			stream.set_nodelay(true)?;
			let mut reader = BufReader::new(stream);
			write_line(
				reader.get_mut(),
				&Auth {
					token: self.token.clone(),
				},
			)
			.await?;
			let line = read_line(&mut reader).await?.ok_or_else(|| {
				std::io::Error::new(
					std::io::ErrorKind::UnexpectedEof,
					"embedding service closed during handshake",
				)
			})?;
			let hello = serde_json::from_str(&line)?;
			Ok((reader, hello))
		})
		.await
		.context("embedding service handshake timed out")?
	}

	pub(super) async fn embed_many(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
		let mut vectors = Vec::with_capacity(texts.len());
		for batch in texts.chunks(MAX_BATCH_TEXTS) {
			let response = tokio::time::timeout(SERVICE_TIMEOUT, async {
				let (mut reader, _) = self.connect().await?;
				write_line(reader.get_mut(), &Request { texts: batch.to_vec() }).await?;
				let line = read_line(&mut reader).await?.ok_or_else(|| {
					std::io::Error::new(
						std::io::ErrorKind::UnexpectedEof,
						"embedding service closed before responding",
					)
				})?;
				let response: Response = serde_json::from_str(&line)?;
				match (response.vectors, response.error) {
					(Some(vectors), None) => {
						anyhow::ensure!(
							vectors.len() == batch.len(),
							"embedding service returned the wrong vector count"
						);
						Ok(vectors)
					}
					(None, Some(error)) => bail!("embedding service error: {error}"),
					_ => bail!("embedding service returned an invalid response"),
				}
			})
			.await
			.context("embedding service request timed out")??;
			vectors.extend(response);
		}
		Ok(vectors)
	}
}

pub(super) enum Role {
	/// Owns the serving task as well as the weights' process lifetime.
	Owner(AbortOnDropHandle<()>),
	Client(Client),
}

pub(super) struct Elected {
	pub(super) role: Role,
	pub(super) revision: String,
	pub(super) tokenizer: Arc<Tokenizer>,
}

impl Elected {
	pub(super) fn is_live(&self) -> bool {
		match &self.role {
			Role::Owner(task) => !task.is_finished(),
			Role::Client(_) => true,
		}
	}
}

pub(super) fn is_disconnected(error: &anyhow::Error) -> bool {
	error.downcast_ref::<std::io::Error>().is_some_and(|error| {
		matches!(
			error.kind(),
			std::io::ErrorKind::ConnectionRefused
				| std::io::ErrorKind::ConnectionReset
				| std::io::ErrorKind::ConnectionAborted
				| std::io::ErrorKind::BrokenPipe
				| std::io::ErrorKind::NotConnected
				| std::io::ErrorKind::UnexpectedEof
		)
	})
}

/// Join the shared embedding service, becoming its owner if nobody else is.
pub(super) async fn join() -> Result<Elected> {
	join_at(&endpoint_path()?).await
}

async fn join_at(path: &Path) -> Result<Elected> {
	let _election = election_lock(path).await?;
	let existing = match std::fs::read_to_string(path) {
		Ok(raw) => Some(serde_json::from_str::<Endpoint>(&raw).context("malformed embedding endpoint")?),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
		Err(error) => return Err(error.into()),
	};
	if let Some(existing) = existing {
		anyhow::ensure!(
			existing.model == super::MODEL_NAME,
			"embedding endpoint advertises model {}",
			existing.model
		);
		let client = Client {
			addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, existing.port)),
			token: existing.token,
		};
		match client.connect().await {
			Ok((_, hello)) => {
				let tokenizer = hello
					.tokenizer
					.parse()
					.map_err(|error| anyhow::anyhow!("invalid shared tokenizer: {error}"))?;
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
			Err(error) if is_disconnected(&error) => std::fs::remove_file(path)?,
			Err(error) => return Err(error),
		}
	}

	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
	let endpoint = Endpoint {
		port: listener.local_addr()?.port(),
		token: format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()),
		model: super::MODEL_NAME.to_string(),
		pid: std::process::id(),
	};
	let model = super::model().await?;
	let hello = Hello {
		revision: model.revision.clone(),
		tokenizer: model
			.tokenizer
			.to_string(false)
			.map_err(|error| anyhow::anyhow!("failed to serialize tokenizer: {error}"))?,
	};
	let task = serve(listener, endpoint.token.clone(), hello)?;
	publish(path, &endpoint)?;
	crate::log_debug!(
		"embeddings: elected owner of the shared model on port {}",
		endpoint.port
	);
	Ok(Elected {
		role: Role::Owner(task),
		revision: model.revision.clone(),
		tokenizer: Arc::clone(&model.tokenizer),
	})
}

fn serve(listener: TcpListener, token: String, hello: Hello) -> Result<AbortOnDropHandle<()>> {
	let hello = Arc::new(serde_json::to_string(&hello)?);
	let token = Arc::new(token);
	Ok(AbortOnDropHandle::new(tokio::spawn(async move {
		let mut clients = tokio::task::JoinSet::new();
		loop {
			tokio::select! {
				Some(joined) = clients.join_next() => {
					if let Err(error) = joined { crate::log_error!("Embedding service task failed: {}", error); }
				}
				accepted = listener.accept(), if clients.len() < MAX_SERVICE_CLIENTS => {
					let (stream, _) = match accepted {
						Ok(accepted) => accepted,
						Err(error) => {
							crate::log_error!("Embedding service accept failed: {}", error);
							tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
							continue;
						}
					};
					let token = Arc::clone(&token);
					let hello = Arc::clone(&hello);
					clients.spawn(async move {
						match tokio::time::timeout(SERVICE_TIMEOUT, handle(stream, &token, &hello)).await {
							Ok(Ok(())) => {}
							Ok(Err(error)) => crate::log_debug!("Embedding service client failed: {}", error),
							Err(error) => crate::log_debug!("Embedding service client timed out: {}", error),
						}
					});
				}
			}
		}
	})))
}

async fn handle(stream: TcpStream, token: &str, hello: &str) -> Result<()> {
	stream.set_nodelay(true)?;
	let mut reader = BufReader::new(stream);
	let Some(line) = read_line(&mut reader).await? else {
		return Ok(());
	};
	let auth: Auth = serde_json::from_str(&line)?;
	anyhow::ensure!(auth.token == token, "embedding service rejected a bad token");
	reader.get_mut().write_all(hello.as_bytes()).await?;
	reader.get_mut().write_all(b"\n").await?;
	while let Some(line) = read_line(&mut reader).await? {
		let request: Request = serde_json::from_str(&line)?;
		anyhow::ensure!(
			request.texts.len() <= MAX_BATCH_TEXTS,
			"embedding service batch exceeds text limit"
		);
		let model = super::model().await?;
		super::ensure_disk_cache_loaded(&model.revision);
		let response = match super::embed_many_cached(&request.texts, &model.revision, super::Inference::Local).await {
			Ok(vectors) => Response {
				vectors: Some(vectors),
				error: None,
			},
			Err(error) => Response {
				vectors: None,
				error: Some(error.to_string()),
			},
		};
		write_line(reader.get_mut(), &response).await?;
	}
	Ok(())
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
