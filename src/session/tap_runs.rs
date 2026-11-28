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

use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use tokio::sync::{mpsc, watch};

use crate::session::context::{SessionId, SessionMode};

const MAX_PENDING_PROMPTS: usize = 32;

/// Why a run saved as running comes back failed when its session is read from
/// disk: the process that ran it stopped (a restart, or the session unloaded)
/// before the run finished.
const INTERRUPTED_ERROR: &str = "interrupted: the process running it stopped before it finished";

pub struct TapRunInput<'a> {
	pub id: &'a str,
	pub prompts: mpsc::Receiver<String>,
	pub(crate) context_grant: Option<uuid::Uuid>,
}

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
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TapLiveState {
	/// Last streamed step — a tool call (`name hint`) or assistant text.
	pub last_action: Option<String>,
	/// Cumulative usage from the child's latest `octomind.usage` notification.
	pub usage: Option<TapLiveUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
	/// Role tag — an agent's tag, e.g. `developer` or `developer:rust`, or `workflow:<name>` for a workflow run.
	pub role: String,
	/// Working directory the run operates in (defaults to caller's cwd).
	pub workdir: String,
	/// Read-only and copy flags the run was started with; a resume keeps them.
	pub mode: SessionMode,
	pub started_at: SystemTime,
	pub status: Arc<RwLock<TapJobStatus>>,
	pub error: Option<String>,
	pub cancel_tx: watch::Sender<bool>,
	pub live: Arc<RwLock<TapLiveState>>,
}

/// Snapshot for read APIs — `Sender` isn't `Clone`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TapJobInfo {
	pub id: String,
	pub role: String,
	pub workdir: String,
	#[serde(default)]
	pub mode: SessionMode,
	pub started_at: SystemTime,
	pub status: TapJobStatus,
	pub error: Option<String>,
	pub live: TapLiveState,
}

/// Linear in `history` + `runs`: the log fold calls this once per snapshot over
/// a history of hundreds of runs, so a per-run scan would be quadratic.
pub(crate) fn merge_history(history: &mut Vec<TapJobInfo>, runs: impl IntoIterator<Item = TapJobInfo>) {
	let mut index: HashMap<String, usize> = HashMap::with_capacity(history.len());
	for (position, saved) in history.iter().enumerate() {
		index.entry(saved.id.clone()).or_insert(position);
	}
	for mut run in runs {
		if let Some(&position) = index.get(&run.id) {
			let previous = &mut history[position];
			match (&mut run.live.usage, previous.live.usage) {
				(Some(usage), Some(old)) => {
					usage.cost = usage.cost.max(old.cost);
					usage.input_tokens = usage.input_tokens.max(old.input_tokens);
					usage.output_tokens = usage.output_tokens.max(old.output_tokens);
					usage.cache_read_tokens = usage.cache_read_tokens.max(old.cache_read_tokens);
				}
				(None, old) => run.live.usage = old,
				_ => {}
			}
			if run.live.last_action.is_none() {
				run.live.last_action = previous.live.last_action.clone();
			}
			*previous = run;
		} else {
			index.insert(run.id.clone(), history.len());
			history.push(run);
		}
	}
}

/// What one session has already written to its log in this process. Never
/// persisted: after a restart the first snapshot writes both lists in full once.
#[derive(Default)]
struct LoggedState {
	runs: HashMap<String, TapJobInfo>,
	background_runs: BTreeSet<String>,
}

static LOGGED_RUNS: Mutex<Option<HashMap<SessionId, LoggedState>>> = Mutex::new(None);

fn lock_logged_runs() -> std::sync::MutexGuard<'static, Option<HashMap<SessionId, LoggedState>>> {
	// The map is only a write-dedup cache; a poisoned lock holds no broken invariant.
	LOGGED_RUNS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Trim a snapshot copy to what this process has not logged yet: runs that are
/// new or changed, and background-run ids not yet written. Snapshots carry only
/// these; the loader folds every snapshot (`merge_history` for runs, `merge_max`
/// for the accounting), so the full lists are rebuilt from the deltas. Call
/// `mark_logged` on the same copy once the entry is appended, so a failed append
/// is retried by the next snapshot.
pub(crate) fn keep_unlogged(info: &mut crate::session::SessionInfo) {
	let guard = lock_logged_runs();
	let Some(logged) = guard.as_ref().and_then(|sessions| sessions.get(&info.name)) else {
		return;
	};
	info.agent_runs.retain(|run| logged.runs.get(&run.id) != Some(run));
	info.supervisor_accounting
		.background_runs
		.retain(|id| !logged.background_runs.contains(id));
}

/// Record what a trimmed snapshot copy carried as logged for its session.
pub(crate) fn mark_logged(info: &crate::session::SessionInfo) {
	let mut guard = lock_logged_runs();
	let logged = guard
		.get_or_insert_with(HashMap::new)
		.entry(info.name.clone())
		.or_default();
	for run in &info.agent_runs {
		logged.runs.insert(run.id.clone(), run.clone());
	}
	logged
		.background_runs
		.extend(info.supervisor_accounting.background_runs.iter().cloned());
}

/// Old sessions have no agent snapshot; their structured tap receipts retain run IDs.
pub(crate) fn recover_job(message: &crate::session::Message) -> Option<TapJobInfo> {
	if message.role != "tool" || message.name.as_deref() != Some("tap") {
		return None;
	}
	let receipt: serde_json::Value = serde_json::from_str(&message.content).ok()?;
	let id = receipt.get("id")?.as_str()?.to_owned();
	let workdir = receipt.get("workdir")?.as_str()?.to_owned();
	let role = match receipt.get("role").and_then(|role| role.as_str()) {
		Some(role) => role.to_owned(),
		None => format!("workflow:{}", receipt.get("workflow")?.as_str()?),
	};
	let status = match receipt.get("status") {
		Some(status) => serde_json::from_value(status.clone()).ok()?,
		None => TapJobStatus::Running,
	};
	Some(TapJobInfo {
		id,
		role,
		workdir,
		status,
		started_at: std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(message.timestamp))?,
		error: None,
		live: TapLiveState::default(),
		mode: SessionMode {
			readonly: receipt.get("readonly").and_then(|flag| flag.as_bool()).unwrap_or(false),
			copy: receipt.get("copy").and_then(|flag| flag.as_bool()).unwrap_or(false),
		},
	})
}

struct Registry {
	jobs: HashMap<SessionId, Vec<TapJob>>,
	prompts: HashMap<SessionId, HashMap<String, mpsc::Sender<String>>>,
	websocket: HashMap<SessionId, Option<crate::websocket::run_events::RunEvents>>,
	/// Runs found cut off when the session was read from disk, waiting for
	/// `init_for_session` to queue their handbacks once the inbox exists.
	interrupted: HashMap<SessionId, Vec<TapJobInfo>>,
}

static REGISTRY: RwLock<Option<Registry>> = RwLock::new(None);

fn with_registry<F, R>(f: F) -> Option<R>
where
	F: FnOnce(&mut Registry) -> R,
{
	let mut guard = REGISTRY.write().ok()?;
	let reg = guard.get_or_insert_with(|| Registry {
		jobs: HashMap::new(),
		prompts: HashMap::new(),
		websocket: HashMap::new(),
		interrupted: HashMap::new(),
	});
	Some(f(reg))
}

/// Initialise the per-session bucket. Called from `init_session_services`.
///
/// Also hands back the runs `restore_runs` found cut off: the inbox only exists
/// from `init_session_services` on, so their handbacks are queued here.
pub fn init_for_session() {
	let session_id = match crate::session::context::current_session_id() {
		Some(id) => id,
		None => return,
	};
	let interrupted = with_registry(|r| {
		r.prompts.entry(session_id.clone()).or_default();
		r.websocket.entry(session_id.clone()).or_default();
		r.jobs.entry(session_id.clone()).or_default();
		r.interrupted.remove(&session_id).unwrap_or_default()
	})
	.unwrap_or_default();
	for run in interrupted {
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			content: format!(
				"[Tap-run '{}' ({}) interrupted]\n\n{}, so no reply is coming. Its transcript is kept: \
				 call tap(action=\"run\", session=\"{}\", prompt=...) to resume it, or finish the task without it.",
				run.id, run.role, INTERRUPTED_ERROR, run.id
			),
			source: crate::session::inbox::InboxSource::TapRun {
				id: run.id,
				role: run.role,
			},
		});
	}
}

/// Put a session's saved runs back into the registry when the session is read
/// from disk, so `send`, `stop` and resume find them again. A run saved as
/// running stopped with the process that ran it: it comes back failed, and its
/// handback waits for `init_for_session`.
pub fn restore_runs(session_id: &SessionId, runs: &[TapJobInfo]) {
	let _ = with_registry(|r| {
		let jobs = r.jobs.entry(session_id.clone()).or_default();
		for run in runs {
			// A live run of this session already speaks for itself.
			if jobs.iter().any(|job| job.id == run.id) {
				continue;
			}
			let mut run = run.clone();
			if run.status == TapJobStatus::Running {
				run.status = TapJobStatus::Failed;
				run.error = Some(INTERRUPTED_ERROR.to_string());
				r.interrupted.entry(session_id.clone()).or_default().push(run.clone());
			}
			jobs.push(TapJob {
				id: run.id,
				role: run.role,
				workdir: run.workdir,
				mode: run.mode,
				started_at: run.started_at,
				status: Arc::new(RwLock::new(run.status)),
				error: run.error,
				cancel_tx: watch::channel(false).0,
				live: Arc::new(RwLock::new(run.live)),
			});
		}
	});
	crate::log_debug!("Restored {} tap runs for session '{}'", runs.len(), session_id);
}

/// Cancel every running job for the session and drop the bucket. Called
/// from `cleanup_session` on session end.
pub fn clear_for_session(session_id: &SessionId) {
	if let Some(sessions) = lock_logged_runs().as_mut() {
		sessions.remove(session_id);
	}
	let removed = {
		let mut guard = match REGISTRY.write() {
			Ok(g) => g,
			Err(_) => return,
		};
		match guard.as_mut() {
			Some(reg) => {
				reg.prompts.remove(session_id);
				reg.websocket.remove(session_id);
				reg.interrupted.remove(session_id);
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
		r.prompts.entry(session_id.clone()).or_default();
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
		mode: job.mode,
		started_at: job.started_at,
		status,
		error: job.error.clone(),
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
/// for deterministically reminding the model an agent is still working —
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
	list_jobs_for(&session_id)
}

pub fn list_jobs_for(session_id: &str) -> Vec<TapJobInfo> {
	let guard = match REGISTRY.read() {
		Ok(g) => g,
		Err(_) => return Vec::new(),
	};
	let Some(reg) = guard.as_ref() else {
		return Vec::new();
	};
	let Some(jobs) = reg.jobs.get(session_id) else {
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
				mode: j.mode,
				started_at: j.started_at,
				status,
				error: j.error.clone(),
				live,
			})
		})
		.collect();
	out.sort_by_key(|b| std::cmp::Reverse(b.started_at));
	out
}

pub(crate) fn open_prompt_channel(id: &str) -> Result<mpsc::Receiver<String>> {
	let session_id = crate::session::context::current_session_id().context("tap prompts require an active session")?;
	let mut guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let reg = guard.as_mut().context("tap registry is not initialized")?;
	let prompts = reg
		.prompts
		.get_mut(&session_id)
		.context("tap prompt registry is not initialized for this session")?;
	anyhow::ensure!(
		prompts.get(id).is_none_or(mpsc::Sender::is_closed),
		"run {id} already accepts prompts"
	);
	let (tx, rx) = mpsc::channel(MAX_PENDING_PROMPTS);
	prompts.insert(id.to_string(), tx);
	Ok(rx)
}

pub(crate) fn send_prompt(id: &str, prompt: String) -> Result<()> {
	let session_id = crate::session::context::current_session_id().context("tap send requires an active session")?;
	let guard = REGISTRY.read().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let reg = guard.as_ref().context("tap registry is not initialized")?;
	let job = reg
		.jobs
		.get(&session_id)
		.and_then(|jobs| jobs.iter().find(|job| job.id == id))
		.with_context(|| format!("No tap-run with id '{id}' in this session."))?;
	let status = job.status.read().map_err(|_| anyhow::anyhow!("tap status poisoned"))?;
	anyhow::ensure!(
		*status == TapJobStatus::Running,
		"run {id} is {}; use run to resume it",
		status.as_str()
	);
	let sender = reg
		.prompts
		.get(&session_id)
		.and_then(|prompts| prompts.get(id))
		.with_context(|| format!("run {id} does not accept prompts (workflow or finishing run)"))?;
	sender.try_send(prompt).map_err(|error| match error {
		mpsc::error::TrySendError::Full(_) => anyhow::anyhow!("run {id} has {MAX_PENDING_PROMPTS} queued prompts"),
		mpsc::error::TrySendError::Closed(_) => anyhow::anyhow!("run {id} is finishing and no longer accepts prompts"),
	})
}

/// Admission and the final empty check share the registry lock: an accepted
/// prompt is either taken here or prevents closing the child's input.
pub(crate) fn take_prompt_or_close(input: &mut TapRunInput<'_>) -> Result<Option<String>> {
	let _guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	match input.prompts.try_recv() {
		Ok(prompt) => Ok(Some(prompt)),
		Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
			input.prompts.close();
			Ok(None)
		}
	}
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

#[derive(Debug)]
pub(crate) struct TapRunClaim {
	pub role: String,
	pub workdir: String,
	pub mode: SessionMode,
	pub status: Arc<RwLock<TapJobStatus>>,
	pub cancel_rx: watch::Receiver<bool>,
}

/// Lock order matches finish_run: registry, then status. The event buffer and
/// cancellation receiver must belong to the same, exclusively claimed turn.
pub(crate) fn resume_run(id: &str) -> Result<TapRunClaim> {
	let missing = || anyhow::anyhow!("No tap-run with id '{id}' in this session.");
	let session_id = crate::session::context::current_session_id().ok_or_else(missing)?;
	let mut guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let reg = guard.as_mut().ok_or_else(missing)?;
	let job = reg
		.jobs
		.get_mut(&session_id)
		.and_then(|jobs| jobs.iter_mut().find(|job| job.id == id))
		.ok_or_else(missing)?;
	let mut status = job.status.write().map_err(|_| anyhow::anyhow!("tap status poisoned"))?;
	anyhow::ensure!(
		*status != TapJobStatus::Running,
		"run {id} is already running; use send to queue a prompt or stop to cancel it."
	);
	if let Some(events) = reg.websocket.get_mut(&session_id).and_then(Option::as_mut) {
		events.begin(&session_id, id, &job.role)?;
	}
	*status = TapJobStatus::Running;
	job.error = None;
	let (cancel_tx, cancel_rx) = watch::channel(false);
	job.cancel_tx = cancel_tx;
	Ok(TapRunClaim {
		role: job.role.clone(),
		workdir: job.workdir.clone(),
		mode: job.mode,
		status: Arc::clone(&job.status),
		cancel_rx,
	})
}

/// Publish the terminal state before a resume can start a new event buffer.
pub(crate) fn finish_run(
	id: &str,
	status: &RwLock<TapJobStatus>,
	terminal: TapJobStatus,
	error: Option<String>,
) -> Result<()> {
	let mut guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let mut status = status.write().map_err(|_| anyhow::anyhow!("tap status poisoned"))?;
	if *status == TapJobStatus::Running {
		*status = terminal;
	}
	if let Some(parent) = crate::session::context::current_session_id() {
		if let Some(reg) = guard.as_mut() {
			if let Some(job) = reg
				.jobs
				.get_mut(&parent)
				.and_then(|jobs| jobs.iter_mut().find(|job| job.id == id))
			{
				job.error = error.clone();
			}
			if let Some(events) = reg.websocket.get_mut(&parent).and_then(Option::as_mut) {
				events.emit(
					&parent,
					id,
					crate::websocket::protocol::RunEvent::Status { status: *status, error },
				)?;
			}
		}
	}
	Ok(())
}

pub(crate) fn snapshot_run_log(
	request: &crate::websocket::protocol::RunLogMessage,
) -> Result<crate::websocket::protocol::RunLogPayload> {
	use crate::websocket::protocol::{RunEvent, RunLogPayload};
	let guard = REGISTRY.read().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let reg = guard.as_ref();
	let job = reg
		.and_then(|r| r.jobs.get(&request.session_id))
		.and_then(|jobs| jobs.iter().find(|job| job.id == request.run_id));
	let status = job
		.map(|job| {
			job.status
				.read()
				.map(|status| *status)
				.map_err(|_| anyhow::anyhow!("tap status poisoned"))
		})
		.transpose()?;
	let events = reg
		.and_then(|r| r.websocket.get(&request.session_id))
		.and_then(Option::as_ref);
	if status == Some(TapJobStatus::Running) {
		if let Some(snapshot) = events.and_then(|events| events.snapshot(request)) {
			return Ok(snapshot);
		}
	}
	Ok(RunLogPayload {
		session_id: request.session_id.clone(),
		run_id: request.run_id.clone(),
		role: job.map(|job| job.role.clone()),
		status: status.map_or("unknown", TapJobStatus::as_str).to_string(),
		source: "disk".to_string(),
		seq: events.map_or(0, |events| events.last_seq(&request.run_id)),
		truncated: false,
		events: status
			.map(|status| RunEvent::Status {
				status,
				error: job.and_then(|job| job.error.clone()),
			})
			.into_iter()
			.collect(),
		error: None,
		request_id: request.request_id.clone(),
	})
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

pub(crate) fn enable_run_events(sender: Option<crate::websocket::server::WsSender>) -> Result<()> {
	let session_id = crate::session::context::current_session_id().context("bound session context missing")?;
	let mut guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let events = guard
		.as_mut()
		.context("tap registry not initialized")?
		.websocket
		.get_mut(&session_id)
		.context("session services not initialized")?
		.get_or_insert_with(Default::default);
	if let Some(sender) = sender {
		events.bind(sender);
	}
	Ok(())
}

pub(crate) fn unbind_run_events(sender: &crate::websocket::server::WsSender) -> Result<()> {
	let mut guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	if let Some(reg) = guard.as_mut() {
		for events in reg.websocket.values_mut().flatten() {
			events.unbind(sender);
		}
	}
	Ok(())
}

/// Returns None on other transports, including ACP's channel-based output.
pub(crate) fn with_run_events<R>(
	f: impl FnOnce(&mut crate::websocket::run_events::RunEvents, &str) -> Result<R>,
) -> Result<Option<R>> {
	let Some(session_id) = crate::session::context::current_session_id() else {
		return Ok(None);
	};
	let mut guard = REGISTRY.write().map_err(|_| anyhow::anyhow!("tap registry poisoned"))?;
	let Some(events) = guard
		.as_mut()
		.and_then(|reg| reg.websocket.get_mut(&session_id))
		.and_then(Option::as_mut)
	else {
		return Ok(None);
	};
	f(events, &session_id).map(Some)
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
