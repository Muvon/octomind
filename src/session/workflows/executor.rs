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
use colored::*;

use super::parser::PatternParser;

/// Executes individual workflow steps
pub struct StepExecutor;

impl StepExecutor {
	/// Execute a single workflow step
	pub fn execute_step<'a>(
		step: &'a WorkflowStep,
		input: &'a str,
		session: &'a Session,
		config: &'a Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
		Box::pin(async move {
			if *operation_cancelled.borrow() {
				return Err(anyhow::anyhow!("Operation cancelled"));
			}

			crate::log_debug!(
				"Executing workflow step: {} (type: {:?})",
				step.name,
				step.step_type
			);

			match step.step_type {
				WorkflowStepType::Once => {
					Self::execute_once(step, input, session, config, operation_cancelled).await
				}
				WorkflowStepType::Loop => {
					Self::execute_loop(step, input, session, config, operation_cancelled).await
				}
				WorkflowStepType::Foreach => {
					Self::execute_foreach(step, input, session, config, operation_cancelled).await
				}
				WorkflowStepType::Conditional => {
					Self::execute_conditional(step, input, session, config, operation_cancelled)
						.await
				}
				WorkflowStepType::Parallel => {
					Self::execute_parallel(step, input, session, config, operation_cancelled).await
				}
			}
		})
	}

	/// Execute a layer once
	async fn execute_once(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<String> {
		let layer_name = step
			.layer
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Once step requires layer name"))?;

		println!("{} {}", "→".bright_yellow(), step.name.bright_white());

		Self::execute_layer(layer_name, input, session, config, operation_cancelled).await
	}

	/// Execute a loop until exit condition
	async fn execute_loop(
		step: &WorkflowStep,
		input: &str,
		session: &Session,
		config: &Config,
		operation_cancelled: tokio::sync::watch::Receiver<bool>,
	) -> Result<String> {
		let exit_pattern = step
			.exit_pattern
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Loop step requires exit_pattern"))?;

		let max_iterations = step.max_iterations.unwrap_or(10);
		let mut current_input = input.to_string();

		println!(
			"{} {} (max: {})",
			"⟳".bright_cyan(),
			step.name.bright_white(),
			max_iterations
		);

		for iteration in 0..max_iterations {
			crate::log_debug!("Loop iteration {}/{}", iteration + 1, max_iterations);

			// Execute substeps
			for substep in &step.substeps {
				current_input = Self::execute_step(
					substep,
					&current_input,
					session,
					config,
					operation_cancelled.clone(),
				)
				.await?;

				if *operation_cancelled.borrow() {
					return Err(anyhow::anyhow!("Operation cancelled"));
				}
			}

			// Check exit condition
			if PatternParser::matches(&current_input, exit_pattern)? {
				println!(
					"{} Loop complete (iteration {})",
					"✓".bright_green(),
					iteration + 1
				);
				break;
			}

			if iteration == max_iterations - 1 {
				println!("{} Loop max iterations reached", "⚠".bright_yellow());
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
	) -> Result<String> {
		let pattern = step
			.parse_pattern
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Foreach step requires parse_pattern"))?;

		// Parse items from input
		let items = PatternParser::parse_items(input, pattern)?;

		println!(
			"{} {} ({} items)",
			"⇉".bright_magenta(),
			step.name.bright_white(),
			items.len()
		);

		let mut results = Vec::new();

		for (i, item) in items.iter().enumerate() {
			crate::log_debug!("Processing item {}/{}: {}", i + 1, items.len(), item);

			println!("  {} Item {}/{}", "→".bright_blue(), i + 1, items.len());

			let mut current_input = item.clone();

			// Execute substeps for this item
			for substep in &step.substeps {
				current_input = Self::execute_step(
					substep,
					&current_input,
					session,
					config,
					operation_cancelled.clone(),
				)
				.await?;

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
	) -> Result<String> {
		let layer_name = step
			.layer
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Conditional step requires layer"))?;

		let condition_pattern = step
			.condition_pattern
			.as_ref()
			.ok_or_else(|| anyhow::anyhow!("Conditional step requires condition_pattern"))?;

		println!("{} {}", "⎇".bright_cyan(), step.name.bright_white());

		// Execute the condition layer
		let output = Self::execute_layer(
			layer_name,
			input,
			session,
			config,
			operation_cancelled.clone(),
		)
		.await?;

		// Check pattern match
		let matches = PatternParser::matches(&output, condition_pattern)?;

		let layers_to_execute = if matches {
			println!("  {} Condition matched", "✓".bright_green());
			&step.on_match
		} else {
			println!("  {} Condition not matched", "✗".bright_red());
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
	) -> Result<String> {
		println!(
			"{} {} ({} layers)",
			"⫴".bright_magenta(),
			step.name.bright_white(),
			step.parallel_layers.len()
		);

		// Execute all layers in parallel
		let mut futures = Vec::new();
		for layer_name in &step.parallel_layers {
			let layer_name = layer_name.clone();
			let input = input.to_string();
			let session = session.clone();
			let config = config.clone();
			let operation_cancelled = operation_cancelled.clone();

			futures.push(tokio::spawn(async move {
				Self::execute_layer(&layer_name, &input, &session, &config, operation_cancelled)
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
	) -> Result<String> {
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
		let layer = GenericLayer::new(layer_config);

		// Execute layer
		let result = layer
			.process(input, session, config, operation_cancelled)
			.await?;

		// Return last output
		Ok(result.outputs.last().unwrap_or(&String::new()).clone())
	}
}
