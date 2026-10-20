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

//! Tap-run job registry — tracks agents launched via the `tap` core tool.
//!
//! This is intentionally **separate** from `BackgroundJobManager` (which
//! tracks `agent_*` runs). Tap-runs and `agent_*` are two different runtime
//! concepts that happen to share the same underlying tokio + watch-channel
//! cancellation primitives. The registries are not unified.
//!
//! Lifecycle:
//! - `init_for_session()` — called once per session by `init_session_services`.
//! - `register_job(...)` — called when `tap run` spawns or starts a turn.
//! - `find_job(id)` / `list_jobs()` — read access for `tap list` / `tap stop`.
//! - `cancel_job(id)` — sends `true` over the job's cancel watch channel.
//! - `clear_for_session(id)` — kills every job and drops the registry entry
//!   when the parent session ends. Disk-side state, if any, survives — this
//!   only kills in-memory tasks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use tokio::sync::watch;

use crate::session::context::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapJobStatus {
	Running,
	Done,
	Failed,
	Cancelled,
}

impl TapJobStatus {
	pub fn as_str(self) -> &'static str {
		match self {
			TapJobStatus::Running => "running",
			TapJobStatus::Done => "done",
			TapJobStatus::Failed => "failed",
			TapJobStatus::Cancelled => "cancelled",
		}
	}
}

/// Live, in-flight view of a run — pushed from the child's ACP stream as it
/// works (tool calls, message chunks, per-API-call usage), ahead of the
/// on-disk session snapshot which only flushes per completed message.
#[derive(Debug, Clone, Default)]
pub struct TapLiveState {
	/// Last streamed step — a tool call (`name hint`) or assistant text.
	pub last_action: Option<String>,
	/// Cumulative usage from the child's latest `octomind.usage` notification.
	pub usage: Option<TapLiveUsage>,
}

#[derive(Debug, Clone, Copy)]
pub struct TapLiveUsage {
	pub input_tokens: u64,
	pub output_tokens: u64,
	pub cache_read_tokens: u64,
	pub cost: f64,
}

/// A single tap-run, tracked from spawn through completion.
///
/// `cancel_tx` carries the abort signal — the ACP subprocess driver holds a
/// paired receiver and kills the child when the value flips to `true`.
/// Conversation state lives on disk in the named session file (`<id>.jsonl`),
/// so resume is just a fresh subprocess with `--name <id>`.
pub struct TapJob {
	pub id: String,
	/// Role tag — `category:variant`, e.g. `developer:general`.
	pub role: String,
	/// Working directory the run operates in (defaults to caller's cwd).
	pub workdir: String,
	pub started_at: SystemTime,
	pub status: Arc<RwLock<TapJobStatus>>,
	pub cancel_tx: watch::Sender<bool>,
	pub live: Arc<RwLock<TapLiveState>>,
}

/// Snapshot for read APIs — `Sender` isn't `Clone`.
#[derive(Debug, Clone)]
pub struct TapJobInfo {
	pub id: String,
	pub role: String,
	pub workdir: String,
	pub started_at: SystemTime,
	pub status: TapJobStatus,
	pub live: TapLiveState,
}

struct Registry {
	jobs: HashMap<SessionId, Vec<TapJob>>,
	websocket: HashMap<SessionId, Option<crate::websocket::run_events::RunEvents>>,
}

static REGISTRY: RwLock<Option<Registry>> = RwLock::new(None);

fn with_registry<F, R>(f: F) -> Option<R>
where
	F: FnOnce(&mut Registry) -> R,
{
	let mut guard = REGISTRY.write().ok()?;
	let reg = guard.get_or_insert_with(|| Registry {
		jobs: HashMap::new(),
		websocket: HashMap::new(),
	});
	Some(f(reg))
}

/// Initialise the per-session bucket. Called from `init_session_services`.
pub fn init_for_session() {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return,
	};
	let _ = with_registry(|r| {
		r.websocket.entry(session_id.clone()).or_default();
		r.jobs.entry(session_id).or_default();
	});
}

/// Cancel every running job for the session and drop the bucket. Called
/// from `cleanup_session` on session end.
pub fn clear_for_session(session_id: &SessionId) {
	let removed = {
		let mut guard = match REGISTRY.write() {
			Ok(g) => g,
			Err(_) => return,
		};
		match guard.as_mut() {
			Some(reg) => {
				reg.websocket.remove(session_id);
				reg.jobs.remove(session_id)
			}
			None => None,
		}
	};
	if let Some(jobs) = removed {
		for job in &jobs {
			let _ = job.cancel_tx.send(true);
		}
		// Task handles drop here — the watch signal already told them to bail.
	}
}

/// Register a newly-spawned job for the current session.
pub fn register_job(job: TapJob) {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return,
	};
	let _ = with_registry(|r| {
		r.jobs.entry(session_id).or_default().push(job);
	});
}

/// Look up a job by id (current session). Returns a snapshot — the live
/// fields aren't `Clone`-able so we copy the visible state.
pub fn find_job(id: &str) -> Option<TapJobInfo> {
	let session_id = crate::session::context::current_session_id()?;
	let guard = REGISTRY.read().ok()?;
	let reg = guard.as_ref()?;
	let jobs = reg.jobs.get(&session_id)?;
	let job = jobs.iter().find(|j| j.id == id)?;
	// Bind the inner `read()` guard's result to a local first — otherwise
	// the temporary RwLockReadGuard outlives `guard` in the struct
	// initializer's drop order and the borrow-checker rejects.
	let status = *job.status.read().ok()?;
	let live = job.live.read().ok()?.clone();
	Some(TapJobInfo {
		id: job.id.clone(),
		role: job.role.clone(),
		workdir: job.workdir.clone(),
		started_at: job.started_at,
		status,
		live,
	})
}

/// Returns true if the current session has any tap-runs in the `Running` state.
pub fn has_running_jobs() -> bool {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return false,
	};
	let Ok(guard) = REGISTRY.read() else {
		return false;
	};
	let Some(reg) = guard.as_ref() else {
		return false;
	};
	let Some(jobs) = reg.jobs.get(&session_id) else {
		return false;
	};
	jobs.iter()
		.any(|j| j.status.read().map(|s| *s == TapJobStatus::Running).unwrap_or(false))
}

/// Labels of the current session's still-running tap-runs, `"role (id)"` each,
/// for deterministically reminding the model a specialist is still working —
/// e.g. when a compaction would otherwise drop the launch message.
pub fn pending_labels() -> Vec<String> {
	let mut labels: Vec<String> = list_jobs()
		.into_iter()
		.filter(|j| j.status == TapJobStatus::Running)
		.map(|j| format!("{} ({})", j.role, j.id))
		.collect();
	labels.sort();
	labels
}

/// List all jobs for the current session, newest first.
pub fn list_jobs() -> Vec<TapJobInfo> {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return Vec::new(),
	};
	let guard = match REGISTRY.read() {
		Ok(g) => g,
		Err(_) => return Vec::new(),
	};
	let Some(reg) = guard.as_ref() else {
		return Vec::new();
	};
	let Some(jobs) = reg.jobs.get(&session_id) else {
		return Vec::new();
	};
	let mut out: Vec<TapJobInfo> = jobs
		.iter()
		.filter_map(|j| {
			let status = *j.status.read().ok()?;
			let live = j.live.read().ok()?.clone();
			Some(TapJobInfo {
				id: j.id.clone(),
				role: j.role.clone(),
				workdir: j.workdir.clone(),
				started_at: j.started_at,
				status,
				live,
			})
		})
		.collect();
	out.sort_by_key(|b| std::cmp::Reverse(b.started_at));
	out
}

/// Record the run's latest streamed step (tool call or assistant text).
pub fn record_live_action(id: &str, action: String) {
	with_live(id, |l| l.last_action = Some(action));
}

/// Record cumulative usage pushed by the run's `octomind.usage` notification.
pub fn record_live_usage(id: &str, usage: TapLiveUsage) {
	with_live(id, |l| l.usage = Some(usage));
}

fn with_live(id: &str, f: impl FnOnce(&mut TapLiveState)) {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return;
	};
	let Ok(guard) = REGISTRY.read() else {
		return;
	};
	let Some(reg) = guard.as_ref() else {
		return;
	};
	let Some(jobs) = reg.jobs.get(&session_id) else {
		return;
	};
	let Some(job) = jobs.iter().find(|j| j.id == id) else {
		return;
	};
	if let Ok(mut live) = job.live.write() {
		f(&mut live);
	};
}

/// Send the cancel signal to a running job. Returns the job's status after
/// the call (which may already be `Done`/`Failed` if the job finished
/// before we could signal it).
pub fn cancel_job(id: &str) -> Option<TapJobStatus> {
	let session_id = crate::session::context::current_session_id()?;
	let guard = REGISTRY.read().ok()?;
	let reg = guard.as_ref()?;
	let jobs = reg.jobs.get(&session_id)?;
	let job = jobs.iter().find(|j| j.id == id)?;
	let current = *job.status.read().ok()?;
	if current == TapJobStatus::Running {
		let _ = job.cancel_tx.send(true);
		// The WebSocket buffer belongs to this child until it exits; allowing a
		// resume earlier would mix two children into the same run sequence.
		let streaming = reg
			.websocket
			.get(&session_id)
			.and_then(Option::as_ref)
			.is_some_and(|events| events.is_running(id));
		if !streaming {
			if let Ok(mut s) = job.status.write() {
				*s = TapJobStatus::Cancelled;
			}
		}
		return Some(TapJobStatus::Cancelled);
	}
	Some(current)
}

/// Borrow the live status handle and a fresh cancellation receiver for a
/// running job. Used by `tap run` when a caller resumes by id — we need the
/// shared status cell (so the resumed turn flips it back to terminal) and
/// a cancel receiver subscribed to the existing sender.
pub fn get_status_and_cancel(id: &str) -> Option<(Arc<RwLock<TapJobStatus>>, watch::Receiver<bool>)> {
	let session_id = crate::session::context::current_session_id()?;
	let guard = REGISTRY.read().ok()?;
	let reg = guard.as_ref()?;
	let jobs = reg.jobs.get(&session_id)?;
	let job = jobs.iter().find(|j| j.id == id)?;
	Some((Arc::clone(&job.status), job.cancel_tx.subscribe()))
}

/// Replace a job's cancellation channel with a fresh one (value `false`) and
/// return a receiver for it. Used when resuming a stopped tap-run: `stop`
/// latched the old channel to `true`, so a receiver subscribed to it would
/// cancel the resumed turn before it began.
pub fn reset_cancel(id: &str) -> Option<watch::Receiver<bool>> {
	let session_id = crate::session::context::current_session_id()?;
	let mut guard = REGISTRY.write().ok()?;
	let reg = guard.as_mut()?;
	let jobs = reg.jobs.get_mut(&session_id)?;
	let job = jobs.iter_mut().find(|j| j.id == id)?;
	let (tx, rx) = watch::channel(false);
	job.cancel_tx = tx;
	Some(rx)
}

/// Publish the terminal state before a resume can start a new event buffer.
pub(crate) fn finish_run(id: &str, status: &RwLock<TapJobStatus>, terminal: TapJobStatus, error: Option<String>) {
	let mut guard = REGISTRY.write().expect("tap registry poisoned");
	let mut status = status.write().expect("tap status poisoned");
	if *status == TapJobStatus::Running {
		*status = terminal;
	}
	if let Some(events) = crate::session::context::current_session_id()
		.and_then(|sid| guard.as_mut()?.websocket.get_mut(&sid))
		.and_then(Option::as_mut)
	{
		let parent = crate::session::context::current_session_id().expect("tap parent session");
		events.emit(
			&parent,
			id,
			crate::websocket::protocol::RunEvent::Status { status: *status, error },
		);
	}
}

pub(crate) fn snapshot_run_log(
	request: &crate::websocket::protocol::RunLogMessage,
) -> crate::websocket::protocol::RunLogPayload {
	use crate::websocket::protocol::RunLogPayload;
	let guard = REGISTRY.read().expect("tap registry poisoned");
	let reg = guard.as_ref();
	let job = reg
		.and_then(|r| r.jobs.get(&request.session_id))
		.and_then(|jobs| jobs.iter().find(|job| job.id == request.run_id));
	let status = job.map(|job| *job.status.read().expect("tap status poisoned"));
	let events = reg
		.and_then(|r| r.websocket.get(&request.session_id))
		.and_then(Option::as_ref);
	if status == Some(TapJobStatus::Running) {
		if let Some(snapshot) = events.and_then(|events| events.snapshot(request)) {
			return snapshot;
		}
	}
	RunLogPayload {
		session_id: request.session_id.clone(),
		run_id: request.run_id.clone(),
		role: job.map(|job| job.role.clone()),
		status: status.map_or("unknown", TapJobStatus::as_str).to_string(),
		source: "disk".to_string(),
		seq: events.map_or(0, |events| events.last_seq(&request.run_id)),
		truncated: false,
		events: Vec::new(),
		error: None,
		request_id: request.request_id.clone(),
	}
}

/// Generated run IDs are filenames, unlike package tap IDs (`owner/repo`).
pub fn validate_run_id(id: &str) -> Result<(), String> {
	let valid = id
		.strip_prefix("tap-")
		.and_then(|rest| rest.rsplit_once('-'))
		.is_some_and(|(role, hash)| {
			!role.is_empty()
				&& role
					.bytes()
					.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
				&& hash.len() == 6
				&& hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
		});
	if valid {
		Ok(())
	} else {
		Err(format!("invalid run_id: {id}"))
	}
}

pub(crate) fn enable_run_events(sender: Option<crate::websocket::server::WsSender>) {
	let session_id = crate::session::context::current_session_id().expect("bound session context");
	let mut guard = REGISTRY.write().expect("tap registry poisoned");
	let events = guard
		.as_mut()
		.expect("tap registry initialized")
		.websocket
		.get_mut(&session_id)
		.expect("session services initialized")
		.get_or_insert_with(Default::default);
	if let Some(sender) = sender {
		events.bind(sender);
	}
}

pub(crate) fn unbind_run_events(sender: &crate::websocket::server::WsSender) {
	let mut guard = REGISTRY.write().expect("tap registry poisoned");
	if let Some(reg) = guard.as_mut() {
		for events in reg.websocket.values_mut().flatten() {
			events.unbind(sender);
		}
	}
}

/// Returns None on other transports, including ACP's channel-based output.
pub(crate) fn with_run_events<R>(f: impl FnOnce(&mut crate::websocket::run_events::RunEvents, &str) -> R) -> Option<R> {
	let session_id = crate::session::context::current_session_id()?;
	let mut guard = REGISTRY.write().expect("tap registry poisoned");
	let events = guard.as_mut()?.websocket.get_mut(&session_id)?.as_mut()?;
	Some(f(events, &session_id))
}

/// Generate a fresh tap-run id from a role tag — `tap-<role-with-dash>-<6hex>`.
///
/// The hash component is taken from process id + monotonic counter to
/// avoid collisions inside a single session without pulling a uuid crate.
pub fn generate_id(role: &str) -> String {
	use std::sync::atomic::{AtomicU64, Ordering};
	static COUNTER: AtomicU64 = AtomicU64::new(0);
	let n = COUNTER.fetch_add(1, Ordering::Relaxed);
	let pid = std::process::id() as u64;
	let now = SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_nanos() as u64)
		.unwrap_or(0);
	let h = pid ^ now ^ (n.wrapping_mul(0x9E37_79B9_7F4A_7C15));
	let slug: String = role
		.chars()
		.map(|c| match c {
			'a'..='z' | '0'..='9' => c,
			'A'..='Z' => c.to_ascii_lowercase(),
			_ => '-',
		})
		.collect();
	format!("tap-{slug}-{:06x}", (h & 0xFF_FFFF))
}

#[cfg(test)]
#[path = "tap_runs_tests.rs"]
mod tests;
