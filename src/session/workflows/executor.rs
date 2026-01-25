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

use crate::config::{Config, WorkflowStep, WorkflowStepType};
use crate::session::layers::types::GenericLayer;
use crate::session::layers::Layer;
use crate::session::Session;
use anyhow::Result;

use super::parser::PatternParser;

/// Workflow execution context
#[derive(Clone)]
pub struct WorkflowContext<'a> {
	pub step_index: usize,
	pub total_steps: usize,
	pub workflow_name: &'a str,
}

impl<'a> WorkflowContext<'a> {
	/// Create an owned version for use in spawned tasks
	pub fn to_owned(&self) -> OwnedWorkflowContext {
		OwnedWorkflowContext {
			step_index: self.step_index,
			total_steps: self.total_steps,
			workflow_name: self.workflow_name.to_string(),
		}
	}
}

/// Owned workflow execution context for spawned tasks
#[derive(Clone)]
pub struct OwnedWorkflowContext {
	pub step_index: usize,
	pub total_steps: usize,
	pub workflow_name: String,
}

impl OwnedWorkflowContext {
	/// Convert to borrowed context
	pub fn as_ref(&self) -> WorkflowContext<'_> {
		WorkflowContext {
			step_index: self.step_index,
			total_steps: self.total_steps,
			workflow_name: &self.workflow_name,
		}
	}
}

/// Workflow step execution result with timing
pub struct StepExecutionResult {
	pub output: String,
	pub step_name: String,
	pub step_index: usize,
	pub total_steps: usize,
	pub duration_ms: u64,
}

/// Executes individual workflow steps
pub struct StepExecutor;

impl StepExecutor {
	/// Execute a single workflow step and return result with timing
	pub fn execute_step<'a>(
		step: &'a WorkflowStep,
		input: &'a str,
		session: &'a Session,
		config: &'a Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: WorkflowContext<'a>,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<StepExecutionResult>> + Send + 'a>>
	{
		Box::pin(async move {
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			crate::log_debug!(
				"Executing workflow step: {} (type: {:?})",
				step.name,
				step.step_type
			);

			let step_start = std::time::Instant::now();
			let result = match step.step_type {
				WorkflowStepType::Once => {
					Self::execute_once(step, input, session, config, operation_cancelled, &context)
						.await?
				}
				WorkflowStepType::Loop => {
					Self::execute_loop(step, input, session, config, operation_cancelled, &context)
						.await?
				}
				WorkflowStepType::Foreach => {
					Self::execute_foreach(
						step,
						input,
						session,
						config,
						operation_cancelled,
						&context,
					)
					.await?
				}
				WorkflowStepType::Conditional => {
					Self::execute_conditional(
						step,
						input,
						session,
						config,
						operation_cancelled,
						&context,
					)
					.await?
				}
				WorkflowStepType::Parallel => {
					Self::execute_parallel(
						step,
						input,
						session,
						config,
						operation_cancelled,
						&context,
					)
					.await?
				}
			};

			let duration_ms = step_start.elapsed().as_millis() as u64;

			Ok(StepExecutionResult {
				output: result,
				step_name: step.name.clone(),
				step_index: context.step_index,
				total_steps: context.total_steps,
				duration_ms,
			})
		})
	}

	/// Execute a layer once
	async fn execute_once(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: &WorkflowContext<'_>,
	) -> Result<String> {
		let layer_name = step
			.layer
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Once step requires layer name"))?;

		Self::execute_layer(
			layer_name,
			input,
			session,
			config,
			operation_cancelled,
			context,
		)
		.await
	}

	/// Execute a loop until exit condition
	async fn execute_loop(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: &WorkflowContext<'_>,
	) -> Result<String> {
		let exit_pattern = step
			.exit_pattern
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Loop step requires exit_pattern"))?;

		let max_iterations = step.max_iterations.unwrap_or(10);
		let mut current_input = input.to_string();

		for iteration in 0..max_iterations {
			crate::log_debug!("Loop iteration {}/{}", iteration + 1, max_iterations);

			// Execute substeps
			for substep in &step.substeps {
				let substep_result = Self::execute_step(
					substep,
					&current_input,
					session,
					config,
					operation_cancelled.clone(),
					context.clone(),
				)
				.await?;
				current_input = substep_result.output;

				if *operation_cancelled.borrow() {
					return Err(anyhow::anyhow!("Operation cancelled"));
				}
			}

			// Check exit condition
			if PatternParser::matches(&current_input, exit_pattern)? {
				break;
			}

			if iteration == max_iterations - 1 {
				crate::log_info!("Loop max iterations reached for step: {}", step.name);
			}
		}

		Ok(current_input)
	}

	/// Execute foreach over parsed items
	async fn execute_foreach(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: &WorkflowContext<'_>,
	) -> Result<String> {
		let pattern = step
			.parse_pattern
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Foreach step requires parse_pattern"))?;

		// Parse items from input
		let items = PatternParser::parse_items(input, pattern)?;
		let total_items = items.len();

		let mut results = Vec::new();

		for (i, item) in items.iter().enumerate() {
			crate::log_debug!("Processing item {}/{}: {}", i + 1, total_items, item);

			let mut current_input = item.clone();

			// Execute substeps for this item
			for substep in &step.substeps {
				let substep_result = Self::execute_step(
					substep,
					&current_input,
					session,
					config,
					operation_cancelled.clone(),
					context.clone(),
				)
				.await?;
				current_input = substep_result.output;

				if *operation_cancelled.borrow() {
					return Err(anyhow::anyhow!("Operation cancelled"));
				}
			}

			results.push(current_input);
		}

		// Combine results
		Ok(results.join("\n\n"))
	}

	/// Execute conditional branching
	async fn execute_conditional(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: &WorkflowContext<'_>,
	) -> Result<String> {
		let layer_name = step
			.layer
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Conditional step requires layer"))?;

		let condition_pattern = step
			.condition_pattern
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Conditional step requires condition_pattern"))?;

		// Execute the condition layer
		let output = Self::execute_layer(
			layer_name,
			input,
			session,
			config,
			operation_cancelled.clone(),
			context,
		)
		.await?;

		// Check pattern match
		let matches = PatternParser::matches(&output, condition_pattern)?;

		if matches {
			crate::log_debug!("Condition matched for step: {}", step.name);
		} else {
			crate::log_debug!("Condition not matched for step: {}", step.name);
		}

		let layers_to_execute = if matches {
			&step.on_match
		} else {
			&step.on_no_match
		};

		// Execute selected branch
		let mut current_input = output;
		for layer_name in layers_to_execute {
			current_input = Self::execute_layer(
				layer_name,
				&current_input,
				session,
				config,
				operation_cancelled.clone(),
				context,
			)
			.await?;
		}

		Ok(current_input)
	}

	/// Execute layers in parallel
	async fn execute_parallel(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: &WorkflowContext<'_>,
	) -> Result<String> {
		// Execute all layers in parallel
		let mut futures = Vec::new();
		for layer_name in &step.parallel_layers {
			let layer_name = layer_name.clone();
			let input = input.to_string();
			let session = session.clone();
			let config = config.clone();
			let operation_cancelled = operation_cancelled.clone();
			let ctx = context.to_owned();

			futures.push(tokio::spawn(async move {
				Self::execute_layer(
					&layer_name,
					&input,
					&session,
					&config,
					operation_cancelled,
					&ctx.as_ref(),
				)
				.await
			}));
		}

		// Wait for all to complete
		let results = futures::future::join_all(futures).await;

		// Collect results
		let mut outputs = Vec::new();
		for result in results {
			let output = result??;
			outputs.push(output);
		}

		// If aggregator specified, use it to combine results
		if let Some(aggregator) = &step.aggregator {
			let combined_input = outputs.join("\n\n---\n\n");
			Self::execute_layer(
				aggregator,
				&combined_input,
				session,
				config,
				operation_cancelled,
				context,
			)
			.await
		} else {
			// Otherwise just concatenate
			Ok(outputs.join("\n\n"))
		}
	}

	/// Execute a single layer by name
	async fn execute_layer(
		layer_name: &str,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
		context: &WorkflowContext<'_>,
	) -> Result<String> {
		use colored::Colorize;

		// Get layer config from global registry
		let layer_config = if let Some(layers) = &config.layers {
			layers
				.iter()
				.find(|l| l.name == layer_name)
				.ok_or_else(|| anyhow::anyhow!("Layer '{}' not found in config", layer_name))?
				.clone()
		} else {
			return Err(anyhow::anyhow!(
				"No layers defined in config, cannot execute layer '{}'",
				layer_name
			));
		};

		// Create GenericLayer instance
		let mut layer = GenericLayer::new(layer_config);

		// Set workflow context for display
		layer.set_workflow_context(
			context.step_index,
			context.total_steps,
			context.workflow_name.to_string(),
		);

		// Execute layer
		let result = layer
			.process(input, session, config, operation_cancelled)
			.await?;

		// Check if layer had tool calls
		let had_tool_calls =
			result.tool_calls.is_some() && !result.tool_calls.as_ref().unwrap().is_empty();

		// Display layer output (same as /run command does)
		if let Some(output) = result.outputs.last() {
			if !output.trim().is_empty() {
				// Display step header for non-tool responses (tool responses already have headers)
				if !had_tool_calls {
					let response_header = format!(
						" {} | {} | Step {}/{} ",
						context.workflow_name.bright_yellow(),
						layer_name.bright_cyan(),
						context.step_index,
						context.total_steps
					);
					let separator_length = 70.max(response_header.len() + 4);
					let dashes = "─".repeat(separator_length - response_header.len());
					let separator = format!("──{}{}──", response_header, dashes.dimmed());
					println!("{}", separator);
				}

				println!();
				use crate::session::chat::assistant_output::print_assistant_response;
				print_assistant_response(output, config, "", &None);
				println!();
			}
		}

		// Return last output
		Ok(result.outputs.last().unwrap_or(&String::new()).clone())
	}
}
