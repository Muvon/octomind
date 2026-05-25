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

use crate::config::{PipelineStep, PipelineStepType};
use anyhow::{anyhow, Result};
use regex::Regex;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::watch;

/// True iff `pattern` matches anywhere in `text`. Used by loop exit / conditional branching.
fn pattern_matches(text: &str, pattern: &str) -> Result<bool> {
	Ok(Regex::new(pattern)?.is_match(text))
}

/// Extract items from `text` using `pattern`. Returns the first capture group of
/// each match, falling back to the full match when no groups are present.
/// Used by foreach to split a step's output into per-item invocations.
fn pattern_parse_items(text: &str, pattern: &str) -> Result<Vec<String>> {
	let regex = Regex::new(pattern)?;
	let mut items = Vec::new();
	for cap in regex.captures_iter(text) {
		if let Some(matched) = cap.get(1).or_else(|| cap.get(0)) {
			items.push(matched.as_str().to_string());
		}
	}
	Ok(items)
}

/// Context passed to each pipeline step during execution
pub struct PipelineContext {
	pub pipeline_name: String,
	pub step_name: String,
	pub step_index: usize,
	pub total_steps: usize,
	pub role: String,
	pub working_dir: PathBuf,
}

pub struct PipelineStepExecutor;

impl PipelineStepExecutor {
	/// Execute a pipeline step based on its type
	/// Uses Box::pin for recursive async calls (substeps can call execute_step)
	pub fn execute_step<'a>(
		step: &'a PipelineStep,
		input: &'a str,
		context: &'a PipelineContext,
		operation_cancelled: watch::Receiver<bool>,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
		Box::pin(async move {
			match step.step_type {
				PipelineStepType::Once => Self::execute_once(step, input, context).await,
				PipelineStepType::Loop => {
					Self::execute_loop(step, input, context, operation_cancelled).await
				}
				PipelineStepType::Foreach => {
					Self::execute_foreach(step, input, context, operation_cancelled).await
				}
				PipelineStepType::Conditional => {
					Self::execute_conditional(step, input, context, operation_cancelled).await
				}
			}
		})
	}

	/// Execute a single command, piping stdin and capturing stdout
	/// Exit code 0 = success (return stdout), non-zero = fatal error
	async fn execute_command(
		command: &str,
		input: &str,
		timeout_secs: u64,
		context: &PipelineContext,
	) -> Result<String> {
		let mut cmd = Command::new(command);
		cmd.stdin(std::process::Stdio::piped())
			.stdout(std::process::Stdio::piped())
			.stderr(std::process::Stdio::piped())
			.current_dir(&context.working_dir)
			.env("PIPELINE_NAME", &context.pipeline_name)
			.env("PIPELINE_STEP", &context.step_name)
			.env("PIPELINE_STEP_INDEX", context.step_index.to_string())
			.env("PIPELINE_TOTAL_STEPS", context.total_steps.to_string())
			.env("OCTOMIND_ROLE", &context.role)
			.env(
				"OCTOMIND_WORKING_DIR",
				context.working_dir.to_string_lossy().as_ref(),
			);

		let mut child = cmd
			.spawn()
			.map_err(|e| anyhow!("Failed to spawn '{}': {}", command, e))?;

		// Write stdin
		if let Some(mut stdin) = child.stdin.take() {
			let _ = stdin.write_all(input.as_bytes()).await;
			drop(stdin);
		}

		// Wait with timeout
		let result =
			tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

		match result {
			Ok(Ok(output)) => {
				if output.status.success() {
					Ok(String::from_utf8_lossy(&output.stdout).to_string())
				} else {
					let stderr = String::from_utf8_lossy(&output.stderr);
					let code = output.status.code().unwrap_or(-1);
					Err(anyhow!(
						"Pipeline step '{}' command '{}' failed (exit {}): {}",
						context.step_name,
						command,
						code,
						stderr.trim()
					))
				}
			}
			Ok(Err(e)) => Err(anyhow!(
				"Pipeline step '{}' command '{}' IO error: {}",
				context.step_name,
				command,
				e
			)),
			Err(_) => Err(anyhow!(
				"Pipeline step '{}' command '{}' timed out after {}s",
				context.step_name,
				command,
				timeout_secs
			)),
		}
	}

	/// Once: execute command, pipe stdin → stdout
	async fn execute_once(
		step: &PipelineStep,
		input: &str,
		context: &PipelineContext,
	) -> Result<String> {
		let command = step
			.command
			.as_ref()
			.ok_or_else(|| anyhow!("Step '{}': missing command", step.name))?;
		Self::execute_command(command, input, step.timeout, context).await
	}

	/// Loop: repeat substeps until exit_pattern matches in stdout or max_iterations
	async fn execute_loop(
		step: &PipelineStep,
		input: &str,
		context: &PipelineContext,
		operation_cancelled: watch::Receiver<bool>,
	) -> Result<String> {
		let max_iterations = step.max_iterations.unwrap_or(10);
		let exit_pattern = step
			.exit_pattern
			.as_ref()
			.ok_or_else(|| anyhow!("Step '{}': missing exit_pattern", step.name))?;

		let mut current_input = input.to_string();

		for _iteration in 0..max_iterations {
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Pipeline cancelled"));
			}

			// Execute all substeps in sequence
			for substep in &step.substeps {
				let sub_context = PipelineContext {
					step_name: substep.name.clone(),
					..PipelineContext {
						pipeline_name: context.pipeline_name.clone(),
						step_name: substep.name.clone(),
						step_index: context.step_index,
						total_steps: context.total_steps,
						role: context.role.clone(),
						working_dir: context.working_dir.clone(),
					}
				};

				current_input = Self::execute_step(
					substep,
					&current_input,
					&sub_context,
					operation_cancelled.clone(),
				)
				.await?;
			}

			if pattern_matches(&current_input, exit_pattern)? {
				break;
			}
		}

		Ok(current_input)
	}

	/// Foreach: parse items from input, run substeps for each
	async fn execute_foreach(
		step: &PipelineStep,
		input: &str,
		context: &PipelineContext,
		operation_cancelled: watch::Receiver<bool>,
	) -> Result<String> {
		let parse_pattern = step
			.parse_pattern
			.as_ref()
			.ok_or_else(|| anyhow!("Step '{}': missing parse_pattern", step.name))?;
		let items = pattern_parse_items(input, parse_pattern)?;
		let mut results = Vec::new();

		for item in items {
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Pipeline cancelled"));
			}

			let mut current_input = item;

			for substep in &step.substeps {
				let sub_context = PipelineContext {
					pipeline_name: context.pipeline_name.clone(),
					step_name: substep.name.clone(),
					step_index: context.step_index,
					total_steps: context.total_steps,
					role: context.role.clone(),
					working_dir: context.working_dir.clone(),
				};

				current_input = Self::execute_step(
					substep,
					&current_input,
					&sub_context,
					operation_cancelled.clone(),
				)
				.await?;
			}

			results.push(current_input);
		}

		Ok(results.join("\n\n"))
	}

	/// Conditional: run command, check stdout pattern, branch to on_match or on_no_match
	/// Non-zero exit code from the condition command is a fatal error.
	async fn execute_conditional(
		step: &PipelineStep,
		input: &str,
		context: &PipelineContext,
		operation_cancelled: watch::Receiver<bool>,
	) -> Result<String> {
		let command = step
			.command
			.as_ref()
			.ok_or_else(|| anyhow!("Step '{}': missing command", step.name))?;
		let condition_pattern = step
			.condition_pattern
			.as_ref()
			.ok_or_else(|| anyhow!("Step '{}': missing condition_pattern", step.name))?;

		// Run the condition command — non-zero exit = fatal
		let output = Self::execute_command(command, input, step.timeout, context).await?;

		let matches = pattern_matches(&output, condition_pattern)?;

		let commands_to_run = if matches {
			&step.on_match
		} else {
			&step.on_no_match
		};

		// Execute selected branch commands sequentially, piping between them
		let mut current_input = output;
		for cmd in commands_to_run {
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Pipeline cancelled"));
			}

			let branch_context = PipelineContext {
				pipeline_name: context.pipeline_name.clone(),
				step_name: format!("{}:{}", step.name, cmd),
				step_index: context.step_index,
				total_steps: context.total_steps,
				role: context.role.clone(),
				working_dir: context.working_dir.clone(),
			};

			current_input =
				Self::execute_command(cmd, &current_input, step.timeout, &branch_context).await?;
		}

		Ok(current_input)
	}
}
