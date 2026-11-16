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

//! A plugin in memory, for tests of the host and of the code that calls it.
//!
//! [`connect_stub`] switches a stub on for one session of the process-wide
//! host, the one the functions of `crate::plugins` use. Sessions in tests have
//! unique ids, so tests run in parallel on that host; each stub has a unique
//! plugin name for the same reason.

use super::protocol::{InitializeResult, ProgramInfo, PROTOCOL_VERSION};
use super::transport::{Connector, ProcessConnector, Transport};
use crate::config::PluginConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

type Responder = Box<dyn Fn(&str, &Value) -> Result<Value> + Send + Sync>;

/// Time limit of a stub plugin's events, in seconds. Short, so a test that
/// makes a stub slow does not wait long.
const STUB_TIMEOUT_SECONDS: u64 = 5;

struct StubInner {
	name: String,
	init: Value,
	responder: Responder,
	queue: parking_lot::Mutex<VecDeque<Value>>,
	delay: Duration,
	log: parking_lot::Mutex<Vec<(String, Value)>>,
	connected: AtomicBool,
}

/// A plugin that records everything it is sent. Clones share one recording.
#[derive(Clone)]
pub struct StubPlugin {
	inner: Arc<StubInner>,
}

impl StubPlugin {
	/// A stub that subscribes to `subscribe` and answers every event with an empty answer.
	pub fn new(subscribe: &[&str]) -> Self {
		static COUNTER: AtomicUsize = AtomicUsize::new(0);
		let result = InitializeResult {
			protocol: PROTOCOL_VERSION,
			plugin: ProgramInfo {
				name: "stub".into(),
				version: "0".into(),
			},
			subscribe: subscribe.iter().map(|name| name.to_string()).collect(),
			options: Default::default(),
			status_fields: Vec::new(),
			commands: Vec::new(),
			timeouts: Default::default(),
		};
		let mut stub = Self::from_result(&result);
		stub.configure().name = format!("stub-{}", COUNTER.fetch_add(1, Ordering::SeqCst));
		stub
	}

	/// A stub that answers `initialize` with `result`.
	pub fn from_result(result: &InitializeResult) -> Self {
		Self {
			inner: Arc::new(StubInner {
				name: "stub".into(),
				init: serde_json::to_value(result).expect("initialize result serializes"),
				responder: Box::new(|_, _| Ok(json!({}))),
				queue: parking_lot::Mutex::new(VecDeque::new()),
				delay: Duration::ZERO,
				log: parking_lot::Mutex::new(Vec::new()),
				connected: AtomicBool::new(true),
			}),
		}
	}

	fn configure(&mut self) -> &mut StubInner {
		Arc::get_mut(&mut self.inner).expect("a stub is configured before it is connected")
	}

	/// Answer each event with `responder(method, params)` once the queue is empty.
	pub fn respond(mut self, responder: impl Fn(&str, &Value) -> Result<Value> + Send + Sync + 'static) -> Self {
		self.configure().responder = Box::new(responder);
		self
	}

	/// Answer every event with this answer once the queue is empty.
	pub fn answer(self, answer: Value) -> Self {
		self.respond(move |_, _| Ok(answer.clone()))
	}

	/// Answer the next events with these answers, oldest first.
	pub fn queue(self, answers: impl IntoIterator<Item = Value>) -> Self {
		self.inner.queue.lock().extend(answers);
		self
	}

	/// Set one field of the answer to `initialize`.
	pub fn init_field(mut self, key: &str, value: Value) -> Self {
		self.configure().init[key] = value;
		self
	}

	/// Wait this long before answering an event.
	pub fn delay(mut self, delay: Duration) -> Self {
		self.configure().delay = delay;
		self
	}

	/// The plugin name the stub is configured under.
	pub fn name(&self) -> &str {
		&self.inner.name
	}

	/// `params` of everything sent with `method`, oldest first.
	pub fn sent(&self, method: &str) -> Vec<Value> {
		self.inner
			.log
			.lock()
			.iter()
			.filter(|(name, _)| name == method)
			.map(|(_, params)| params.clone())
			.collect()
	}

	/// Every request and notification received, as `(method, params)`, oldest first.
	pub fn received(&self) -> Vec<(String, Value)> {
		self.inner.log.lock().clone()
	}

	/// The connection is gone: the host connects again for the next session.
	pub fn disconnect(&self) {
		self.inner.connected.store(false, Ordering::SeqCst);
	}
}

#[async_trait]
impl Transport for StubPlugin {
	async fn request(&self, method: &str, params: Value, _timeout: Duration) -> Result<Value> {
		self.inner.log.lock().push((method.to_string(), params.clone()));
		if !self.inner.delay.is_zero() {
			tokio::time::sleep(self.inner.delay).await;
		}
		if method == super::protocol::METHOD_INITIALIZE {
			return Ok(self.inner.init.clone());
		}
		if let Some(answer) = self.inner.queue.lock().pop_front() {
			return Ok(answer);
		}
		(self.inner.responder)(method, &params)
	}

	async fn notify(&self, method: &str, params: Value) -> Result<()> {
		self.inner.log.lock().push((method.to_string(), params));
		Ok(())
	}

	fn is_connected(&self) -> bool {
		self.inner.connected.load(Ordering::SeqCst)
	}
}

static STUBS: OnceLock<parking_lot::Mutex<HashMap<String, StubPlugin>>> = OnceLock::new();

fn stubs() -> &'static parking_lot::Mutex<HashMap<String, StubPlugin>> {
	STUBS.get_or_init(Default::default)
}

/// The connector of the process-wide host in tests: a plugin name that belongs
/// to a connected stub reaches the stub, any other reaches a real process.
pub struct StubConnector;

#[async_trait]
impl Connector for StubConnector {
	async fn connect(&self, config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		let stub = stubs().lock().get(&config.name).cloned();
		match stub {
			Some(stub) => Ok(Arc::new(stub)),
			None => ProcessConnector.connect(config).await,
		}
	}
}

/// The config entry that stands for `stub`.
pub fn stub_config(stub: &StubPlugin) -> PluginConfig {
	toml::from_str(&format!(
		"name = \"{}\"\ntype = \"stdio\"\ncommand = \"stub\"\ntimeout_seconds = {STUB_TIMEOUT_SECONDS}\n",
		stub.name()
	))
	.expect("stub config parses")
}

/// Switch `stub` on for `session_id` on the process-wide host, as
/// `plugins::start_session` would for a configured plugin. Remove it again with
/// `plugins::end_session(session_id)`.
pub async fn connect_stub(session_id: &str, role: &str, workdir: &Path, stub: &StubPlugin) {
	stubs().lock().insert(stub.name().to_string(), stub.clone());
	super::host()
		.start_session(session_id, role, workdir, &[stub_config(stub)])
		.await;
}
