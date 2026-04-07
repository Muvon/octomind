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

use crate::config::PipelineDefinition;
use anyhow::Result;
use colored::Colorize;
use std::path::Path;
use tokio::sync::watch;

use super::executor::{PipelineContext, PipelineStepExecutor};

/// Orchestrates pipeline execution — runs steps sequentially, piping stdout between them
pub struct PipelineOrchestrator {
	pipeline: PipelineDefinition,
	pipeline_name: String,
}

impl PipelineOrchestrator {
	pub fn new(pipeline: PipelineDefinition, pipeline_name: String) -> Self {
		Self {
			pipeline,
			pipeline_name,
		}
	}

	/// Execute the complete pipeline
	/// Input flows through each step: stdin → step1 → stdout/stdin → step2 → ... → final stdout
	pub async fn execute(
		&self,
		input: &str,
		working_dir: &Path,
		role: &str,
		operation_cancelled: watch::Receiver<bool>,
	) -> Result<String> {
		let mut current_input = input.to_string();
		let total_steps = self.pipeline.steps.len();

		for (i, step) in self.pipeline.steps.iter().enumerate() {
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Pipeline cancelled"));
			}

			let step_index = i + 1;
			let context = PipelineContext {
				pipeline_name: self.pipeline_name.clone(),
				step_name: step.name.clone(),
				step_index,
				total_steps,
				role: role.to_string(),
				working_dir: working_dir.to_path_buf(),
			};

			let step_start = std::time::Instant::now();

			current_input = PipelineStepExecutor::execute_step(
				step,
				&current_input,
				&context,
				operation_cancelled.clone(),
			)
			.await?;

			let duration_ms = step_start.elapsed().as_millis();

			// Render step result (same style as workflow step rendering in executor.rs)
			let trimmed = current_input.trim();
			if !trimmed.is_empty() {
				let response_header = format!(
					" {} | {} | Step {}/{} | {}ms ",
					self.pipeline_name.bright_yellow(),
					step.name.bright_cyan(),
					step_index,
					total_steps,
					duration_ms,
				);
				let separator_length = 70.max(response_header.len() + 4);
				let dashes = "─".repeat(separator_length - response_header.len());
				let separator = format!("──{}{}──", response_header, dashes.dimmed());
				println!("{}", separator);
				println!();
				println!("{}", trimmed.bright_green());
				println!();
			}
		}

		Ok(current_input)
	}
}
