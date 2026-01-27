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

		// STRICT: Validate required fields are not empty
		self.validate_required_fields()?;

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

		// Validate ask configuration - STRICT MODE: system field is required
		if self.ask.system.is_empty() {
			return Err(anyhow!("Ask configuration 'system' field cannot be empty"));
		}

		// Validate shell configuration - STRICT MODE: system field is required
		if self.shell.system.is_empty() {
			return Err(anyhow!(
				"Shell configuration 'system' field cannot be empty"
			));
		}

		// Validate temperature ranges for ask and shell
		if self.ask.temperature < 0.0 || self.ask.temperature > 2.0 {
			return Err(anyhow!(
				"Ask configuration temperature must be between 0.0 and 2.0, got: {}",
				self.ask.temperature
			));
		}

		// Validate top_p ranges for ask
		if self.ask.top_p < 0.0 || self.ask.top_p > 1.0 {
			return Err(anyhow!(
				"Ask configuration top_p must be between 0.0 and 1.0, got: {}",
				self.ask.top_p
			));
		}

		// Validate top_k ranges for ask
		if self.ask.top_k < 1 || self.ask.top_k > 1000 {
			return Err(anyhow!(
				"Ask configuration top_k must be between 1 and 1000, got: {}",
				self.ask.top_k
			));
		}

		if self.shell.temperature < 0.0 || self.shell.temperature > 2.0 {
			return Err(anyhow!(
				"Shell configuration temperature must be between 0.0 and 2.0, got: {}",
				self.shell.temperature
			));
		}

		// Validate top_p ranges for shell
		if self.shell.top_p < 0.0 || self.shell.top_p > 1.0 {
			return Err(anyhow!(
				"Shell configuration top_p must be between 0.0 and 1.0, got: {}",
				self.shell.top_p
			));
		}

		// Validate top_k ranges for shell
		if self.shell.top_k < 1 || self.shell.top_k > 1000 {
			return Err(anyhow!(
				"Shell configuration top_k must be between 1 and 1000, got: {}",
				self.shell.top_k
			));
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

		// Validate layer configurations
		if let Some(layers) = &self.layers {
			for layer in layers {
				// Validate temperature
				if layer.temperature < 0.0 || layer.temperature > 2.0 {
					return Err(anyhow!(
						"Layer '{}' temperature must be between 0.0 and 2.0, got: {}",
						layer.name,
						layer.temperature
					));
				}

				// Validate top_p
				if layer.top_p < 0.0 || layer.top_p > 1.0 {
					return Err(anyhow!(
						"Layer '{}' top_p must be between 0.0 and 1.0, got: {}",
						layer.name,
						layer.top_p
					));
				}

				// Validate top_k
				if layer.top_k < 1 || layer.top_k > 1000 {
					return Err(anyhow!(
						"Layer '{}' top_k must be between 1 and 1000, got: {}",
						layer.name,
						layer.top_k
					));
				}
			}
		}

		// Role configurations no longer have models - using system-wide model

		Ok(())
	}

	pub fn validate_thresholds(&self) -> Result<()> {
		// Validate cache tokens threshold (0 is valid for disabling)
		if self.cache_tokens_threshold > 1_000_000 {
			return Err(anyhow!(
				"Cache tokens threshold too high: {}. Maximum allowed: 1,000,000",
				self.cache_tokens_threshold
			));
		}

		// Validate MCP response warning threshold (0 is valid for disabling)
		if self.mcp_response_warning_threshold > 1_000_000 {
			return Err(anyhow!(
				"MCP response warning threshold too high: {}. Maximum allowed: 1,000,000",
				self.mcp_response_warning_threshold
			));
		}

		// Validate max session tokens threshold (0 = disabled, >0 = enabled)
		if self.max_session_tokens_threshold > 2_000_000 {
			return Err(anyhow!(
				"Max session tokens threshold too high: {}. Maximum allowed: 2,000,000",
				self.max_session_tokens_threshold
			));
		}

		// Validate cache timeout
		if self.cache_timeout_seconds > 86400 {
			// 24 hours max
			return Err(anyhow!(
				"Cache timeout too high: {} seconds. Maximum allowed: 86400 (24 hours)",
				self.cache_timeout_seconds
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

			// Validate layer name is not empty (layer_type field doesn't exist)
			// Additional layer-specific validation can be added here if needed

			// Additional layer-specific validation can be added here
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
