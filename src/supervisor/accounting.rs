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

use super::{evaluate::Seam, stats::CallKind};
use crate::session::context::{current_session_id, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SupervisorAccounting {
	#[serde(default)]
	pub turn_time: TurnTime,
	#[serde(default)]
	pub background_runs: BTreeSet<String>,
	pub kinds: BTreeMap<CallKind, Usage>,
	pub seams: BTreeMap<Seam, Usage>,
	/// Union of blocking intervals; per-kind and seam wall times are inclusive.
	pub blocking_wall_time_ms: u64,
}

impl SupervisorAccounting {
	pub fn add(&mut self, other: &Self) {
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

tokio::task_local! { static BACKGROUND: bool; }

pub async fn background<T>(future: impl std::future::Future<Output = T>) -> T {
	BACKGROUND.scope(true, future).await
}

/// Awaited tool tasks inherit their caller's mode; spawning them does not detach the turn.
pub fn inherit<F: std::future::Future>(future: F) -> impl std::future::Future<Output = F::Output> {
	BACKGROUND.scope(
		BACKGROUND
			.try_with(|background| *background)
			.unwrap_or(false),
		future,
	)
}

#[derive(Default)]
struct Bank {
	usage: SupervisorAccounting,
	blocking: usize,
	active: usize,
	since: Option<Instant>,
}

impl Bank {
	fn tick(&mut self, now: Instant) {
		if let Some(since) = self.since {
			let elapsed = now.duration_since(since).as_millis() as u64;
			self.usage.blocking_wall_time_ms += elapsed;
			// Preserve fractional milliseconds across nested boundaries and folds.
			self.since = Some(since + std::time::Duration::from_millis(elapsed));
		}
	}
}

fn pending() -> &'static Mutex<HashMap<Option<SessionId>, Bank>> {
	static BANK: OnceLock<Mutex<HashMap<Option<SessionId>, Bank>>> = OnceLock::new();
	BANK.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn record(kind: CallKind, usage: Usage) {
	let mut banks = pending()
		.lock()
		.expect("supervisor accounting bank poisoned");
	let bank = banks.entry(current_session_id()).or_default();
	bank.usage.kinds.entry(kind).or_default().add(&usage);
}

pub(super) fn record_seam(seam: Seam, usage: Usage) {
	let mut banks = pending()
		.lock()
		.expect("supervisor accounting bank poisoned");
	banks
		.entry(current_session_id())
		.or_default()
		.usage
		.seams
		.entry(seam)
		.or_default()
		.add(&usage);
}

pub fn take() -> SupervisorAccounting {
	let mut banks = pending()
		.lock()
		.expect("supervisor accounting bank poisoned");
	let owner = current_session_id();
	let mut result = SupervisorAccounting::default();
	let keys = if owner.is_some() {
		vec![owner, None]
	} else {
		vec![None]
	};
	for key in keys {
		if let Some(bank) = banks.get_mut(&key) {
			bank.tick(Instant::now());
			result.add(&std::mem::take(&mut bank.usage));
			if bank.active == 0 {
				banks.remove(&key);
			}
		}
	}
	result
}

pub struct WallTimer {
	owner: Option<SessionId>,
	kind: CallKind,
	seam: Option<Seam>,
	background: bool,
	started: Instant,
}

impl WallTimer {
	pub fn start(kind: CallKind) -> Self {
		Self::new(kind, None)
	}
	pub fn seam(seam: Seam) -> Self {
		Self::new(CallKind::Evaluate, Some(seam))
	}
	fn new(kind: CallKind, seam: Option<Seam>) -> Self {
		let timer = Self {
			owner: current_session_id(),
			kind,
			seam,
			background: BACKGROUND
				.try_with(|background| *background)
				.unwrap_or(false),
			started: Instant::now(),
		};
		let mut banks = pending()
			.lock()
			.expect("supervisor accounting bank poisoned");
		let bank = banks.entry(timer.owner.clone()).or_default();
		bank.active += 1;
		if !timer.background {
			if bank.blocking == 0 {
				bank.since = Some(timer.started);
			}
			bank.blocking += 1;
		}
		timer
	}
}

impl Drop for WallTimer {
	fn drop(&mut self) {
		let now = Instant::now();
		let elapsed = now.duration_since(self.started).as_millis() as u64;
		let mut banks = pending()
			.lock()
			.expect("supervisor accounting bank poisoned");
		let bank = banks
			.get_mut(&self.owner)
			.expect("active supervisor timer owns a bank");
		let mut usage = Usage::default();
		if self.background {
			usage.background_wall_time_ms = elapsed;
		} else {
			usage.blocking_wall_time_ms = elapsed;
			bank.tick(now);
			bank.blocking -= 1;
			if bank.blocking == 0 {
				bank.since = None;
			}
		}
		bank.active -= 1;
		bank.usage.kinds.entry(self.kind).or_default().add(&usage);
		if let Some(seam) = self.seam {
			bank.usage.seams.entry(seam).or_default().add(&usage);
		}
	}
}

#[cfg(test)]
#[path = "accounting_tests.rs"]
mod tests;
