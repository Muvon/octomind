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

// Background job tracking for async agent execution.
// Pure state management — no tokio::spawn here. Callers own the async lifecycle.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap_or_default()
		.as_secs()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundJobStatus {
	Pending,
	Running,
	Completed,
	Failed,
}

impl std::fmt::Display for BackgroundJobStatus {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			BackgroundJobStatus::Pending => write!(f, "pending"),
			BackgroundJobStatus::Running => write!(f, "running"),
			BackgroundJobStatus::Completed => write!(f, "completed"),
			BackgroundJobStatus::Failed => write!(f, "failed"),
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundJob {
	pub job_id: String,
	pub agent_name: String,
	/// Truncated task description stored for reference (first 500 chars)
	pub task_preview: String,
	pub status: BackgroundJobStatus,
	pub result: Option<String>,
	pub error: Option<String>,
	pub created_at: u64,
	pub updated_at: u64,
	/// Unix timestamp after which this job may be cleaned up
	pub expires_at: u64,
}

/// Thread-safe store for background jobs with TTL-based expiry.
#[derive(Clone)]
pub struct BackgroundJobManager {
	jobs: Arc<RwLock<HashMap<String, BackgroundJob>>>,
	ttl_seconds: u64,
}

impl BackgroundJobManager {
	pub fn new(ttl_seconds: u64) -> Self {
		Self {
			jobs: Arc::new(RwLock::new(HashMap::new())),
			ttl_seconds,
		}
	}

	/// Register a new job and return its ID. Status starts as Pending.
	pub fn submit_job(&self, agent_name: String, task: &str) -> String {
		let job_id = uuid::Uuid::new_v4().to_string();
		let now = now_secs();
		let task_preview = task.chars().take(500).collect();
		let job = BackgroundJob {
			job_id: job_id.clone(),
			agent_name,
			task_preview,
			status: BackgroundJobStatus::Pending,
			result: None,
			error: None,
			created_at: now,
			updated_at: now,
			expires_at: now + self.ttl_seconds,
		};
		self.jobs.write().unwrap().insert(job_id.clone(), job);
		job_id
	}

	/// Transition job to Running state.
	pub fn update_job_running(&self, job_id: &str) {
		let now = now_secs();
		if let Some(job) = self.jobs.write().unwrap().get_mut(job_id) {
			job.status = BackgroundJobStatus::Running;
			job.updated_at = now;
		}
	}

	/// Mark job as Completed with result and optional cost data.
	pub fn complete_job(&self, job_id: &str, result: String) {
		let now = now_secs();
		if let Some(job) = self.jobs.write().unwrap().get_mut(job_id) {
			job.status = BackgroundJobStatus::Completed;
			job.result = Some(result);
			job.updated_at = now;
			// Extend TTL from completion time so callers have time to retrieve results
			job.expires_at = now + self.ttl_seconds;
		}
	}

	/// Mark job as Failed with an error message.
	pub fn fail_job(&self, job_id: &str, error: String) {
		let now = now_secs();
		if let Some(job) = self.jobs.write().unwrap().get_mut(job_id) {
			job.status = BackgroundJobStatus::Failed;
			job.error = Some(error);
			job.updated_at = now;
			job.expires_at = now + self.ttl_seconds;
		}
	}

	/// Retrieve a snapshot of a job by ID.
	pub fn get_job(&self, job_id: &str) -> Option<BackgroundJob> {
		self.jobs.read().unwrap().get(job_id).cloned()
	}

	/// Count jobs currently in Running or Pending state.
	pub fn active_job_count(&self) -> usize {
		self.jobs
			.read()
			.unwrap()
			.values()
			.filter(|j| {
				j.status == BackgroundJobStatus::Pending || j.status == BackgroundJobStatus::Running
			})
			.count()
	}

	/// Paginated job listing sorted by (created_at DESC, job_id ASC) for stable ordering.
	///
	/// `cursor` is an opaque string encoding `"{created_at_secs}:{job_id}"` from the
	/// last item of the previous page. Pass `None` to start from the beginning.
	/// Returns `(page_items, next_cursor)` where `next_cursor` is `None` on the last page.
	pub fn list_jobs(
		&self,
		cursor: Option<&str>,
		limit: usize,
	) -> (Vec<BackgroundJob>, Option<String>) {
		let limit = limit.clamp(1, 100);

		// Parse cursor into (created_at, job_id) for comparison
		let cursor_key: Option<(u64, &str)> = cursor.and_then(|c| {
			let mut parts = c.splitn(2, ':');
			let ts: u64 = parts.next()?.parse().ok()?;
			let id = parts.next()?;
			Some((ts, id))
		});

		let guard = self.jobs.read().unwrap();

		// Collect and sort: newest first, then by job_id for tie-breaking
		let mut all: Vec<&BackgroundJob> = guard.values().collect();
		all.sort_by(|a, b| {
			b.created_at
				.cmp(&a.created_at)
				.then_with(|| a.job_id.cmp(&b.job_id))
		});

		// Apply cursor: skip everything up to and including the cursor position
		let start = if let Some((cur_ts, cur_id)) = cursor_key {
			all.iter()
				.position(|j| {
					j.created_at < cur_ts || (j.created_at == cur_ts && j.job_id.as_str() > cur_id)
				})
				.unwrap_or(all.len())
		} else {
			0
		};

		let page: Vec<BackgroundJob> = all
			.into_iter()
			.skip(start)
			.take(limit + 1) // fetch one extra to detect if there's a next page
			.cloned()
			.collect();

		if page.len() > limit {
			let items = page[..limit].to_vec();
			let last = &items[limit - 1];
			let next_cursor = format!("{}:{}", last.created_at, last.job_id);
			(items, Some(next_cursor))
		} else {
			(page, None)
		}
	}

	/// Remove all jobs whose `expires_at` is in the past. Returns count removed.
	pub fn cleanup_expired_jobs(&self) -> usize {
		let now = now_secs();
		let mut guard = self.jobs.write().unwrap();
		let before = guard.len();
		guard.retain(|_, job| job.expires_at > now);
		before - guard.len()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn make_manager() -> BackgroundJobManager {
		BackgroundJobManager::new(86400)
	}

	#[test]
	fn test_submit_and_get() {
		let mgr = make_manager();
		let id = mgr.submit_job("test_agent".to_string(), "do something");
		let job = mgr.get_job(&id).expect("job should exist");
		assert_eq!(job.status, BackgroundJobStatus::Pending);
		assert_eq!(job.agent_name, "test_agent");
		assert_eq!(job.task_preview, "do something");
	}

	#[test]
	fn test_lifecycle() {
		let mgr = make_manager();
		let id = mgr.submit_job("agent".to_string(), "task");

		mgr.update_job_running(&id);
		assert_eq!(
			mgr.get_job(&id).unwrap().status,
			BackgroundJobStatus::Running
		);

		mgr.complete_job(&id, "result text".to_string(), None);
		let job = mgr.get_job(&id).unwrap();
		assert_eq!(job.status, BackgroundJobStatus::Completed);
		assert_eq!(job.result.as_deref(), Some("result text"));
	}

	#[test]
	fn test_fail_job() {
		let mgr = make_manager();
		let id = mgr.submit_job("agent".to_string(), "task");
		mgr.fail_job(&id, "something went wrong".to_string());
		let job = mgr.get_job(&id).unwrap();
		assert_eq!(job.status, BackgroundJobStatus::Failed);
		assert_eq!(job.error.as_deref(), Some("something went wrong"));
	}

	#[test]
	fn test_active_count() {
		let mgr = make_manager();
		let id1 = mgr.submit_job("a".to_string(), "t1");
		let id2 = mgr.submit_job("a".to_string(), "t2");
		assert_eq!(mgr.active_job_count(), 2);
		mgr.complete_job(&id1, "done".to_string(), None);
		assert_eq!(mgr.active_job_count(), 1);
		mgr.fail_job(&id2, "err".to_string());
		assert_eq!(mgr.active_job_count(), 0);
	}

	#[test]
	fn test_cleanup_expired() {
		let mgr = BackgroundJobManager::new(0); // TTL = 0 → expires immediately
		mgr.submit_job("a".to_string(), "t");
		// expires_at = now + 0 = now, so after a tiny sleep it's expired
		std::thread::sleep(std::time::Duration::from_millis(10));
		let removed = mgr.cleanup_expired_jobs();
		assert_eq!(removed, 1);
		assert!(mgr.get_job("anything").is_none());
	}

	#[test]
	fn test_list_jobs_pagination() {
		let mgr = make_manager();
		for i in 0..5 {
			mgr.submit_job("a".to_string(), &format!("task {i}"));
		}
		let (page1, cursor) = mgr.list_jobs(None, 3);
		assert_eq!(page1.len(), 3);
		assert!(cursor.is_some());

		let (page2, cursor2) = mgr.list_jobs(cursor.as_deref(), 3);
		assert_eq!(page2.len(), 2);
		assert!(cursor2.is_none());

		// No overlap between pages
		let ids1: std::collections::HashSet<_> = page1.iter().map(|j| &j.job_id).collect();
		let ids2: std::collections::HashSet<_> = page2.iter().map(|j| &j.job_id).collect();
		assert!(ids1.is_disjoint(&ids2));
	}
}
