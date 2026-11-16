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

//! Session-owned usage of the model calls made on a session's behalf: what
//! plugins spent and what detached conversation folds spent. Banks drain
//! exactly once, like external spend.

pub mod report;

use crate::plugins::protocol;
use crate::session::context::{current_session_id, SessionId};
use crate::session::Session;
use report::Outcome;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

// Every variant stays: session files written by earlier octomind versions name these rows.
/// Which kind of work made a model call, so `/info` can break the count down
/// instead of showing one opaque total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
	/// Recall keyword/query preparation.
	Recall,
	/// Verify-gate completion check.
	Gate,
	/// Current-turn dependency classification and minimal resolution.
	Resolve,
	/// External plan creation and phase transition decision.
	Plan,
	/// End-of-trajectory lesson/orientation extraction.
	Distill,
	/// Tool-output condensation (task-aware narrowing).
	Condense,
	/// Intent-based tool admission, using the shared supervisor profile.
	Authorize,
	/// Calibrated evaluation-model gate (one of the `evaluate::Seam`s).
	Evaluate,
	Retention,
	Evolution,
	Hindsight,
	/// Detached conversation compression.
	Compression,
	/// Model-written observation on a loop or recovery alarm.
	Advise,
}

// Every variant stays: session files written by earlier octomind versions name these rows.
/// The unit of routing, stats, and unavailable outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seam {
	Recall,
	Skills,
	Authorizer,
	Condense,
	Compression,
	Distill,
	Plan,
	Gate,
	Capabilities,
	Evolution,
	Advise,
	Taps,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
	/// Findings of the latest verify-gate run only. Each run replaces the
	/// previous set and a genuine user turn clears it: the text is free-form
	/// verifier prose that never deduplicates, so a session-lifetime union grew
	/// without bound. `Outcome::Reported` keeps the lifetime count.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub reported_findings: Vec<String>,
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
	/// A verify-gate run always records an `Outcome::Reported` count (possibly
	/// zero), so a Usage carrying that key is a genuine latest gate run: its
	/// findings — including the empty set of a clean run — replace the previous
	/// set. Any other Usage (zero-initialized defaults, other kinds, sums of
	/// unrelated spend) never touches the findings.
	fn replaces_findings(&self) -> bool {
		!self.reported_findings.is_empty() || self.outcome.contains_key(&Outcome::Reported)
	}
	fn add(&mut self, other: &Self) {
		if other.replaces_findings() {
			self.reported_findings = other.reported_findings.clone();
		}
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
		if other.replaces_findings() {
			self.reported_findings = other.reported_findings.clone();
		}
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
		self.background_wall_time_ms = self.background_wall_time_ms.max(other.background_wall_time_ms);
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
	/// What plugins spent, summed per label. Kept apart from `kinds` so the
	/// built-in totals do not count it twice.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub plugin_usage: BTreeMap<String, Usage>,
	/// What each plugin says it did, by plugin. A report is already a session
	/// total, so a newer one replaces the older one and is never summed.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub plugin_reports: BTreeMap<String, protocol::Report>,
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
			plugin_usage: BTreeMap::new(),
			plugin_reports: BTreeMap::new(),
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
		self.background_runs.extend(other.background_runs.iter().cloned());
		for (&kind, usage) in &other.kinds {
			self.kinds.entry(kind).or_default().add(usage);
		}
		for (&seam, usage) in &other.seams {
			self.seams.entry(seam).or_default().add(usage);
		}
		for (label, usage) in &other.plugin_usage {
			self.plugin_usage.entry(label.clone()).or_default().add(usage);
		}
		self.plugin_reports.extend(
			other
				.plugin_reports
				.iter()
				.map(|(plugin, report)| (plugin.clone(), report.clone())),
		);
		self.blocking_wall_time_ms += other.blocking_wall_time_ms;
	}
	pub fn merge_max(&mut self, other: &Self) {
		self.background_wall_time_ms = self
			.background_wall_time_ms
			.zip(other.background_wall_time_ms)
			.map(|(a, b)| a.max(b));
		self.turn_time.completed = self.turn_time.completed.max(other.turn_time.completed);
		self.turn_time.total_time_ms = self.turn_time.total_time_ms.max(other.turn_time.total_time_ms);
		self.turn_time.main_api_time_ms = self.turn_time.main_api_time_ms.max(other.turn_time.main_api_time_ms);
		self.turn_time.blocking_supervisor_time_ms = self
			.turn_time
			.blocking_supervisor_time_ms
			.max(other.turn_time.blocking_supervisor_time_ms);
		self.background_runs.extend(other.background_runs.iter().cloned());
		for (&kind, usage) in &other.kinds {
			self.kinds.entry(kind).or_default().merge_max(usage);
		}
		for (&seam, usage) in &other.seams {
			self.seams.entry(seam).or_default().merge_max(usage);
		}
		for (label, usage) in &other.plugin_usage {
			self.plugin_usage.entry(label.clone()).or_default().merge_max(usage);
		}
		self.plugin_reports.extend(
			other
				.plugin_reports
				.iter()
				.map(|(plugin, report)| (plugin.clone(), report.clone())),
		);
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

/// Book what a plugin spent on models into the session: the per-label sums,
/// the dollar total, and the time the agent waited for those calls.
pub fn record_plugin_usage(session: &mut Session, plugin: &str, usage: &protocol::Usage) {
	crate::log_debug!("Plugin '{plugin}' spent {} on '{}'", usage.cost, usage.label);
	let entry = Usage {
		calls: usage.calls,
		input_tokens: usage.input_tokens,
		output_tokens: usage.output_tokens,
		reasoning_tokens: usage.reasoning_tokens,
		cost: usage.cost,
		blocking_wall_time_ms: if usage.blocking { usage.wall_ms } else { 0 },
		background_wall_time_ms: if usage.blocking { 0 } else { usage.wall_ms },
		..Usage::default()
	};
	let accounting = &mut session.info.supervisor_accounting;
	accounting
		.plugin_usage
		.entry(usage.label.clone())
		.or_default()
		.add(&entry);
	accounting.blocking_wall_time_ms += entry.blocking_wall_time_ms;
	session.info.total_cost += usage.cost;
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

fn has_detached_usage() -> bool {
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
	bank.lock().expect("detached supervisor bank poisoned").publish()?;
	result
}

/// Called at the shared provider-response boundaries, after both aggregate and
/// seam usage are recorded, before cancellation can interrupt another call.
pub(crate) fn checkpoint() -> anyhow::Result<()> {
	match DETACHED_USAGE.try_with(Arc::clone) {
		Ok(bank) => bank.lock().expect("detached supervisor bank poisoned").publish(),
		Err(_) => Ok(()),
	}
}

struct Journal {
	owner: SessionId,
	session_file: Option<std::path::PathBuf>,
}

fn completed() -> &'static Mutex<HashMap<SessionId, BTreeMap<String, SupervisorAccounting>>> {
	static COMPLETED: OnceLock<Mutex<HashMap<SessionId, BTreeMap<String, SupervisorAccounting>>>> = OnceLock::new();
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
struct Bank {
	usage: SupervisorAccounting,
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
		std::mem::take(&mut self.usage)
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

fn record(kind: CallKind, usage: Usage) {
	current_bank()
		.lock()
		.expect("supervisor accounting bank poisoned")
		.usage
		.kinds
		.entry(kind)
		.or_default()
		.add(&usage);
}

/// Record one model call's usage, attributed to the kind of work that made it.
pub fn record_call(
	kind: CallKind,
	input_tokens: u64,
	output_tokens: u64,
	reasoning_tokens: u64,
	api_time_ms: u64,
	cost: f64,
) {
	record(
		kind,
		Usage {
			calls: 1,
			input_tokens,
			output_tokens,
			reasoning_tokens,
			api_time_ms,
			cost,
			..Default::default()
		},
	);
	if !has_detached_usage() {
		crate::session::external_spend::record(cost);
	}
}

fn take_from(banks: &mut Banks, owner: Option<SessionId>) -> SupervisorAccounting {
	let mut result = SupervisorAccounting::default();
	let keys = if owner.is_some() { vec![owner, None] } else { vec![None] };
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
		&mut pending().lock().expect("supervisor accounting bank poisoned"),
		Some(owner.to_owned()),
	)
}

#[cfg(test)]
pub(crate) fn take() -> SupervisorAccounting {
	take_from(
		&mut pending().lock().expect("supervisor accounting bank poisoned"),
		current_session_id(),
	)
}

#[cfg(test)]
#[path = "accounting_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "accounting_plugin_tests.rs"]
mod plugin_tests;
