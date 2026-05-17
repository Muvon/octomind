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

use crate::config::{Config, WorkflowDefinition};
use crate::session::Session;
use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::executor::{StepExecutor, WorkflowContext};

/// Workflow execution progress tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowProgress {
	pub step_outputs: Vec<StepOutput>,
	pub duration_secs: f64,
}

/// Individual step output with timing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepOutput {
	pub step_name: String,
	pub step_index: usize,
	pub total_steps: usize,
	pub content: String,
	pub duration_ms: u64,
}

/// Orchestrates workflow execution
pub struct WorkflowOrchestrator {
	workflow: WorkflowDefinition,
	workflow_name: String,
}

impl WorkflowOrchestrator {
	/// Create new workflow orchestrator
	pub fn new(workflow: WorkflowDefinition, workflow_name: String) -> Self {
		Self {
			workflow,
			workflow_name,
		}
	}

	/// Execute the complete workflow
	pub async fn execute(
		&self,
		input: &str,
		session: &mut Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<(String, WorkflowProgress)> {
		let workflow_start = std::time::Instant::now();
		let mut current_input = input.to_string();
		let mut step_outputs = Vec::new();
		let total_steps = self.workflow.steps.len();

		// Execute each top-level step
		for (i, step) in self.workflow.steps.iter().enumerate() {
			// Execute the step and get result with timing
			let step_result = StepExecutor::execute_step(
				step,
				&current_input,
				session,
				config,
				operation_cancelled.clone(),
				WorkflowContext {
					step_index: i + 1,
					total_steps,
					workflow_name: &self.workflow_name,
				},
			)
			.await?;

			if *operation_cancelled.borrow() {
				return Err(anyhow::Error::new(crate::session::cancellation::Cancelled));
			}

			// Update current input with step output
			current_input = step_result.output;

			// Capture step output for progress tracking
			step_outputs.push(StepOutput {
				step_name: step_result.step_name,
				step_index: i + 1,
				total_steps,
				content: current_input.clone(),
				duration_ms: step_result.duration_ms,
			});
		}
		let duration_secs = workflow_start.elapsed().as_secs_f64();

		let progress = WorkflowProgress {
			step_outputs,
			duration_secs,
		};

		Ok((current_input, progress))
	}
}
