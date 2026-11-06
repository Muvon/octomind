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

//! Prompt cache keepalive while the session waits.
//!
//! While the session waits — for the user's reply or for a long tool run —
//! a cache pre-warm request (no output generated, only a cache read billed)
//! replays the cached prefix so the provider's TTL keeps resetting and the
//! next real request still hits cache.
//!
//! How many pings to spend is an economic decision, never a fixed cap. Each
//! ping costs one cache read of the whole prefix; letting the entry expire
//! costs a write instead of a read of the tail that expires with it (tools,
//! system prompt and 1h-marked messages stay warm on their own). In pings,
//! that penalty is `B = (write − read) / read × tail share`, and `⌊B⌋` is the
//! most that can ever pay off.
//!
//! Within that bound the budget minimizes the expected cost under this
//! session's own wait rhythm. Each ping interval `j` has a reply hazard `h_j`
//! (the chance the wait ends in it, given it lasted that long), estimated
//! from past waits of the same kind — answered and cut-short ones alike — and
//! shrunk toward the session's pooled rate so a few samples cannot overrule
//! it. An interval no wait ever reached reads as no reply at all, so the
//! budget never outruns the longest wait seen: the first longer wait pays one
//! rewrite and extends the record. With `S_j` the chance of still waiting when
//! interval `j` starts: `E[cost(n)] = Σ_{j=1..n} S_j + B · S_{n+1}`, so the
//! n-th ping pays exactly when `B · h_n > 1`. Until a kind has measured
//! [`COLD_START_WAITS`] waits the budget is capped at [`COLD_START_PINGS`]:
//! the coin-flip prior alone would spend the whole bound on a user who left.
//! A known wake (a scheduled turn) is bridged exactly when affordable, and a
//! prefix the next request replaces anyway (a pending or due compression) is
//! never pinged.
//!
//! Read-only on session state: the snapshot is owned by the keepalive task and
//! `session.messages` is never mutated by a ping. [`KeepaliveHandle::finish`]
//! bills the pings and records the measured wait.
//!
//! Provider-aware: a ping only fires when the resolved provider returns
//! `Some(KeepalivePolicy)` and has pricing. Today that is Anthropic and
//! OpenRouter Claude routes.

use crate::config::{Config, ModelProfile};
use crate::providers::{ChatCompletionParams, ProviderExchange, ProviderFactory};
use crate::session::chat::ChatSession;
use crate::session::Message;
use anyhow::Result;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// TTL marker value for the long (1h) Anthropic cache entry.
const LONG_CACHE_TTL: &str = "1h";

/// Prices are decimal dollars stored as binary floats: (5.00 − 0.20) / 0.20
/// lands at 23.999…, so the floor needs this much slack to give 24.
const PRICE_RATIO_TOLERANCE: f64 = 1e-9;

/// Jeffreys prior on the session's pooled reply rate: half a reply per
/// interval. With no history every interval reads as a coin flip; the
/// cold-start cap, not this prior, bounds that first budget.
const PRIOR_REPLIES: f64 = 0.5;
const PRIOR_INTERVALS: f64 = 1.0;

/// Weight, in waits, of the pooled rate an interval's own estimate is shrunk
/// toward: one pseudo-wait, so measured waits overrule it as soon as there
/// are any, yet an interval no wait has reached still reads the session's
/// rhythm instead of a coin flip.
const POOLED_RATE_WEIGHT: f64 = 1.0;

/// Recency half-life of a measured wait, in waits. A long session drifts —
/// focused stretches with quick replies, then breaks — so the estimate tracks
/// the current phase: the wait eight waits back counts half the latest one.
const WAIT_HALF_LIFE: f64 = 8.0;

/// Waits kept per kind: past four half-lives a wait weighs under 1/16 and no
/// longer moves the estimate.
const MAX_WAITS: usize = 32;

/// Waits a kind must have measured before its own rhythm sets the budget.
/// Until then the budget is capped at [`COLD_START_PINGS`]: with no rhythm to
/// read, the coin-flip prior would spend the whole ski-rental bound on a user
/// who walked away — on the 1h cache, a night of pings.
const COLD_START_WAITS: usize = 3;

/// Pings a cold wait may spend. The interval is 0.9 × TTL and the entry
/// outlives the last ping by one interval, so two pings cover about three
/// TTLs: one long first reply without a rewrite, never a night of pings.
const COLD_START_PINGS: u32 = 2;

/// What the session waits on while a keepalive runs. Each kind keeps its own
/// history: how fast a user replies says nothing about how long builds take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
	/// Waiting for the next user / client message.
	Reply,
	/// Waiting with work in flight (tap runs, detached shell or agent jobs,
	/// running scheduled tools): a result is certain to come back. Chosen by
	/// [`KeepaliveHandle::spawn`] for a `Reply` wait while the session is busy.
	Background,
	/// Waiting for the current turn's tool calls to finish.
	Tools,
}

/// One measured wait: how long it ran, and whether it ended with what it
/// waited for. A cut-short wait (session exit, client gone) still says the
/// answer took at least that long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wait {
	pub length: Duration,
	pub answered: bool,
}

/// Why a wait got the ping budget it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetBasis {
	/// The next request folds or trims the prefix: nothing to keep warm.
	PrefixReplaced,
	/// A ping of this session missed the cache: another would pay the same rewrite.
	PingMissed,
	/// A scheduled wake within the bound, bridged exactly.
	KnownWake,
	/// Fewer than [`COLD_START_WAITS`] waits measured: capped at [`COLD_START_PINGS`].
	ColdStart,
	/// The expected-cost minimum under the session's own wait rhythm.
	Learned,
}

/// The ping budget of one wait and how it was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BudgetDecision {
	pub kind: WaitKind,
	pub pings: u32,
	/// The ski-rental bound `⌊B⌋` the budget was chosen within.
	pub bound: u32,
	pub basis: BudgetBasis,
}

/// Keepalive accounting for `/info`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct KeepaliveReport {
	pub pings_sent: u64,
	pub ping_cost: f64,
	pub last_budget: BudgetDecision,
}

/// Measured waits per kind, newest last, and what their keepalives cost
/// (runtime-only, never persisted).
#[derive(Debug, Clone, Default)]
pub struct WaitHistory {
	reply: VecDeque<Wait>,
	background: VecDeque<Wait>,
	tools: VecDeque<Wait>,
	pings_sent: u64,
	ping_cost: f64,
	last_budget: Option<BudgetDecision>,
	ping_missed: bool,
}

impl WaitHistory {
	pub fn record(&mut self, kind: WaitKind, length: Duration, answered: bool) {
		let waits = self.samples_mut(kind);
		waits.push_back(Wait { length, answered });
		if waits.len() > MAX_WAITS {
			waits.pop_front();
		}
	}

	pub fn samples(&self, kind: WaitKind) -> &VecDeque<Wait> {
		match kind {
			WaitKind::Reply => &self.reply,
			WaitKind::Background => &self.background,
			WaitKind::Tools => &self.tools,
		}
	}

	/// `None` until a wait has been measured.
	pub fn report(&self) -> Option<KeepaliveReport> {
		self.last_budget.map(|last_budget| KeepaliveReport {
			pings_sent: self.pings_sent,
			ping_cost: self.ping_cost,
			last_budget,
		})
	}

	fn samples_mut(&mut self, kind: WaitKind) -> &mut VecDeque<Wait> {
		match kind {
			WaitKind::Reply => &mut self.reply,
			WaitKind::Background => &mut self.background,
			WaitKind::Tools => &mut self.tools,
		}
	}
}

/// A running keepalive. Finish it with [`KeepaliveHandle::finish`]: dropping
/// it stops the pings (the cancel channel closes) but loses their cost.
pub struct KeepaliveHandle {
	cancel_tx: watch::Sender<bool>,
	task: JoinHandle<Vec<ProviderExchange>>,
	kind: WaitKind,
	budget: BudgetDecision,
	started: Instant,
}

impl KeepaliveHandle {
	/// Start a keepalive for a wait of `kind`. Returns `None` when keepalive
	/// is disabled, nothing is cached, the session requests structured output
	/// (the API rejects a pre-warm with an output schema), or the provider has
	/// no keepalive policy or pricing. A zero ping budget still returns a
	/// handle: the wait is measured either way, so the next budget learns.
	pub fn spawn(chat_session: &ChatSession, config: &Config, kind: WaitKind) -> Option<Self> {
		if !config.cache_keepalive_enabled || chat_session.schema.is_some() {
			return None;
		}
		let kind = match kind {
			WaitKind::Reply if !crate::mcp::orchestration::is_session_idle() => WaitKind::Background,
			other => other,
		};
		let snapshot = ping_prefix(chat_session)?;
		let profile = chat_session.model_profile(config);
		let (provider, actual_model) = ProviderFactory::get_provider_for_model(&profile.model).ok()?;
		let long_ttl = snapshot.last()?.cache_ttl.as_deref() == Some(LONG_CACHE_TTL);
		let policy = provider.keepalive_policy(&actual_model, long_ttl)?;
		let pricing = provider.get_model_pricing(&actual_model)?;

		let penalty = rewrite_penalty_in_pings(
			pricing.cache_write_price_per_1m,
			pricing.cache_read_price_per_1m,
			tail_share(&snapshot, chat_session.cached_tools.as_deref(), &profile.model),
		);
		let known_wake = match kind {
			WaitKind::Reply | WaitKind::Background => crate::mcp::orchestration::next_due_in(),
			WaitKind::Tools => None,
		};
		// A prefix the next request rewrites anyway (a fold or a trim) has
		// nothing left to save: keep measuring the wait, send nothing.
		let prefix_replaced = crate::session::chat::conversation_compression::replaces_prefix_on_next_request(
			chat_session,
			config,
			kind == WaitKind::Reply,
		);
		let (pings, basis) = if chat_session.wait_history.ping_missed {
			(0, BudgetBasis::PingMissed)
		} else if prefix_replaced {
			(0, BudgetBasis::PrefixReplaced)
		} else {
			ping_budget(
				chat_session.wait_history.samples(kind),
				known_wake,
				policy.interval,
				penalty,
			)
		};
		let budget = BudgetDecision {
			kind,
			pings,
			bound: max_pings(penalty),
			basis,
		};

		let (cancel_tx, cancel_rx) = watch::channel(false);
		let ping_loop = run(
			snapshot,
			profile,
			config.clone(),
			policy.interval,
			budget.pings,
			cancel_rx,
		);
		// A spawned task drops the task-local session: without it the ping
		// would assemble the global tool list instead of this session's, and
		// a different tools block misses the cache.
		let session_id = crate::session::context::current_session_id();
		let task = tokio::spawn(async move {
			match session_id {
				Some(id) => crate::session::context::with_session_id(id, ping_loop).await,
				None => ping_loop.await,
			}
		});

		crate::log_debug!(
			"Cache keepalive started: kind={:?}, interval={}s, budget={} ping(s) ({:?}) within {}, rewrite penalty={:.1} ping(s)",
			kind,
			policy.interval.as_secs(),
			budget.pings,
			budget.basis,
			budget.bound,
			penalty
		);

		Some(Self {
			cancel_tx,
			task,
			kind,
			budget,
			started: Instant::now(),
		})
	}

	/// Stop the task, bill its pings to the session and record the wait.
	/// `answered`: the wait ended with what it waited for (input arrived,
	/// tools finished) rather than being cut short (disconnect, unload,
	/// session exit). Both kinds feed the next budget. Returns the ping count.
	pub async fn finish(self, chat_session: &mut ChatSession, answered: bool) -> usize {
		let elapsed = self.started.elapsed();
		let _ = self.cancel_tx.send(true);
		let exchanges = match self.task.await {
			Ok(exchanges) => exchanges,
			Err(e) => {
				crate::log_debug!("Cache keepalive task join error: {}", e);
				Vec::new()
			}
		};
		let history = &mut chat_session.wait_history;
		history.record(self.kind, elapsed, answered);
		history.pings_sent += exchanges.len() as u64;
		history.ping_cost += exchanges
			.iter()
			.filter_map(|exchange| exchange.usage.as_ref().and_then(|usage| usage.cost))
			.sum::<f64>();
		history.last_budget = Some(self.budget);
		history.ping_missed |= exchanges.iter().any(ping_missed_cache);
		bill_pings(&mut chat_session.session.info, &exchanges);
		chat_session.estimated_cost = chat_session.session.info.total_cost;
		crate::log_debug!(
			"Cache keepalive finished: kind={:?}, waited={}s, answered={}, {} ping(s), session_total=${:.5}",
			self.kind,
			elapsed.as_secs(),
			answered,
			exchanges.len(),
			chat_session.session.info.total_cost
		);
		exchanges.len()
	}
}

/// The messages a ping replays: what the latest request sent, up to its newest
/// cache marker. Anything after the marker was never cached and would only add
/// input. `None` when nothing was sent yet, or when a fold, rollback or clear
/// replaced that marker since: the next request can't reuse that prefix.
fn ping_prefix(chat_session: &ChatSession) -> Option<Vec<Message>> {
	let sent = &chat_session.last_sent_messages;
	let last_cached = sent.iter().rposition(|m| m.cached)?;
	let live = chat_session.session.messages.get(last_cached)?;
	(live.content == sent[last_cached].content).then(|| sent[..=last_cached].to_vec())
}

/// A ping replays a prefix the last request already cached, so a hit writes
/// nothing. Reading the 1h head while re-writing the tail is still a miss.
fn ping_missed_cache(exchange: &ProviderExchange) -> bool {
	exchange
		.usage
		.as_ref()
		.is_none_or(|usage| usage.cache_read_tokens == 0 || usage.cache_write_tokens > 0)
}

/// Pings are billed for their cost only. They are not conversation requests:
/// counting them as API calls, prompt tokens or API time would skew the
/// session's pace and context counters that compression decides on, and the
/// cache statistics shown to the user.
fn bill_pings(info: &mut crate::session::SessionInfo, exchanges: &[ProviderExchange]) {
	for exchange in exchanges {
		info.record_main_cost(exchange.usage.as_ref().and_then(|usage| usage.cost));
	}
}

/// Share of the pinged prefix that an expiry would actually re-write. A ping
/// reads the whole prefix, but the tools, the system prompt and any 1h-marked
/// message stay warm in their own long entry; only the tail after the last
/// long boundary expires with the newest marker. Without cached tool
/// definitions the tools are left out of both sides, which overstates the
/// share slightly (more pings), never understates it.
fn tail_share(snapshot: &[Message], tools: Option<&[crate::mcp::McpFunction]>, model: &str) -> f64 {
	let newest = snapshot.len() - 1;
	// The provider gives the cached system prompt the long TTL on every request.
	let long_end = snapshot[..newest]
		.iter()
		.rposition(|m| m.cache_ttl.as_deref() == Some(LONG_CACHE_TTL))
		.or_else(|| (snapshot[0].role == "system" && snapshot[0].cached).then_some(0))
		.map_or(0, |i| i + 1);
	let total = crate::session::estimate_sent_full_context_tokens(snapshot, tools, model) as f64;
	let long = crate::session::estimate_sent_full_context_tokens(&snapshot[..long_end], tools, model) as f64;
	if total <= 0.0 {
		return 0.0;
	}
	((total - long) / total).clamp(0.0, 1.0)
}

/// What letting the entry expire costs, in pings. A ping is one cache read of
/// the whole prefix; an expiry makes the next request write the tail instead
/// of reading it.
fn rewrite_penalty_in_pings(write_price: f64, read_price: f64, tail_share: f64) -> f64 {
	if read_price <= 0.0 {
		return 0.0;
	}
	((write_price - read_price) / read_price * tail_share).max(0.0)
}

/// Ski-rental bound: the most pings that can beat the rewrite.
fn max_pings(penalty: f64) -> u32 {
	(penalty + PRICE_RATIO_TOLERANCE).floor() as u32
}

/// Pings needed to keep the entry warm until `wait` ends. The interval is
/// below the TTL, so the entry outlives the last ping by at least one gap.
fn pings_to_bridge(wait: Duration, interval: Duration) -> u32 {
	(wait.as_millis() / interval.as_millis().max(1)) as u32
}

/// Pings to spend on this wait, in `0..=max_pings(penalty)`, and why. A
/// known wake that is affordable is bridged exactly — a turn is certain then.
/// A kind with fewer than [`COLD_START_WAITS`] waits measured is capped at
/// [`COLD_START_PINGS`]. Otherwise the budget minimizes
/// `E[cost(n)] = Σ_{j=1..n} S_j + B · S_{n+1}`: ping `j` goes out only while
/// the wait is still open at its interval, and the rewrite is paid only when
/// the wait outlasts the last ping's coverage. Ties choose the smaller budget.
fn ping_budget(
	history: &VecDeque<Wait>,
	known_wake: Option<Duration>,
	interval: Duration,
	penalty: f64,
) -> (u32, BudgetBasis) {
	let bound = max_pings(penalty);
	if let Some(wake) = known_wake {
		let needed = pings_to_bridge(wake, interval);
		if needed <= bound {
			return (needed, BudgetBasis::KnownWake);
		}
	}
	let (max, basis) = if history.len() < COLD_START_WAITS {
		(bound.min(COLD_START_PINGS), BudgetBasis::ColdStart)
	} else {
		(bound, BudgetBasis::Learned)
	};
	// survival[j]: the chance the wait is still open when interval j starts.
	let mut survival = vec![1.0];
	let mut open = 1.0;
	for hazard in interval_hazards(history, interval, max) {
		open *= 1.0 - hazard;
		survival.push(open);
	}
	let expected_cost = |budget: usize| survival[1..=budget].iter().sum::<f64>() + penalty * survival[budget + 1];
	let pings = (0..=max as usize)
		.min_by(|a, b| expected_cost(*a).total_cmp(&expected_cost(*b)))
		.unwrap_or(0) as u32;
	(pings, basis)
}

/// Reply hazard of each ping interval `0..=max`: replies that landed in it
/// over waits that reached it, recency-weighted and shrunk toward the
/// session's pooled rate (replies per interval waited, over every past wait
/// of this kind). A cut-short wait counts as waited up to where it stopped,
/// never as a reply — dropping it would make waits look shorter than they are.
/// An interval no measured wait reached has no reply evidence and reads as
/// zero, so pings never outrun the longest wait seen; with no waits at all
/// every interval is the prior's coin flip.
fn interval_hazards(history: &VecDeque<Wait>, interval: Duration, max: u32) -> Vec<f64> {
	let slots = max as usize + 1;
	let mut reached = vec![0.0; slots];
	let mut replies = vec![0.0; slots];
	let mut pooled_replies = PRIOR_REPLIES;
	let mut pooled_intervals = PRIOR_INTERVALS;
	for (age, wait) in history.iter().rev().enumerate() {
		let weight = 0.5_f64.powf(age as f64 / WAIT_HALF_LIFE);
		let last = pings_to_bridge(wait.length, interval) as usize;
		pooled_intervals += weight * (last + 1) as f64;
		for slot in reached.iter_mut().take(last + 1) {
			*slot += weight;
		}
		if wait.answered {
			pooled_replies += weight;
			if let Some(slot) = replies.get_mut(last) {
				*slot += weight;
			}
		}
	}
	let pooled_rate = pooled_replies / pooled_intervals;
	reached
		.iter()
		.zip(&replies)
		.map(|(reached, replies)| {
			if !history.is_empty() && *reached == 0.0 {
				return 0.0;
			}
			(replies + POOLED_RATE_WEIGHT * pooled_rate) / (reached + POOLED_RATE_WEIGHT)
		})
		.collect()
}

async fn run(
	snapshot: Vec<Message>,
	profile: ModelProfile,
	config: Config,
	interval: Duration,
	budget: u32,
	mut cancel_rx: watch::Receiver<bool>,
) -> Vec<ProviderExchange> {
	let mut exchanges = Vec::new();

	for _ in 0..budget {
		tokio::select! {
			_ = tokio::time::sleep(interval) => {}
			res = cancel_rx.changed() => {
				// channel closed or cancel signalled
				if res.is_err() || *cancel_rx.borrow() {
					break;
				}
			}
		}

		// Re-check right before firing: a ping must never race the real
		// request that ended the wait.
		if *cancel_rx.borrow() {
			break;
		}

		// Best-effort: failures are logged and the next interval retries.
		// A keepalive must never abort the session.
		match send_ping(&snapshot, &profile, &config, cancel_rx.clone()).await {
			Ok(exchange) => {
				// A missed ping re-wrote the prefix at the write price: its
				// request differs from the real one. Stop instead of paying
				// that again every interval.
				let missed = ping_missed_cache(&exchange);
				exchanges.push(exchange);
				if missed {
					crate::log_info!("Cache keepalive: ping missed the cache, stopping");
					break;
				}
			}
			Err(e) if hit_rate_limit(&e) => {
				// The quota is shared with the real requests: spending the rest
				// of the budget on retries would only crowd them out.
				crate::log_info!("Cache keepalive: provider rate limit, stopping");
				break;
			}
			Err(e) => {
				crate::log_debug!("Cache keepalive ping failed: {}", e);
			}
		}
	}

	exchanges
}

/// Providers wrap a 429 in octolib's typed retryable status; with the ping's
/// zero retries it surfaces as the error's root cause.
fn hit_rate_limit(err: &anyhow::Error) -> bool {
	err.chain().any(|cause| {
		cause
			.downcast_ref::<octolib::llm::retry::RetryableStatusError>()
			.is_some_and(|status| status.status == reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16())
	})
}

async fn send_ping(
	messages: &[Message],
	profile: &ModelProfile,
	config: &Config,
	cancel_rx: watch::Receiver<bool>,
) -> Result<ProviderExchange> {
	let (provider, actual_model) = ProviderFactory::get_provider_for_model(&profile.model)?;

	// Same model, effort, tools and vision as the real request: any of them
	// differing changes the cached prefix and the ping would miss.
	let chat_params = ChatCompletionParams::new(
		messages,
		&actual_model,
		profile.temperature,
		profile.top_p,
		profile.top_k,
		profile.max_tokens,
		config,
	)
	.with_max_retries(0)
	.with_cancellation_token(cancel_rx)
	.with_reasoning_effort(profile.reasoning_effort)
	.with_vision(crate::session::model_utils::provider_supports_vision(
		provider.as_ref(),
		&actual_model,
	))
	.with_cache_prewarm();

	let octolib_params = chat_params
		.to_octolib_params(provider.name())
		.await
		.map_err(|e| anyhow::anyhow!("Failed to convert keepalive ping parameters: {}", e))?;

	let response = provider.chat_completion(octolib_params).await?;
	Ok(response.exchange)
}

#[cfg(test)]
#[path = "cache_keepalive_tests.rs"]
mod tests;
