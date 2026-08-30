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

// Async agent job tracking — push model.
// When an async agent finishes, it pushes a message directly into the session
// inbox so the AI sees the result on the next turn without any polling.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Outcome of a completed async agent run.
#[derive(Debug, Clone)]
pub struct CompletedJob {
	pub agent_name: String,
	/// Full output from the agent, or an error description prefixed with "ERROR: ".
	pub output: String,
}

/// Handle for a spawned async job that can be cancelled.
#[derive(Debug)]
pub struct JobHandle {
	/// Cancellation sender - sending true signals the job to abort.
	pub cancel_tx: watch::Sender<bool>,
	/// Task handle for awaiting completion.
	pub task_handle: tokio::task::JoinHandle<()>,
}

/// Tracks active job count and pushes completions directly into the session inbox.
#[derive(Clone, Debug)]
pub struct BackgroundJobManager {
	active: Arc<AtomicUsize>,
	max_concurrent: usize,
	/// Running jobs that can be cancelled on session exit.
	jobs: Arc<Mutex<Vec<JobHandle>>>,
}

impl BackgroundJobManager {
	pub fn new(max_concurrent: usize) -> Self {
		Self {
			active: Arc::new(AtomicUsize::new(0)),
			max_concurrent,
			jobs: Arc::new(Mutex::new(Vec::new())),
		}
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

	/// Call when an async job finishes (success or failure).
	/// Pushes the result directly into the session inbox.
	pub fn release(&self, job: CompletedJob) {
		let content = if job.output.starts_with("ERROR: ") {
			format!(
				"[Async agent '{}' failed]\n\n{}",
				job.agent_name,
				job.output.trim_start_matches("ERROR: ")
			)
		} else {
			format!(
				"[Async agent '{}' completed]\n\n{}",
				job.agent_name, job.output
			)
		};
		crate::session::inbox::push_inbox_message(crate::session::inbox::InboxMessage {
			source: crate::session::inbox::InboxSource::BackgroundAgent {
				name: job.agent_name,
			},
			content,
		});
		// Publish the completion before clearing the final active count. An idle
		// observer therefore always sees either active work or its queued result.
		self.active.fetch_sub(1, Ordering::SeqCst);
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

	/// Wait for all async jobs to complete.
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

	/// Kill all running async jobs immediately.
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
#[path = "background_jobs_tests.rs"]
mod tests;
