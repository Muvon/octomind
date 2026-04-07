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

use serde::{Deserialize, Serialize};

fn default_pipeline_timeout() -> u64 {
	30
}

/// Complete pipeline definition — deterministic script steps that run before workflows
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PipelineDefinition {
	pub name: String,
	pub description: String,
	pub steps: Vec<PipelineStep>,
}

/// Single pipeline step — executes an external script
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PipelineStep {
	pub name: String,

	#[serde(rename = "type")]
	pub step_type: PipelineStepType,

	/// Script command to execute (for Once, Conditional types)
	#[serde(skip_serializing_if = "Option::is_none")]
	pub command: Option<String>,

	/// Pattern to parse items from input (for Foreach)
	#[serde(skip_serializing_if = "Option::is_none")]
	pub parse_pattern: Option<String>,

	/// Nested steps (for Loop, Foreach)
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub substeps: Vec<PipelineStep>,

	/// Maximum loop iterations
	#[serde(skip_serializing_if = "Option::is_none")]
	pub max_iterations: Option<usize>,

	/// Pattern to exit loop when matched in stdout
	#[serde(skip_serializing_if = "Option::is_none")]
	pub exit_pattern: Option<String>,

	/// Pattern to check in stdout for conditional branching
	#[serde(skip_serializing_if = "Option::is_none")]
	pub condition_pattern: Option<String>,

	/// Commands to run when condition_pattern matches
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub on_match: Vec<String>,

	/// Commands to run when condition_pattern does not match
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub on_no_match: Vec<String>,

	/// Timeout in seconds for script execution (default: 30)
	#[serde(default = "default_pipeline_timeout")]
	pub timeout: u64,
}

/// Pipeline step types
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PipelineStepType {
	Once,        // Execute command once
	Loop,        // Repeat substeps until exit condition
	Foreach,     // Parse items, run substeps for each
	Conditional, // Branch based on stdout pattern
}

impl PipelineDefinition {
	pub fn validate(&self) -> Result<(), String> {
		if self.name.trim().is_empty() {
			return Err("Pipeline name cannot be empty".to_string());
		}

		if self.steps.is_empty() {
			return Err(format!("Pipeline '{}' has no steps", self.name));
		}

		for (i, step) in self.steps.iter().enumerate() {
			step.validate(&format!("{}[{}]", self.name, i))?;
		}

		Ok(())
	}
}

impl PipelineStep {
	pub fn validate(&self, path: &str) -> Result<(), String> {
		match self.step_type {
			PipelineStepType::Once => {
				if self.command.is_none() {
					return Err(format!("{}: Once requires 'command'", path));
				}
			}
			PipelineStepType::Loop => {
				if self.substeps.is_empty() {
					return Err(format!("{}: Loop requires 'substeps'", path));
				}
				if self.exit_pattern.is_none() {
					return Err(format!("{}: Loop requires 'exit_pattern'", path));
				}
			}
			PipelineStepType::Foreach => {
				if self.parse_pattern.is_none() {
					return Err(format!("{}: Foreach requires 'parse_pattern'", path));
				}
				if self.substeps.is_empty() {
					return Err(format!("{}: Foreach requires 'substeps'", path));
				}
			}
			PipelineStepType::Conditional => {
				if self.command.is_none() {
					return Err(format!("{}: Conditional requires 'command'", path));
				}
				if self.condition_pattern.is_none() {
					return Err(format!(
						"{}: Conditional requires 'condition_pattern'",
						path
					));
				}
			}
		}

		if self.timeout == 0 {
			return Err(format!("{}: timeout must be > 0", path));
		}

		if self.timeout > 3600 {
			return Err(format!("{}: timeout too high (max 3600)", path));
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
	fn test_pipeline_validation_empty_steps() {
		let pipeline = PipelineDefinition {
			name: "test".to_string(),
			description: "Test".to_string(),
			steps: vec![],
		};
		assert!(pipeline.validate().is_err());
	}

	#[test]
	fn test_pipeline_validation_empty_name() {
		let pipeline = PipelineDefinition {
			name: "".to_string(),
			description: "Test".to_string(),
			steps: vec![PipelineStep {
				name: "test".to_string(),
				step_type: PipelineStepType::Once,
				command: Some("./test.sh".to_string()),
				parse_pattern: None,
				substeps: Vec::new(),
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: Vec::new(),
				on_no_match: Vec::new(),
				timeout: 30,
			}],
		};
		assert!(pipeline.validate().is_err());
	}

	#[test]
	fn test_once_step_validation() {
		let step = PipelineStep {
			name: "test".to_string(),
			step_type: PipelineStepType::Once,
			command: Some("./test.sh".to_string()),
			parse_pattern: None,
			substeps: Vec::new(),
			max_iterations: None,
			exit_pattern: None,
			condition_pattern: None,
			on_match: Vec::new(),
			on_no_match: Vec::new(),
			timeout: 30,
		};
		assert!(step.validate("test").is_ok());
	}

	#[test]
	fn test_once_step_missing_command() {
		let step = PipelineStep {
			name: "test".to_string(),
			step_type: PipelineStepType::Once,
			command: None,
			parse_pattern: None,
			substeps: Vec::new(),
			max_iterations: None,
			exit_pattern: None,
			condition_pattern: None,
			on_match: Vec::new(),
			on_no_match: Vec::new(),
			timeout: 30,
		};
		assert!(step.validate("test").is_err());
	}

	#[test]
	fn test_loop_step_missing_exit_pattern() {
		let step = PipelineStep {
			name: "test".to_string(),
			step_type: PipelineStepType::Loop,
			command: None,
			parse_pattern: None,
			substeps: vec![PipelineStep {
				name: "inner".to_string(),
				step_type: PipelineStepType::Once,
				command: Some("./inner.sh".to_string()),
				parse_pattern: None,
				substeps: Vec::new(),
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: Vec::new(),
				on_no_match: Vec::new(),
				timeout: 30,
			}],
			max_iterations: Some(5),
			exit_pattern: None,
			condition_pattern: None,
			on_match: Vec::new(),
			on_no_match: Vec::new(),
			timeout: 30,
		};
		assert!(step.validate("test").is_err());
	}

	#[test]
	fn test_conditional_step_validation() {
		let step = PipelineStep {
			name: "test".to_string(),
			step_type: PipelineStepType::Conditional,
			command: Some("./check.sh".to_string()),
			parse_pattern: None,
			substeps: Vec::new(),
			max_iterations: None,
			exit_pattern: None,
			condition_pattern: Some("MATCH".to_string()),
			on_match: vec!["./yes.sh".to_string()],
			on_no_match: vec!["./no.sh".to_string()],
			timeout: 30,
		};
		assert!(step.validate("test").is_ok());
	}

	#[test]
	fn test_timeout_zero_invalid() {
		let step = PipelineStep {
			name: "test".to_string(),
			step_type: PipelineStepType::Once,
			command: Some("./test.sh".to_string()),
			parse_pattern: None,
			substeps: Vec::new(),
			max_iterations: None,
			exit_pattern: None,
			condition_pattern: None,
			on_match: Vec::new(),
			on_no_match: Vec::new(),
			timeout: 0,
		};
		assert!(step.validate("test").is_err());
	}
}
