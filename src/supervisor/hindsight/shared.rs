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

use super::{fingerprint, Cached};
use crate::local_inference::{election_lock, publish, read_line, write_line};
use anyhow::{bail, Context, Result};
use octolib::hindsight::{trace::Event, Hindsight, Scores, Session};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions, TryLockError};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

const PROTOCOL: u32 = 1;
const MAX_CLIENTS: usize = 16;
const MAX_EVENTS: usize = 4096;
const MAX_CACHED_SESSIONS: usize = 32;
const MAX_CACHED_EVENTS: usize = 8192;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(600);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Serialize, Deserialize, Clone)]
struct Endpoint {
	protocol: u32,
	model: String,
	port: u16,
	token: String,
	pid: u32,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
	token: String,
	request: Request,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
	Ping,
	Score { session: String, events: Vec<Event> },
	Shutdown,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Response {
	Ready { model: String, pid: u32 },
	Scores { scores: Scores, events: usize },
	Stopping,
}

type Reply = std::result::Result<Response, String>;

fn canonical_model(spec: &str) -> Result<String> {
	let spec = spec.trim();
	if Path::new(spec).is_dir() {
		return Ok(std::fs::canonicalize(spec)?
			.to_str()
			.context("Hindsight model path is not UTF-8")?
			.to_owned());
	}
	let (repo, prefix) = spec
		.split_once('#')
		.map_or((spec, "onnx"), |(r, p)| (r.trim(), p.trim()));
	anyhow::ensure!(
		!repo.is_empty() && !prefix.is_empty(),
		"Hindsight model repository and prefix must not be empty"
	);
	Ok(format!("{repo}#{prefix}"))
}

fn endpoint_path(spec: &str) -> Result<PathBuf> {
	let key = hex::encode(Sha256::digest(format!("hindsight-{PROTOCOL}:{spec}")));
	Ok(crate::directories::get_run_dir()?.join(format!("hindsight-{key}.json")))
}

fn owner_lock(path: &Path) -> Result<File> {
	let mut options = OpenOptions::new();
	options.read(true).write(true).create(true).truncate(false);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		options.mode(0o600);
	}
	Ok(options.open(path.with_extension("owner"))?)
}

#[derive(Clone)]
struct Client(Endpoint);

impl Client {
	async fn request(&self, request: Request) -> Result<Response> {
		tokio::time::timeout(REQUEST_TIMEOUT, async {
			let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.0.port));
			let stream = TcpStream::connect(address).await?;
			stream.set_nodelay(true)?;
			let mut reader = BufReader::new(stream);
			write_line(
				reader.get_mut(),
				&Envelope {
					token: self.0.token.clone(),
					request,
				},
			)
			.await?;
			let line = read_line(&mut reader).await?.ok_or_else(|| {
				std::io::Error::new(
					std::io::ErrorKind::UnexpectedEof,
					"Hindsight service closed before responding",
				)
			})?;
			let reply: Reply = serde_json::from_str(&line)?;
			reply.map_err(anyhow::Error::msg)
		})
		.await
		.context("Hindsight service request timed out")?
	}

	async fn ready(&self) -> Result<()> {
		match self.request(Request::Ping).await? {
			Response::Ready { model, pid } if model == self.0.model && pid == self.0.pid => Ok(()),
			_ => bail!("Hindsight endpoint returned inconsistent owner identity"),
		}
	}
}

fn worker_command(spec: &str, _path: &Path) -> Result<Command> {
	let mut command = Command::new(std::env::current_exe()?);
	command
		.args(["hindsight-worker", "--model", spec])
		.current_dir(crate::directories::get_run_dir()?);
	#[cfg(unix)]
	command.process_group(0);
	#[cfg(windows)]
	{
		const CREATE_NO_WINDOW: u32 = 0x0800_0000;
		command.creation_flags(CREATE_NO_WINDOW);
	}
	Ok(command)
}

async fn join_at(path: &Path, spec: &str) -> Result<Client> {
	let _startup = election_lock(path).await?;
	match std::fs::read(path) {
		Ok(raw) => {
			let endpoint: Endpoint = serde_json::from_slice(&raw).context("Malformed Hindsight endpoint")?;
			anyhow::ensure!(
				endpoint.protocol == PROTOCOL && endpoint.model == spec,
				"Incompatible Hindsight endpoint"
			);
			let client = Client(endpoint);
			match client.ready().await {
				Ok(()) => return Ok(client),
				Err(error) if disconnected(&error) => {}
				Err(error) => return Err(error),
			}
		}
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
		Err(error) => return Err(error.into()),
	}
	let lease = owner_lock(path)?;
	match lease.try_lock() {
		Ok(()) => drop(lease),
		Err(TryLockError::WouldBlock) => {
			bail!("Hindsight owner is still starting or stopping; no second model will be loaded")
		}
		Err(TryLockError::Error(error)) => return Err(error.into()),
	}
	let mut command = worker_command(spec, path)?;
	command
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::inherit())
		.kill_on_drop(false);
	let mut child = command.spawn().context("Starting the Hindsight model worker")?;
	let stdout = child.stdout.take().context("Hindsight worker stdout was not piped")?;
	let mut reader = BufReader::new(stdout);
	let line = tokio::time::timeout(STARTUP_TIMEOUT, read_line(&mut reader))
		.await
		.context("Hindsight worker initialization timed out")??
		.context("Hindsight worker exited before becoming ready")?;
	let ready: std::result::Result<(), String> =
		serde_json::from_str(&line).context("Invalid Hindsight worker readiness")?;
	ready.map_err(anyhow::Error::msg)?;
	let endpoint: Endpoint = serde_json::from_slice(&std::fs::read(path)?)?;
	anyhow::ensure!(
		endpoint.protocol == PROTOCOL && endpoint.model == spec,
		"Hindsight worker published an inconsistent endpoint"
	);
	let client = Client(endpoint);
	client.ready().await?;
	Ok(client)
}

fn disconnected(error: &anyhow::Error) -> bool {
	error
		.chain()
		.find_map(|cause| cause.downcast_ref::<std::io::Error>())
		.is_some_and(|error| {
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

pub async fn score(spec: &str, session: &str, events: Vec<Event>) -> Result<(Scores, usize)> {
	let spec = canonical_model(spec)?;
	let path = endpoint_path(&spec)?;
	score_at(&path, &spec, session, events).await
}

async fn score_at(path: &Path, spec: &str, session: &str, events: Vec<Event>) -> Result<(Scores, usize)> {
	anyhow::ensure!(
		events.len() <= MAX_EVENTS,
		"Hindsight request exceeds {MAX_EVENTS} events"
	);
	let client = join_at(path, spec).await?;
	let make_request = || Request::Score {
		session: session.to_owned(),
		events: events.clone(),
	};
	let response = match client.request(make_request()).await {
		Err(error) if disconnected(&error) => join_at(path, spec).await?.request(make_request()).await?,
		result => result?,
	};
	match response {
		Response::Scores { scores, events } => Ok((scores, events)),
		_ => bail!("Hindsight service returned the wrong response"),
	}
}

struct Worker {
	model: Arc<Hindsight>,
	traces: Vec<(String, Cached)>,
	// Held until the last inference finishes and all model references are dropped.
	_lease: File,
}

impl Worker {
	fn score(&mut self, name: String, events: Vec<Event>) -> Result<(Scores, usize)> {
		anyhow::ensure!(
			!events.is_empty() && events.len() <= MAX_EVENTS,
			"Invalid Hindsight event count"
		);
		let mut cached = match self.traces.iter().position(|(session, _)| *session == name) {
			Some(index) => self.traces.remove(index).1,
			None => Cached {
				session: Session::new(self.model.clone()),
				consumed: 0,
				fingerprint: fingerprint(&[])?,
			},
		};
		if cached.consumed > events.len() || fingerprint(&events[..cached.consumed])? != cached.fingerprint {
			cached = Cached {
				session: Session::new(self.model.clone()),
				consumed: 0,
				fingerprint: fingerprint(&[])?,
			};
		}
		while self.traces.len() >= MAX_CACHED_SESSIONS
			|| self.traces.iter().map(|(_, trace)| trace.consumed).sum::<usize>() + events.len() > MAX_CACHED_EVENTS
		{
			self.traces.remove(0);
		}
		for event in &events[cached.consumed..] {
			cached.session.push(event)?;
		}
		cached.consumed = events.len();
		cached.fingerprint = fingerprint(&events)?;
		let scores = cached.session.score()?;
		self.traces.push((name, cached));
		Ok((scores, events.len()))
	}
}

pub async fn serve(spec: &str) -> Result<()> {
	let spec = canonical_model(spec)?;
	serve_at(&endpoint_path(&spec)?, &spec).await
}

async fn serve_at(path: &Path, spec: &str) -> Result<()> {
	let lease = owner_lock(path)?;
	lease
		.try_lock()
		.map_err(|error| anyhow::anyhow!("Another Hindsight owner already holds the model: {error}"))?;
	let model = match Hindsight::load(spec).await {
		Ok(model) => Arc::new(model),
		Err(error) => {
			write_line(&mut tokio::io::stdout(), &Err::<(), _>(format!("{error:#}"))).await?;
			return Err(error);
		}
	};
	let state = Arc::new(Mutex::new(Worker {
		model,
		traces: Vec::new(),
		_lease: lease,
	}));
	let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
	let endpoint = Endpoint {
		protocol: PROTOCOL,
		model: spec.to_owned(),
		port: listener.local_addr()?.port(),
		token: format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()),
		pid: std::process::id(),
	};
	publish(path, &endpoint)?;
	// The initiating client may be cancelled during model loading; other clients
	// still discover this owner from its published endpoint.
	match write_line(&mut tokio::io::stdout(), &Ok::<(), String>(())).await {
		Ok(()) => {}
		Err(error)
			if error
				.downcast_ref::<std::io::Error>()
				.is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe) => {}
		Err(error) => return Err(error),
	}
	let (stop, mut stopped) = tokio::sync::watch::channel(false);
	let permit = Arc::new(tokio::sync::Semaphore::new(1));
	let mut clients = tokio::task::JoinSet::new();
	let mut last_activity = tokio::time::Instant::now();
	loop {
		tokio::select! {
			changed = stopped.changed() => {
				changed.context("Hindsight worker stop channel closed")?;
				if *stopped.borrow() { break; }
			}
			_ = tokio::time::sleep_until(last_activity + IDLE_TIMEOUT), if clients.is_empty() => break,
			joined = clients.join_next(), if !clients.is_empty() => {
				if let Some(joined) = joined {
					if let Err(error) = joined.context("Hindsight request task panicked")? {
						crate::log_debug!("Hindsight client request failed: {:#}", error);
					}
				}
				last_activity = tokio::time::Instant::now();
			}
			accepted = listener.accept(), if clients.len() < MAX_CLIENTS => {
				let (stream,_) = accepted?;
				last_activity = tokio::time::Instant::now();
				let endpoint = endpoint.clone();
				let state = state.clone();
				let permit = permit.clone();
				let stop = stop.clone();
				clients.spawn(async move {
					match tokio::time::timeout(REQUEST_TIMEOUT, handle(stream,endpoint,state,permit,stop)).await {
						Ok(result) => result,
						Err(_) => Err(anyhow::anyhow!("Hindsight client request timed out")),
					}
				});
			}
		}
	}
	drop(listener);
	clients.abort_all();
	while let Some(joined) = clients.join_next().await {
		match joined {
			Ok(Err(error)) => crate::log_debug!("Hindsight client disconnected during shutdown: {:#}", error),
			Ok(Ok(())) => {}
			Err(error) if error.is_cancelled() => {}
			Err(error) => return Err(error.into()),
		}
	}
	std::fs::remove_file(path)?;
	Ok(())
}

async fn handle(
	stream: TcpStream,
	endpoint: Endpoint,
	state: Arc<Mutex<Worker>>,
	permit: Arc<tokio::sync::Semaphore>,
	stop: tokio::sync::watch::Sender<bool>,
) -> Result<()> {
	stream.set_nodelay(true)?;
	let mut reader = BufReader::new(stream);
	let Some(line) = read_line(&mut reader).await? else {
		return Ok(());
	};
	let envelope: Envelope = serde_json::from_str(&line)?;
	anyhow::ensure!(
		envelope.token == endpoint.token,
		"Hindsight service rejected an invalid token"
	);
	let reply: Reply = match envelope.request {
		Request::Ping => Ok(Response::Ready {
			model: endpoint.model,
			pid: endpoint.pid,
		}),
		Request::Shutdown => {
			write_line(reader.get_mut(), &Ok::<_, String>(Response::Stopping)).await?;
			stop.send(true).context("Hindsight worker stopped before shutdown")?;
			return Ok(());
		}
		Request::Score { session, events } => {
			if events.is_empty() || events.len() > MAX_EVENTS {
				Err("Hindsight request has an invalid event count".into())
			} else {
				let permit = permit.acquire_owned().await?;
				tokio::task::spawn_blocking(move || {
					let _permit = permit;
					state
						.lock()
						.map_err(|_| anyhow::anyhow!("Hindsight worker state poisoned"))?
						.score(session, events)
				})
				.await
				.context("Hindsight inference task panicked")?
				.map(|(scores, events)| Response::Scores { scores, events })
				.map_err(|error| format!("{error:#}"))
			}
		}
	};
	write_line(reader.get_mut(), &reply).await
}
