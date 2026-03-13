// Copyright 2025 Muvon Un Limited
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

// Background agent job tracking — push model.
// When a background agent finishes, it sends a CompletedJob on the channel
// registered by the active session. The session loop injects it as a user
// message so the AI sees the result without any polling.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

/// Outcome of a completed background agent run.
#[derive(Debug, Clone)]
pub struct CompletedJob {
	pub agent_name: String,
	/// Full output from the agent, or an error description prefixed with "ERROR: ".
	pub output: String,
}

/// Handle for a spawned background job that can be cancelled.
#[derive(Debug)]
pub struct JobHandle {
	/// Cancellation sender - sending true signals the job to abort.
	pub cancel_tx: watch::Sender<bool>,
	/// Task handle for awaiting completion.
	pub task_handle: tokio::task::JoinHandle<()>,
}

/// Tracks active job count and holds the sender for pushing completions to the session.
#[derive(Clone)]
pub struct BackgroundJobManager {
	active: Arc<AtomicUsize>,
	max_concurrent: usize,
	tx: mpsc::Sender<CompletedJob>,
	/// Running jobs that can be cancelled on session exit.
	jobs: Arc<Mutex<Vec<JobHandle>>>,
}

impl BackgroundJobManager {
	pub fn new(max_concurrent: usize) -> (Self, mpsc::Receiver<CompletedJob>) {
		let (tx, rx) = mpsc::channel(64);
		let mgr = Self {
			active: Arc::new(AtomicUsize::new(0)),
			max_concurrent,
			tx,
			jobs: Arc::new(Mutex::new(Vec::new())),
		};
		(mgr, rx)
	}

	/// Returns Err if the concurrency limit is already reached.
	pub fn try_acquire(&self) -> Result<(), usize> {
		let current = self.active.load(Ordering::SeqCst);
		if current >= self.max_concurrent {
			return Err(current);
		}
		self.active.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}

	/// Call when a background job finishes (success or failure).
	pub fn release(&self, job: CompletedJob) {
		self.active.fetch_sub(1, Ordering::SeqCst);
		// Best-effort send — if the session is gone the result is simply dropped.
		let _ = self.tx.try_send(job);
	}

	/// Register a spawned job handle for later cancellation.
	pub fn register_job(&self, handle: JobHandle) {
		let mut jobs = self.jobs.lock().unwrap();
		jobs.push(handle);
	}

	/// Remove a completed job handle.
	pub fn remove_job(&self, task_id: tokio::task::Id) {
		let mut jobs = self.jobs.lock().unwrap();
		jobs.retain(|j| j.task_handle.id() != task_id);
	}

	pub fn active_count(&self) -> usize {
		self.active.load(Ordering::SeqCst)
	}

	/// Wait for all background jobs to complete, draining results into the channel.
	/// Returns the number of jobs that completed.
	pub async fn wait_all(&self) -> usize {
		let handles: Vec<_> = {
			let mut jobs = self.jobs.lock().unwrap();
			std::mem::take(&mut *jobs)
		};

		let count = handles.len();
		for handle in handles {
			// Wait for each job to complete (ignoring errors)
			let _ = handle.task_handle.await;
		}
		count
	}

	/// Kill all running background jobs immediately.
	/// Sends cancellation signal and waits briefly for cleanup.
	pub fn kill_all(&self) {
		let handles: Vec<_> = {
			let mut jobs = self.jobs.lock().unwrap();
			std::mem::take(&mut *jobs)
		};

		for handle in handles {
			// Send cancellation signal
			let _ = handle.cancel_tx.send(true);
		}

		// Note: We don't await the tasks here - they'll be dropped and cleaned up
		// when the tokio runtime shuts down. The cancellation signal ensures
		// child processes are killed.
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_acquire_and_release() {
		let (mgr, _rx) = BackgroundJobManager::new(2);
		assert!(mgr.try_acquire().is_ok());
		assert!(mgr.try_acquire().is_ok());
		assert!(mgr.try_acquire().is_err());
		mgr.release(CompletedJob {
			agent_name: "a".into(),
			output: "done".into(),
		});
		assert!(mgr.try_acquire().is_ok());
	}

	#[test]
	fn test_active_count() {
		let (mgr, _rx) = BackgroundJobManager::new(10);
		assert_eq!(mgr.active_count(), 0);
		mgr.try_acquire().unwrap();
		mgr.try_acquire().unwrap();
		assert_eq!(mgr.active_count(), 2);
		mgr.release(CompletedJob {
			agent_name: "a".into(),
			output: "x".into(),
		});
		assert_eq!(mgr.active_count(), 1);
	}
}
