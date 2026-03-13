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
use std::sync::Arc;
use tokio::sync::mpsc;

/// Outcome of a completed background agent run.
#[derive(Debug, Clone)]
pub struct CompletedJob {
	pub agent_name: String,
	/// Full output from the agent, or an error description prefixed with "ERROR: ".
	pub output: String,
}

/// Tracks active job count and holds the sender for pushing completions to the session.
#[derive(Clone)]
pub struct BackgroundJobManager {
	active: Arc<AtomicUsize>,
	max_concurrent: usize,
	tx: mpsc::Sender<CompletedJob>,
}

impl BackgroundJobManager {
	pub fn new(max_concurrent: usize) -> (Self, mpsc::Receiver<CompletedJob>) {
		let (tx, rx) = mpsc::channel(64);
		let mgr = Self {
			active: Arc::new(AtomicUsize::new(0)),
			max_concurrent,
			tx,
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

	pub fn active_count(&self) -> usize {
		self.active.load(Ordering::SeqCst)
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
