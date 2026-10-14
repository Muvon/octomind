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

//! Session-owned usage and inclusive mechanic wall time. Blocking time is the
//! union of active mechanic intervals, so nested seams and parallel windows do
//! not inflate the turn split. Banks drain exactly once, like external spend.

use super::{
	evaluate::Seam,
	report::{Mechanic, Outcome},
	stats::CallKind,
};
use crate::session::context::{current_session_id, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
	#[serde(default)]
	pub outcome: BTreeMap<Outcome, u64>,
	pub calls: u64,
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub reasoning_tokens: u64,
	pub cost: f64,
	pub api_time_ms: u64,
	pub blocking_wall_time_ms: u64,
	pub background_wall_time_ms: u64,
}

impl Usage {
	fn add(&mut self, other: &Self) {
		for (&key, &count) in &other.outcome {
			*self.outcome.entry(key).or_default() += count;
		}
		self.calls += other.calls;
		self.input_tokens += other.input_tokens;
		self.output_tokens += other.output_tokens;
		self.reasoning_tokens += other.reasoning_tokens;
		self.cost += other.cost;
		self.api_time_ms += other.api_time_ms;
		self.blocking_wall_time_ms += other.blocking_wall_time_ms;
		self.background_wall_time_ms += other.background_wall_time_ms;
	}
	fn merge_max(&mut self, other: &Self) {
		for (&key, &count) in &other.outcome {
			let own = self.outcome.entry(key).or_default();
			*own = (*own).max(count);
		}
		self.calls = self.calls.max(other.calls);
		self.input_tokens = self.input_tokens.max(other.input_tokens);
		self.output_tokens = self.output_tokens.max(other.output_tokens);
		self.reasoning_tokens = self.reasoning_tokens.max(other.reasoning_tokens);
		self.cost = self.cost.max(other.cost);
		self.api_time_ms = self.api_time_ms.max(other.api_time_ms);
		self.blocking_wall_time_ms = self.blocking_wall_time_ms.max(other.blocking_wall_time_ms);
		self.background_wall_time_ms = self
			.background_wall_time_ms
			.max(other.background_wall_time_ms);
	}
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TurnTime {
	pub completed: u64,
	pub total_time_ms: u64,
	pub main_api_time_ms: u64,
	pub blocking_supervisor_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SupervisorAccounting {
	#[serde(default)]
	pub background_wall_time_ms: Option<u64>,
	#[serde(default)]
	pub turn_time: TurnTime,
	#[serde(default)]
	pub background_runs: BTreeSet<String>,
	pub kinds: BTreeMap<CallKind, Usage>,
	pub seams: BTreeMap<Seam, Usage>,
	/// Union of blocking intervals; per-kind and seam wall times are inclusive.
	pub blocking_wall_time_ms: u64,
}

impl Default for SupervisorAccounting {
	fn default() -> Self {
		Self {
			background_wall_time_ms: Some(0),
			turn_time: TurnTime::default(),
			background_runs: BTreeSet::new(),
			kinds: BTreeMap::new(),
			seams: BTreeMap::new(),
			blocking_wall_time_ms: 0,
		}
	}
}

impl SupervisorAccounting {
	pub fn add(&mut self, other: &Self) {
		self.background_wall_time_ms = self
			.background_wall_time_ms
			.zip(other.background_wall_time_ms)
			.map(|(a, b)| a + b);
		self.turn_time.completed += other.turn_time.completed;
		self.turn_time.total_time_ms += other.turn_time.total_time_ms;
		self.turn_time.main_api_time_ms += other.turn_time.main_api_time_ms;
		self.turn_time.blocking_supervisor_time_ms += other.turn_time.blocking_supervisor_time_ms;
		self.background_runs
			.extend(other.background_runs.iter().cloned());
		for (&kind, usage) in &other.kinds {
			self.kinds.entry(kind).or_default().add(usage);
		}
		for (&seam, usage) in &other.seams {
			self.seams.entry(seam).or_default().add(usage);
		}
		self.blocking_wall_time_ms += other.blocking_wall_time_ms;
	}
	pub fn merge_max(&mut self, other: &Self) {
		self.background_wall_time_ms = self
			.background_wall_time_ms
			.zip(other.background_wall_time_ms)
			.map(|(a, b)| a.max(b));
		self.turn_time.completed = self.turn_time.completed.max(other.turn_time.completed);
		self.turn_time.total_time_ms = self
			.turn_time
			.total_time_ms
			.max(other.turn_time.total_time_ms);
		self.turn_time.main_api_time_ms = self
			.turn_time
			.main_api_time_ms
			.max(other.turn_time.main_api_time_ms);
		self.turn_time.blocking_supervisor_time_ms = self
			.turn_time
			.blocking_supervisor_time_ms
			.max(other.turn_time.blocking_supervisor_time_ms);
		self.background_runs
			.extend(other.background_runs.iter().cloned());
		for (&kind, usage) in &other.kinds {
			self.kinds.entry(kind).or_default().merge_max(usage);
		}
		for (&seam, usage) in &other.seams {
			self.seams.entry(seam).or_default().merge_max(usage);
		}
		self.blocking_wall_time_ms = self.blocking_wall_time_ms.max(other.blocking_wall_time_ms);
	}
	pub fn totals(&self) -> Usage {
		let mut total = Usage::default();
		for usage in self.kinds.values() {
			total.add(usage);
		}
		total
	}
}

tokio::task_local! {
	static BACKGROUND: bool;
	static DETACHED_USAGE: Arc<Mutex<Bank>>;
}

pub async fn background<T>(future: impl std::future::Future<Output = T>) -> T {
	BACKGROUND.scope(true, future).await
}

/// Awaited tool tasks inherit their caller's mode; spawning them does not detach the turn.
pub fn inherit<F: std::future::Future>(future: F) -> impl std::future::Future<Output = F::Output> {
	let background = BACKGROUND.try_with(|value| *value).unwrap_or(false);
	let detached = DETACHED_USAGE.try_with(Arc::clone).ok();
	async move {
		let future = BACKGROUND.scope(background, future);
		match detached {
			Some(bank) => DETACHED_USAGE.scope(bank, future).await,
			None => future.await,
		}
	}
}

pub(super) fn has_detached_usage() -> bool {
	DETACHED_USAGE.try_with(|_| ()).is_ok()
}

/// Completed calls are journaled before the next await; completion publishes
/// the remaining outcomes and wall time through the same deduplicated journal.
pub async fn run_background<T>(
	owner: SessionId,
	session_file: Option<std::path::PathBuf>,
	future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
	let bank = Arc::new(Mutex::new(Bank {
		journal: Some(Journal {
			owner: owner.clone(),
			session_file,
		}),
		..Default::default()
	}));
	let result = crate::session::context::with_session_id(
		owner.clone(),
		background(DETACHED_USAGE.scope(Arc::clone(&bank), future)),
	)
	.await;
	bank.lock()
		.expect("detached supervisor bank poisoned")
		.publish()?;
	result
}

/// Called at the shared provider-response boundaries, after both aggregate and
/// seam usage are recorded, before cancellation can interrupt another call.
pub(super) fn checkpoint() -> anyhow::Result<()> {
	match DETACHED_USAGE.try_with(Arc::clone) {
		Ok(bank) => bank
			.lock()
			.expect("detached supervisor bank poisoned")
			.publish(),
		Err(_) => Ok(()),
	}
}

struct Journal {
	owner: SessionId,
	session_file: Option<std::path::PathBuf>,
}

fn completed() -> &'static Mutex<HashMap<SessionId, BTreeMap<String, SupervisorAccounting>>> {
	static COMPLETED: OnceLock<Mutex<HashMap<SessionId, BTreeMap<String, SupervisorAccounting>>>> =
		OnceLock::new();
	COMPLETED.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn take_completed(owner: &str) -> BTreeMap<String, SupervisorAccounting> {
	completed()
		.lock()
		.expect("completed supervisor bank poisoned")
		.remove(owner)
		.unwrap_or_default()
}

#[derive(Default)]
struct Interval {
	active: usize,
	since: Option<Instant>,
}

impl Interval {
	fn enter(&mut self, now: Instant) {
		if self.active == 0 {
			self.since = Some(now);
		}
		self.active += 1;
	}
	fn leave(&mut self) {
		self.active -= 1;
		if self.active == 0 {
			self.since = None;
		}
	}
	fn tick(&mut self, now: Instant) -> u64 {
		let Some(since) = self.since else {
			return 0;
		};
		let elapsed = now.duration_since(since).as_millis() as u64;
		self.since = Some(since + std::time::Duration::from_millis(elapsed));
		elapsed
	}
}

#[derive(Default)]
struct Clock {
	blocking: Interval,
	background: Interval,
	mechanics: BTreeMap<(Mechanic, bool), Interval>,
}

impl Clock {
	fn tick(&mut self, now: Instant, usage: &mut SupervisorAccounting) {
		self.tick_mode(now, usage, false);
		self.tick_mode(now, usage, true);
	}
	fn tick_mode(&mut self, now: Instant, usage: &mut SupervisorAccounting, background: bool) {
		if background {
			*usage
				.background_wall_time_ms
				.as_mut()
				.expect("live clock has a measured background total") += self.background.tick(now);
		} else {
			usage.blocking_wall_time_ms += self.blocking.tick(now);
		}
		for (&(mechanic, mode), interval) in &mut self.mechanics {
			if mode != background {
				continue;
			}
			let elapsed = interval.tick(now);
			if elapsed == 0 {
				continue;
			}
			let usage = mechanic.usage_mut(usage);
			if background {
				usage.background_wall_time_ms += elapsed;
			} else {
				usage.blocking_wall_time_ms += elapsed;
			}
		}
	}
	fn enter(&mut self, mechanic: Mechanic, background: bool, now: Instant) {
		if background {
			self.background.enter(now);
		} else {
			self.blocking.enter(now);
		}
		self.mechanics
			.entry((mechanic, background))
			.or_default()
			.enter(now);
	}
	fn leave(&mut self, mechanic: Mechanic, background: bool) {
		if background {
			self.background.leave();
		} else {
			self.blocking.leave();
		}
		self.mechanics
			.get_mut(&(mechanic, background))
			.expect("active mechanic interval")
			.leave();
	}
}

#[derive(Default)]
struct Bank {
	usage: SupervisorAccounting,
	clock: Clock,
	journal: Option<Journal>,
}

impl Bank {
	fn publish(&mut self) -> anyhow::Result<()> {
		let journal = self.journal.as_ref().expect("detached bank owns a journal");
		let id = uuid::Uuid::new_v4().to_string();
		if let Some(path) = &journal.session_file {
			let entry = serde_json::json!({"type":"BACKGROUND_SUPERVISOR", "id":id, "accounting":self.usage});
			crate::session::persistence::append_to_session_file(path, &entry.to_string())?;
		}
		completed()
			.lock()
			.expect("completed supervisor bank poisoned")
			.entry(journal.owner.clone())
			.or_default()
			.insert(id, std::mem::take(&mut self.usage));
		Ok(())
	}

	fn take(&mut self) -> SupervisorAccounting {
		self.clock.tick(Instant::now(), &mut self.usage);
		std::mem::take(&mut self.usage)
	}
}

// Shared session clocks partition overlapping detached runs into disjoint
// increments. Each increment lands in one live bank or durable job receipt.
fn with_clock<T>(
	bank: &Arc<Mutex<Bank>>,
	clock_bank: &Arc<Mutex<Bank>>,
	now: impl FnOnce() -> Instant,
	f: impl FnOnce(&mut Clock, &mut SupervisorAccounting, Instant) -> T,
) -> T {
	let mut clock_bank_guard = clock_bank.lock().expect("supervisor clock poisoned");
	let now = now();
	if Arc::ptr_eq(bank, clock_bank) {
		let Bank { clock, usage, .. } = &mut *clock_bank_guard;
		clock.tick(now, usage);
		f(clock, usage, now)
	} else {
		let mut bank = bank.lock().expect("supervisor accounting bank poisoned");
		let Bank { clock, usage, .. } = &mut *clock_bank_guard;
		clock.tick_mode(now, usage, false);
		clock.tick_mode(now, &mut bank.usage, true);
		f(clock, &mut bank.usage, now)
	}
}

type Banks = HashMap<Option<SessionId>, Arc<Mutex<Bank>>>;

#[cfg(test)]
tokio::task_local! { static TEST_PENDING: Arc<Mutex<Banks>>; }

fn pending() -> Arc<Mutex<Banks>> {
	#[cfg(test)]
	if let Ok(banks) = TEST_PENDING.try_with(Arc::clone) {
		return banks;
	}
	static BANK: OnceLock<Arc<Mutex<Banks>>> = OnceLock::new();
	Arc::clone(BANK.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))))
}

#[cfg(test)]
pub(crate) async fn isolated<F: std::future::Future>(future: F) -> F::Output {
	crate::session::external_spend::isolated(TEST_PENDING.scope(Arc::default(), future)).await
}

fn session_bank() -> Arc<Mutex<Bank>> {
	Arc::clone(
		pending()
			.lock()
			.expect("supervisor accounting bank poisoned")
			.entry(current_session_id())
			.or_default(),
	)
}

fn current_bank() -> Arc<Mutex<Bank>> {
	match DETACHED_USAGE.try_with(Arc::clone) {
		Ok(bank) => bank,
		Err(_) => session_bank(),
	}
}

pub fn record_outcome(mechanic: Mechanic, outcome: Outcome, count: u64) {
	if count == 0 {
		return;
	}
	let bank = current_bank();
	let mut bank = bank.lock().expect("supervisor accounting bank poisoned");
	*mechanic
		.usage_mut(&mut bank.usage)
		.outcome
		.entry(outcome)
		.or_default() += count;
}

pub(super) fn record(kind: CallKind, usage: Usage) {
	current_bank()
		.lock()
		.expect("supervisor accounting bank poisoned")
		.usage
		.kinds
		.entry(kind)
		.or_default()
		.add(&usage);
}

pub(super) fn record_seam(seam: Seam, usage: Usage) {
	current_bank()
		.lock()
		.expect("supervisor accounting bank poisoned")
		.usage
		.seams
		.entry(seam)
		.or_default()
		.add(&usage);
}

fn take_from(banks: &mut Banks, owner: Option<SessionId>) -> SupervisorAccounting {
	let mut result = SupervisorAccounting::default();
	let keys = if owner.is_some() {
		vec![owner, None]
	} else {
		vec![None]
	};
	for key in keys {
		let remove = if let Some(bank) = banks.get(&key) {
			// A recorder can hold this bank before acquiring its lock. The registry
			// lock prevents new lookups while we check that only the map owns it.
			let exclusively_owned = Arc::strong_count(bank) == 1;
			let mut bank = bank.lock().expect("supervisor accounting bank poisoned");
			result.add(&bank.take());
			exclusively_owned
		} else {
			false
		};
		if remove {
			banks.remove(&key);
		}
	}
	result
}

pub fn take_for(owner: &str) -> SupervisorAccounting {
	take_from(
		&mut pending()
			.lock()
			.expect("supervisor accounting bank poisoned"),
		Some(owner.to_owned()),
	)
}

#[cfg(test)]
pub(crate) fn take() -> SupervisorAccounting {
	take_from(
		&mut pending()
			.lock()
			.expect("supervisor accounting bank poisoned"),
		current_session_id(),
	)
}

pub struct WallTimer {
	bank: Arc<Mutex<Bank>>,
	clock_bank: Arc<Mutex<Bank>>,
	mechanic: Mechanic,
	seam: Option<Seam>,
	background: bool,
	started: Instant,
}

impl WallTimer {
	pub fn start(kind: CallKind) -> Self {
		Self::new(
			Mechanic::from_kind(kind).expect("evaluation timers require a seam"),
			None,
		)
	}
	pub fn seam(seam: Seam) -> Self {
		Self::new(Mechanic::from_seam(seam), Some(seam))
	}
	fn new(mechanic: Mechanic, seam: Option<Seam>) -> Self {
		let bank = current_bank();
		let clock_bank = session_bank();
		let background = BACKGROUND.try_with(|value| *value).unwrap_or(false);
		let started = with_clock(&bank, &clock_bank, Instant::now, |clock, _usage, now| {
			clock.enter(mechanic, background, now);
			now
		});
		Self {
			bank,
			clock_bank,
			mechanic,
			seam,
			background,
			started,
		}
	}
}

impl Drop for WallTimer {
	fn drop(&mut self) {
		with_clock(
			&self.bank,
			&self.clock_bank,
			Instant::now,
			|clock, usage, now| {
				clock.leave(self.mechanic, self.background);
				// Keep the raw seam view for persisted journals. Standalone seams already
				// own their mechanic interval, so adding their duration again would duplicate it.
				if let Some(seam) = self.seam {
					if !matches!(seam, Seam::Skills | Seam::Capabilities | Seam::Compression) {
						let elapsed = now.duration_since(self.started).as_millis() as u64;
						let usage = usage.seams.entry(seam).or_default();
						if self.background {
							usage.background_wall_time_ms += elapsed;
						} else {
							usage.blocking_wall_time_ms += elapsed;
						}
					}
				}
			},
		);
	}
}

#[cfg(test)]
pub(crate) fn snapshot_owned() -> SupervisorAccounting {
	current_bank()
		.lock()
		.expect("supervisor accounting bank poisoned")
		.usage
		.clone()
}

#[cfg(test)]
#[path = "accounting_tests.rs"]
mod tests;
