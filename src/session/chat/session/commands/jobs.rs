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

// /jobs command handler — display background agent jobs and their status

use super::{CommandOutput, CommandResult};
use anyhow::Result;

pub fn handle_jobs(params: &[&str]) -> Result<CommandResult> {
	// Optional cursor for pagination passed as first param
	let cursor = params.first().copied().filter(|s| !s.is_empty());

	// Access the global job manager if it has been initialized
	let manager = match crate::mcp::agent::functions::try_get_job_manager() {
		Some(m) => m,
		None => {
			return Ok(CommandResult::HandledWithOutput(CommandOutput::Jobs {
				jobs: vec![],
				next_cursor: None,
				total_shown: 0,
			}));
		}
	};

	let (jobs, next_cursor) = manager.list_jobs(cursor, 20);
	let total_shown = jobs.len();
	let jobs_json: Vec<serde_json::Value> = jobs
		.into_iter()
		.map(|j| {
			serde_json::json!({
				"job_id": j.job_id,
				"agent_name": j.agent_name,
				"status": j.status,
				"task_preview": j.task_preview,
				"created_at": j.created_at,
				"updated_at": j.updated_at,
			})
		})
		.collect();

	Ok(CommandResult::HandledWithOutput(CommandOutput::Jobs {
		jobs: jobs_json,
		next_cursor,
		total_shown,
	}))
}
