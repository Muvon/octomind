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

use serde::{Deserialize, Serialize};

/// Complete workflow definition
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct WorkflowDefinition {
	pub name: String,
	pub description: String,
	pub steps: Vec<WorkflowStep>,
}

/// Single workflow step
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct WorkflowStep {
	pub name: String,

	#[serde(rename = "type")]
	pub step_type: WorkflowStepType,

	// For Once, Loop, Conditional types
	#[serde(skip_serializing_if = "Option::is_none")]
	pub layer: Option<String>,

	// For Foreach - pattern to parse items
	#[serde(skip_serializing_if = "Option::is_none")]
	pub parse_pattern: Option<String>,

	// Nested steps for Loop, Foreach, Conditional
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub substeps: Vec<WorkflowStep>,

	// Loop configuration
	#[serde(skip_serializing_if = "Option::is_none")]
	pub max_iterations: Option<usize>,

	#[serde(skip_serializing_if = "Option::is_none")]
	pub exit_pattern: Option<String>,

	// Conditional configuration
	#[serde(skip_serializing_if = "Option::is_none")]
	pub condition_pattern: Option<String>,

	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub on_match: Vec<String>,

	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub on_no_match: Vec<String>,

	// Parallel configuration
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub parallel_layers: Vec<String>,

	#[serde(skip_serializing_if = "Option::is_none")]
	pub aggregator: Option<String>,
}

/// Workflow step types
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStepType {
	Once,        // Execute layer once
	Loop,        // Repeat until exit condition
	Foreach,     // Iterate over parsed items
	Conditional, // Branch based on pattern
	Parallel,    // Execute layers in parallel
}

impl WorkflowDefinition {
	pub fn validate(&self) -> Result<(), String> {
		if self.name.trim().is_empty() {
			return Err("Workflow name cannot be empty".to_string());
		}

		if self.steps.is_empty() {
			return Err(format!("Workflow '{}' has no steps", self.name));
		}

		for (i, step) in self.steps.iter().enumerate() {
			step.validate(&format!("{}[{}]", self.name, i))?;
		}

		Ok(())
	}
}

impl WorkflowStep {
	pub fn validate(&self, path: &str) -> Result<(), String> {
		match self.step_type {
			WorkflowStepType::Once => {
				if self.layer.is_none() {
					return Err(format!("{}: Once requires 'layer'", path));
				}
			}
			WorkflowStepType::Loop => {
				if self.substeps.is_empty() {
					return Err(format!("{}: Loop requires 'substeps'", path));
				}
				if self.exit_pattern.is_none() {
					return Err(format!("{}: Loop requires 'exit_pattern'", path));
				}
			}
			WorkflowStepType::Foreach => {
				if self.parse_pattern.is_none() {
					return Err(format!("{}: Foreach requires 'parse_pattern'", path));
				}
				if self.substeps.is_empty() {
					return Err(format!("{}: Foreach requires 'substeps'", path));
				}
			}
			WorkflowStepType::Conditional => {
				if self.layer.is_none() {
					return Err(format!("{}: Conditional requires 'layer'", path));
				}
				if self.condition_pattern.is_none() {
					return Err(format!(
						"{}: Conditional requires 'condition_pattern'",
						path
					));
				}
			}
			WorkflowStepType::Parallel => {
				if self.parallel_layers.is_empty() {
					return Err(format!("{}: Parallel requires 'parallel_layers'", path));
				}
			}
		}

		for (i, substep) in self.substeps.iter().enumerate() {
			substep.validate(&format!("{}[{}]", path, i))?;
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_workflow_validation_empty_steps() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test".to_string(),
			steps: vec![],
		};

		assert!(workflow.validate().is_err());
	}

	#[test]
	fn test_workflow_validation_empty_name() {
		let workflow = WorkflowDefinition {
			name: "".to_string(),
			description: "Test".to_string(),
			steps: vec![WorkflowStep {
				name: "test".to_string(),
				step_type: WorkflowStepType::Once,
				layer: Some("test_layer".to_string()),
				parse_pattern: None,
				substeps: Vec::new(),
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: Vec::new(),
				on_no_match: Vec::new(),
				parallel_layers: Vec::new(),
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_err());
	}

	#[test]
	fn test_once_step_validation() {
		let step = WorkflowStep {
			name: "test".to_string(),
			step_type: WorkflowStepType::Once,
			layer: Some("test_layer".to_string()),
			parse_pattern: None,
			substeps: Vec::new(),
			max_iterations: None,
			exit_pattern: None,
			condition_pattern: None,
			on_match: Vec::new(),
			on_no_match: Vec::new(),
			parallel_layers: Vec::new(),
			aggregator: None,
		};

		assert!(step.validate("test").is_ok());
	}

	#[test]
	fn test_loop_step_missing_exit_pattern() {
		let step = WorkflowStep {
			name: "test".to_string(),
			step_type: WorkflowStepType::Loop,
			layer: None,
			parse_pattern: None,
			substeps: vec![],
			max_iterations: Some(5),
			exit_pattern: None, // Missing!
			condition_pattern: None,
			on_match: Vec::new(),
			on_no_match: Vec::new(),
			parallel_layers: Vec::new(),
			aggregator: None,
		};

		assert!(step.validate("test").is_err());
	}
}
