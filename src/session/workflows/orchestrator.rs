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

use crate::config::{Config, WorkflowDefinition};
use crate::session::Session;
use anyhow::Result;
use colored::*;

use super::executor::StepExecutor;

/// Orchestrates workflow execution
pub struct WorkflowOrchestrator {
	workflow: WorkflowDefinition,
}

impl WorkflowOrchestrator {
	/// Create new workflow orchestrator
	pub fn new(workflow: WorkflowDefinition) -> Self {
		Self { workflow }
	}

	/// Execute the complete workflow
	pub async fn execute(
		&self,
		input: &str,
		session: &mut Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<String> {
		println!("\n{}", "═══ Workflow ═══".bright_cyan().bold());
		println!("{}", self.workflow.description.bright_white());
		println!();

		let start = std::time::Instant::now();
		let mut current_input = input.to_string();

		// Execute each top-level step
		for (i, step) in self.workflow.steps.iter().enumerate() {
			println!(
				"{} Step {}/{}: {}",
				"▶".bright_green(),
				i + 1,
				self.workflow.steps.len(),
				step.name.bright_white()
			);

			current_input = StepExecutor::execute_step(
				step,
				&current_input,
				session,
				config,
				operation_cancelled.clone(),
			)
			.await?;

			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			println!();
		}

		println!(
			"{} Workflow completed in {:.2}s",
			"✓".bright_green(),
			start.elapsed().as_secs_f64()
		);

		Ok(current_input)
	}
}
