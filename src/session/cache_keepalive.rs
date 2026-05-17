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

//! Idle-time prompt cache keepalive.
//!
//! Sends a minimal `max_tokens=1` chat completion against a frozen snapshot
//! of the conversation while the user is idle, so the provider's prompt
//! cache TTL keeps resetting and the next real turn still hits cache.
//!
//! Read-only on session state: the snapshot is owned by the keepalive task
//! and `session.messages` is never mutated by a ping. Only billing counters
//! move (via [`crate::session::chat::CostTracker::track_exchange_cost`] when
//! the caller folds the returned exchanges back in).
//!
//! Provider-aware: a ping only fires when the resolved provider returns
//! `Some(KeepalivePolicy)` from its trait method. Today that is Anthropic
//! only. Other providers either manage cache server-side with no
//! observable refresh primitive or have no cache at all — pinging them
//! would just burn money. The interval comes from the provider, not from
//! octomind's config.

use crate::config::Config;
use crate::providers::{ChatCompletionParams, ProviderExchange, ProviderFactory};
use crate::session::Message;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Successful pings collected while the handle was alive. Each item is one
/// `ProviderExchange` whose `usage` field carries the cache-read /
/// cache-write / output token counts and provider-computed cost.
/// Folded into the session's cost tracker by the caller.
pub type KeepaliveExchanges = Vec<ProviderExchange>;

/// A running keepalive task. Drop without `cancel()` and the task keeps
/// firing pings until `max_idle` elapses or the process exits — call
/// [`KeepaliveHandle::cancel`] to stop it cleanly and harvest the
/// accumulated exchanges for cost accounting.
pub struct KeepaliveHandle {
	cancel_tx: watch::Sender<bool>,
	task: JoinHandle<KeepaliveExchanges>,
}

impl KeepaliveHandle {
	/// Spawn a keepalive task if conditions are met. Returns `None` when:
	/// - `enabled` is false,
	/// - the snapshot has no cached message (nothing to keep warm),
	/// - model parsing fails,
	/// - or the provider has no keepalive policy (e.g. OpenAI, DeepSeek).
	///
	/// On success the task is detached and runs until `cancel()` is called
	/// or `max_idle` is reached.
	pub fn spawn(
		messages: Vec<Message>,
		model: String,
		config: Config,
		enabled: bool,
		max_idle: Duration,
	) -> Option<Self> {
		if !enabled {
			return None;
		}
		// No cache markers in the snapshot ⇒ no warm cache to extend.
		if !messages.iter().any(|m| m.cached) {
			return None;
		}

		let (provider, actual_model) = ProviderFactory::get_provider_for_model(&model).ok()?;
		let policy = provider.keepalive_policy(&actual_model, true)?;

		let (cancel_tx, cancel_rx) = watch::channel(false);
		let task = tokio::spawn(run(
			messages,
			model,
			config,
			policy.interval,
			max_idle,
			cancel_rx,
		));

		crate::log_debug!(
			"Cache keepalive started: interval={}s, max_idle={}s",
			policy.interval.as_secs(),
			max_idle.as_secs()
		);

		Some(Self { cancel_tx, task })
	}

	/// Signal the task to stop and wait for it to finish, returning all
	/// successful pings for the caller to attribute cost to.
	pub async fn cancel(self) -> KeepaliveExchanges {
		let _ = self.cancel_tx.send(true);
		match self.task.await {
			Ok(exchanges) => exchanges,
			Err(e) => {
				crate::log_debug!("Cache keepalive task join error: {}", e);
				Vec::new()
			}
		}
	}
}

async fn run(
	messages: Vec<Message>,
	model: String,
	config: Config,
	interval: Duration,
	max_idle: Duration,
	mut cancel_rx: watch::Receiver<bool>,
) -> KeepaliveExchanges {
	let mut exchanges = Vec::new();
	let started = Instant::now();
	let max_idle_enabled = max_idle.as_secs() > 0;

	loop {
		// Stop if max_idle has elapsed (cheap session-abandoned guard).
		if max_idle_enabled && started.elapsed() >= max_idle {
			crate::log_debug!(
				"Cache keepalive: max_idle ({}s) reached, stopping",
				max_idle.as_secs()
			);
			break;
		}

		// Wait one interval, with cancel as the override.
		let sleep = tokio::time::sleep(interval);
		tokio::pin!(sleep);
		tokio::select! {
			_ = &mut sleep => {}
			res = cancel_rx.changed() => {
				// channel closed or cancel signalled
				if res.is_err() || *cancel_rx.borrow() {
					break;
				}
				continue;
			}
		}

		// Re-check cancel right before firing — user may have started
		// typing during the interval and we don't want a ping racing the
		// real next turn.
		if *cancel_rx.borrow() {
			break;
		}

		// Best-effort ping. Failures (network, rate limit, etc.) are
		// logged at debug level and silently retried next interval —
		// keepalive is non-essential and must never abort the session.
		match send_ping(&messages, &model, &config, cancel_rx.clone()).await {
			Ok(exchange) => {
				crate::log_debug!(
					"Cache keepalive ping ok ({}s elapsed)",
					started.elapsed().as_secs()
				);
				exchanges.push(exchange);
			}
			Err(e) => {
				crate::log_debug!("Cache keepalive ping failed: {}", e);
			}
		}
	}

	exchanges
}

async fn send_ping(
	messages: &[Message],
	model: &str,
	config: &Config,
	cancel_rx: watch::Receiver<bool>,
) -> Result<ProviderExchange> {
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(model)?;

	// max_tokens=1 is the whole point: cheapest valid completion that still
	// triggers a prompt cache read (which resets the TTL on the cached
	// blocks). Sampling params are irrelevant when generating a single token
	// but must be valid; defaults match the provider's "no preference" path.
	let chat_params = ChatCompletionParams::new(messages, &actual_model, 0.0, 1.0, 1, 1, config)
		.with_max_retries(0)
		.with_cancellation_token(cancel_rx);

	let octolib_params = chat_params
		.to_octolib_params()
		.await
		.map_err(|e| anyhow::anyhow!("Failed to convert keepalive ping parameters: {}", e))?;

	let response = provider.chat_completion(octolib_params).await?;
	Ok(response.exchange)
}
