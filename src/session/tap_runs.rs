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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// Snapshot for read APIs — `Sender` isn't `Clone`.
#[derive(Debug, Clone)]
pub struct TapJobInfo {
	pub id: String,
	pub role: String,
	pub workdir: String,
	pub started_at: SystemTime,
	pub status: TapJobStatus,
}

struct Registry {
	jobs: HashMap<SessionId, Vec<TapJob>>,
}

static REGISTRY: RwLock<Option<Registry>> = RwLock::new(None);

fn with_registry<F, R>(f: F) -> Option<R>
where
	F: FnOnce(&mut Registry) -> R,
{
	let mut guard = REGISTRY.write().ok()?;
	let reg = guard.get_or_insert_with(|| Registry {
		jobs: HashMap::new(),
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
			Some(reg) => reg.jobs.remove(session_id),
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
	Some(TapJobInfo {
		id: job.id.clone(),
		role: job.role.clone(),
		workdir: job.workdir.clone(),
		started_at: job.started_at,
		status,
	})
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
			Some(TapJobInfo {
				id: j.id.clone(),
				role: j.role.clone(),
				workdir: j.workdir.clone(),
				started_at: j.started_at,
				status,
			})
		})
		.collect();
	out.sort_by_key(|b| std::cmp::Reverse(b.started_at));
	out
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
		if let Ok(mut s) = job.status.write() {
			*s = TapJobStatus::Cancelled;
		}
		return Some(TapJobStatus::Cancelled);
	}
	Some(current)
}

/// Borrow the live status handle and a fresh cancellation receiver for a
/// running job. Used by `tap run` when a caller resumes by id — we need the
/// shared status cell (so the resumed turn flips it back to terminal) and
/// a cancel receiver subscribed to the existing sender.
pub fn get_status_and_cancel(
	id: &str,
) -> Option<(Arc<RwLock<TapJobStatus>>, watch::Receiver<bool>)> {
	let session_id = crate::session::context::current_session_id()?;
	let guard = REGISTRY.read().ok()?;
	let reg = guard.as_ref()?;
	let jobs = reg.jobs.get(&session_id)?;
	let job = jobs.iter().find(|j| j.id == id)?;
	Some((Arc::clone(&job.status), job.cancel_tx.subscribe()))
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
mod tests {
	use super::*;

	#[test]
	fn id_format_is_stable() {
		let id = generate_id("developer:general");
		assert!(id.starts_with("tap-developer-general-"));
		// 4 (tap-) + 17 (developer-general-) + 6 (hex) = 27
		assert_eq!(id.len(), "tap-developer-general-".len() + 6);
	}

	#[test]
	fn status_strings_are_stable() {
		assert_eq!(TapJobStatus::Running.as_str(), "running");
		assert_eq!(TapJobStatus::Done.as_str(), "done");
		assert_eq!(TapJobStatus::Failed.as_str(), "failed");
		assert_eq!(TapJobStatus::Cancelled.as_str(), "cancelled");
	}
}
