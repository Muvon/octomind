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

use anyhow::{anyhow, Result};

use super::Config;

impl Config {
	/// Validate the configuration for common issues - STRICT MODE
	/// All validation errors are now fatal in strict mode
	pub fn validate(&self) -> Result<()> {
		// Validate threshold values - STRICT
		self.validate_thresholds()?;

		// Validate MCP configuration - STRICT
		self.validate_mcp_config()?;

		// Validate layer configuration if present - STRICT
		if let Some(layers) = &self.layers {
			self.validate_layers(layers)?;
		}

		// Validate workflows - STRICT
		self.validate_workflows()?;

		// Validate pipelines - STRICT
		self.validate_pipelines()?;

		// Validate webhook hooks - STRICT
		self.validate_hooks()?;

		// STRICT: Validate required fields are not empty
		self.validate_required_fields()?;

		Ok(())
	}

	/// Validate webhook hook configurations
	fn validate_hooks(&self) -> Result<()> {
		let mut seen_names = std::collections::HashSet::new();
		let mut seen_binds = std::collections::HashSet::new();

		for hook in &self.hooks {
			if hook.name.is_empty() {
				return Err(anyhow!("Hook has empty name"));
			}
			if !seen_names.insert(&hook.name) {
				return Err(anyhow!("Duplicate hook name: '{}'", hook.name));
			}
			if hook.bind.is_empty() {
				return Err(anyhow!("Hook '{}' has empty bind address", hook.name));
			}
			if !seen_binds.insert(&hook.bind) {
				return Err(anyhow!(
					"Hook '{}' has duplicate bind address '{}' (already used by another hook)",
					hook.name,
					hook.bind
				));
			}
			if hook.bind.parse::<std::net::SocketAddr>().is_err() {
				return Err(anyhow!(
					"Hook '{}' has invalid bind address: '{}'",
					hook.name,
					hook.bind
				));
			}
			if hook.script.is_empty() {
				return Err(anyhow!("Hook '{}' has empty script path", hook.name));
			}
			if hook.timeout == 0 {
				return Err(anyhow!("Hook '{}' timeout must be > 0", hook.name));
			}
			if hook.timeout > 3600 {
				return Err(anyhow!(
					"Hook '{}' timeout too high: {}s (max 3600)",
					hook.name,
					hook.timeout
				));
			}
		}
		Ok(())
	}

	/// Validate that all required fields are present and not empty
	fn validate_required_fields(&self) -> Result<()> {
		if self.model.is_empty() {
			return Err(anyhow!("Model field cannot be empty"));
		}

		if self.markdown_theme.is_empty() {
			return Err(anyhow!("Markdown theme field cannot be empty"));
		}

		// Validate role configurations
		for role in &self.roles {
			// Validate temperature
			if role.config.temperature < 0.0 || role.config.temperature > 2.0 {
				return Err(anyhow!(
					"Role '{}' temperature must be between 0.0 and 2.0, got: {}",
					role.name,
					role.config.temperature
				));
			}

			// Validate top_p
			if role.config.top_p < 0.0 || role.config.top_p > 1.0 {
				return Err(anyhow!(
					"Role '{}' top_p must be between 0.0 and 1.0, got: {}",
					role.name,
					role.config.top_p
				));
			}

			// Validate top_k
			if role.config.top_k < 1 || role.config.top_k > 1000 {
				return Err(anyhow!(
					"Role '{}' top_k must be between 1 and 1000, got: {}",
					role.name,
					role.config.top_k
				));
			}
		}

		Ok(())
	}

	pub fn validate_thresholds(&self) -> Result<()> {
		// Validate max session tokens threshold (0 = disabled, >0 = enabled)
		if self.max_session_tokens_threshold > 2_000_000 {
			return Err(anyhow!(
				"Max session tokens threshold too high: {}. Maximum allowed: 2,000,000",
				self.max_session_tokens_threshold
			));
		}

		// Validate cache keepalive max idle (0 = unbounded, otherwise cap at 24h
		// so a typo can't burn through credit on an abandoned session).
		if self.cache_keepalive_max_idle_seconds > 86400 {
			return Err(anyhow!(
				"Cache keepalive max idle too high: {} seconds. Maximum allowed: 86400 (24 hours), or 0 for unbounded",
				self.cache_keepalive_max_idle_seconds
			));
		}

		Ok(())
	}

	fn validate_mcp_config(&self) -> Result<()> {
		// Validate server configurations
		for server_config in &self.mcp.servers {
			let server_name = &server_config.name();
			// Validate timeout
			if server_config.timeout_seconds() == 0 {
				return Err(anyhow!(
					"Server '{}' has invalid timeout: 0. Must be greater than 0",
					server_name
				));
			}

			if server_config.timeout_seconds() > 3600 {
				// 1 hour max
				return Err(anyhow!(
					"Server '{}' timeout too high: {} seconds. Maximum allowed: 3600 (1 hour)",
					server_name,
					server_config.timeout_seconds()
				));
			}

			// Validate external server configuration
			if matches!(
				server_config.connection_type(),
				crate::config::McpConnectionType::Http
			) {
				if server_config.url().is_none() && server_config.command().is_none() {
					return Err(anyhow!(
						"External server '{}' must have either 'url' or 'command' specified",
						server_name
					));
				}

				if server_config.url().is_some() && server_config.command().is_some() {
					return Err(anyhow!(
						"External server '{}' cannot have both 'url' and 'command' specified",
						server_name
					));
				}
			}
		}

		Ok(())
	}

	fn validate_layers(&self, layers: &[crate::session::layers::LayerConfig]) -> Result<()> {
		for (index, layer) in layers.iter().enumerate() {
			// Validate layer name
			if layer.name.is_empty() {
				return Err(anyhow!("Layer at index {} has empty name", index));
			}

			// Validate layer description
			if layer.description.is_empty() {
				return Err(anyhow!(
					"Layer '{}' at index {} has empty description",
					layer.name,
					index
				));
			}

			// Validate layer command (required for ACP execution)
			if layer.command.is_empty() {
				return Err(anyhow!(
					"Layer '{}' at index {} has empty command. Layers now execute via ACP protocol — add a 'command' field (e.g., command = 'octomind acp <role>')",
					layer.name,
					index
				));
			}

			// Additional layer-specific validation can be added here
		}

		Ok(())
	}

	/// Validate pipelines configuration
	fn validate_pipelines(&self) -> Result<()> {
		for pipeline in &self.pipelines {
			pipeline
				.validate()
				.map_err(|e| anyhow!("Pipeline validation failed: {}", e))?;
		}

		// Validate role pipeline references
		for role in &self.roles {
			if let Some(pipeline_name) = &role.pipeline {
				if !self.pipelines.iter().any(|p| &p.name == pipeline_name) {
					return Err(anyhow!(
						"Role '{}' references undefined pipeline '{}'",
						role.name,
						pipeline_name
					));
				}
			}
		}

		Ok(())
	}

	/// Validate workflows configuration
	fn validate_workflows(&self) -> Result<()> {
		// Validate each workflow definition
		for workflow in &self.workflows {
			workflow
				.validate()
				.map_err(|e| anyhow!("Workflow validation failed: {}", e))?;
		}

		// Validate role workflow references
		for role in &self.roles {
			if let Some(workflow_name) = &role.workflow {
				if !self.workflows.iter().any(|w| &w.name == workflow_name) {
					return Err(anyhow!(
						"Role '{}' references undefined workflow '{}'",
						role.name,
						workflow_name
					));
				}
			}
		}

		// Validate that all layer references in workflows exist in config.layers
		if let Some(layers) = &self.layers {
			use std::collections::HashSet;
			let layer_names: HashSet<&str> = layers.iter().map(|l| l.name.as_str()).collect();

			for workflow in &self.workflows {
				// Recursive function to validate all steps including substeps
				fn validate_step_layers(
					step: &crate::config::WorkflowStep,
					layer_names: &HashSet<&str>,
					workflow_name: &str,
				) -> Result<(), anyhow::Error> {
					// Check step.layer
					if let Some(layer) = &step.layer {
						if !layer_names.contains(layer.as_str()) {
							return Err(anyhow!(
								"Workflow '{}' step '{}' references undefined layer '{}'",
								workflow_name,
								step.name,
								layer
							));
						}
					}

					// Check conditional branches (on_match, on_no_match)
					for layer in &step.on_match {
						if !layer_names.contains(layer.as_str()) {
							return Err(anyhow!(
								"Workflow '{}' step '{}' on_match references undefined layer '{}'",
								workflow_name,
								step.name,
								layer
							));
						}
					}
					for layer in &step.on_no_match {
						if !layer_names.contains(layer.as_str()) {
							return Err(anyhow!(
								"Workflow '{}' step '{}' on_no_match references undefined layer '{}'",
								workflow_name,
								step.name,
								layer
							));
						}
					}

					// Check parallel_layers
					for layer in &step.parallel_layers {
						if !layer_names.contains(layer.as_str()) {
							return Err(anyhow!(
								"Workflow '{}' step '{}' parallel_layers references undefined layer '{}'",
								workflow_name,
								step.name,
								layer
							));
						}
					}

					// Check aggregator
					if let Some(aggregator) = &step.aggregator {
						if !layer_names.contains(aggregator.as_str()) {
							return Err(anyhow!(
								"Workflow '{}' step '{}' aggregator references undefined layer '{}'",
								workflow_name,
								step.name,
								aggregator
							));
						}
					}

					// Recursively validate substeps
					for substep in &step.substeps {
						validate_step_layers(substep, layer_names, workflow_name)?;
					}

					Ok(())
				}

				// Validate all top-level steps
				for step in &workflow.steps {
					validate_step_layers(step, &layer_names, &workflow.name)?;
				}
			}
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::session::layers::{InputMode, LayerConfig, OutputMode, OutputRole};

	fn valid_layer() -> LayerConfig {
		LayerConfig {
			name: "test_layer".to_string(),
			description: "A test layer".to_string(),
			command: "octomind acp test_role".to_string(),
			workdir: ".".to_string(),
			input_mode: InputMode::Last,
			output_mode: OutputMode::None,
			output_role: OutputRole::Assistant,
		}
	}

	/// validate_layers doesn't use `self` — it only inspects the layers slice.
	/// We replicate the logic here to test it without needing a full Config.
	fn validate_layer_rules(layers: &[LayerConfig]) -> Result<()> {
		for (index, layer) in layers.iter().enumerate() {
			if layer.name.is_empty() {
				return Err(anyhow!("Layer at index {} has empty name", index));
			}
			if layer.description.is_empty() {
				return Err(anyhow!(
					"Layer '{}' at index {} has empty description",
					layer.name,
					index
				));
			}
			if layer.command.is_empty() {
				return Err(anyhow!(
					"Layer '{}' at index {} has empty command. Layers now execute via ACP protocol — add a 'command' field (e.g., command = 'octomind acp <role>')",
					layer.name,
					index
				));
			}
		}
		Ok(())
	}

	#[test]
	fn validate_layers_empty_command_fails() {
		let mut layer = valid_layer();
		layer.command = String::new();
		let result = validate_layer_rules(&[layer]);
		assert!(result.is_err(), "empty command should fail validation");
		let err = result.unwrap_err().to_string();
		assert!(
			err.contains("empty command"),
			"error should mention 'empty command', got: {err}"
		);
	}

	#[test]
	fn validate_layers_valid_command_passes() {
		let layer = valid_layer();
		let result = validate_layer_rules(&[layer]);
		assert!(result.is_ok(), "valid layer should pass validation");
	}

	#[test]
	fn validate_layers_empty_name_fails() {
		let mut layer = valid_layer();
		layer.name = String::new();
		let result = validate_layer_rules(&[layer]);
		assert!(result.is_err(), "empty name should fail validation");
	}

	#[test]
	fn validate_layers_empty_description_fails() {
		let mut layer = valid_layer();
		layer.description = String::new();
		let result = validate_layer_rules(&[layer]);
		assert!(result.is_err(), "empty description should fail validation");
	}
}
