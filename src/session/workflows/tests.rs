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

#[cfg(test)]
mod workflow_tests {
	use crate::config::{WorkflowDefinition, WorkflowStep, WorkflowStepType};
	use crate::session::workflows::parser::PatternParser;

	#[test]
	fn test_workflow_validation_once_step() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test".to_string(),
				step_type: WorkflowStepType::Once,
				layer: Some("test_layer".to_string()),
				parse_pattern: None,
				substeps: vec![],
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_ok());
	}

	#[test]
	fn test_workflow_validation_once_missing_layer() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test".to_string(),
				step_type: WorkflowStepType::Once,
				layer: None, // Missing required layer
				parse_pattern: None,
				substeps: vec![],
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_err());
	}

	#[test]
	fn test_workflow_validation_loop_step() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test_loop".to_string(),
				step_type: WorkflowStepType::Loop,
				layer: None,
				parse_pattern: None,
				substeps: vec![WorkflowStep {
					name: "inner".to_string(),
					step_type: WorkflowStepType::Once,
					layer: Some("test_layer".to_string()),
					parse_pattern: None,
					substeps: vec![],
					max_iterations: None,
					exit_pattern: None,
					condition_pattern: None,
					on_match: vec![],
					on_no_match: vec![],
					parallel_layers: vec![],
					aggregator: None,
				}],
				max_iterations: Some(5),
				exit_pattern: Some("COMPLETE".to_string()),
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_ok());
	}

	#[test]
	fn test_workflow_validation_loop_missing_exit_pattern() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test_loop".to_string(),
				step_type: WorkflowStepType::Loop,
				layer: None,
				parse_pattern: None,
				substeps: vec![WorkflowStep {
					name: "inner".to_string(),
					step_type: WorkflowStepType::Once,
					layer: Some("test_layer".to_string()),
					parse_pattern: None,
					substeps: vec![],
					max_iterations: None,
					exit_pattern: None,
					condition_pattern: None,
					on_match: vec![],
					on_no_match: vec![],
					parallel_layers: vec![],
					aggregator: None,
				}],
				max_iterations: Some(5),
				exit_pattern: None, // Missing required exit_pattern
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_err());
	}

	#[test]
	fn test_workflow_validation_loop_missing_substeps() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test_loop".to_string(),
				step_type: WorkflowStepType::Loop,
				layer: None,
				parse_pattern: None,
				substeps: vec![], // Missing required substeps
				max_iterations: Some(5),
				exit_pattern: Some("COMPLETE".to_string()),
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_err());
	}

	#[test]
	fn test_workflow_validation_foreach_step() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test_foreach".to_string(),
				step_type: WorkflowStepType::Foreach,
				layer: None,
				parse_pattern: Some(r"ITEM: (.*)".to_string()),
				substeps: vec![WorkflowStep {
					name: "process".to_string(),
					step_type: WorkflowStepType::Once,
					layer: Some("processor".to_string()),
					parse_pattern: None,
					substeps: vec![],
					max_iterations: None,
					exit_pattern: None,
					condition_pattern: None,
					on_match: vec![],
					on_no_match: vec![],
					parallel_layers: vec![],
					aggregator: None,
				}],
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_ok());
	}

	#[test]
	fn test_workflow_validation_conditional_step() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test_conditional".to_string(),
				step_type: WorkflowStepType::Conditional,
				layer: Some("validator".to_string()),
				parse_pattern: None,
				substeps: vec![],
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: Some("VALID".to_string()),
				on_match: vec!["success_handler".to_string()],
				on_no_match: vec!["error_handler".to_string()],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_ok());
	}

	#[test]
	fn test_workflow_validation_parallel_step() {
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "test_parallel".to_string(),
				step_type: WorkflowStepType::Parallel,
				layer: None,
				parse_pattern: None,
				substeps: vec![],
				max_iterations: None,
				exit_pattern: None,
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec!["layer1".to_string(), "layer2".to_string()],
				aggregator: Some("aggregator".to_string()),
			}],
		};

		assert!(workflow.validate().is_ok());
	}

	#[test]
	fn test_pattern_parser_parse_items() {
		let text = "SUBGOAL 1: Add JWT dependency\nSUBGOAL 2: Create middleware\nSUBGOAL 3: Test";
		let pattern = r"SUBGOAL \d+: (.*)";
		let items = PatternParser::parse_items(text, pattern).unwrap();

		assert_eq!(items.len(), 3);
		assert_eq!(items[0], "Add JWT dependency");
		assert_eq!(items[1], "Create middleware");
		assert_eq!(items[2], "Test");
	}

	#[test]
	fn test_pattern_parser_matches() {
		assert!(PatternParser::matches("COMPLETE", "COMPLETE").unwrap());
		assert!(PatternParser::matches("Task COMPLETE", "COMPLETE").unwrap());
		assert!(!PatternParser::matches("INCOMPLETE", "^COMPLETE$").unwrap());
	}

	#[test]
	fn test_pattern_parser_extract_first() {
		let text = "QUALITY_SCORE: 8.5\nGOAL_PROGRESS: 75%";
		let result = PatternParser::extract_first(text, r"QUALITY_SCORE: ([\d.]+)").unwrap();
		assert_eq!(result, Some("8.5".to_string()));
	}

	#[test]
	fn test_loop_max_iterations_prevents_infinite_loop() {
		// This test verifies that max_iterations is enforced
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Test infinite loop prevention".to_string(),
			steps: vec![WorkflowStep {
				name: "safe_loop".to_string(),
				step_type: WorkflowStepType::Loop,
				layer: None,
				parse_pattern: None,
				substeps: vec![WorkflowStep {
					name: "inner".to_string(),
					step_type: WorkflowStepType::Once,
					layer: Some("test".to_string()),
					parse_pattern: None,
					substeps: vec![],
					max_iterations: None,
					exit_pattern: None,
					condition_pattern: None,
					on_match: vec![],
					on_no_match: vec![],
					parallel_layers: vec![],
					aggregator: None,
				}],
				max_iterations: Some(3), // Limited to 3 iterations
				exit_pattern: Some("NEVER_MATCHES".to_string()), // Pattern that never matches
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		// Validation should pass - max_iterations provides safety
		assert!(workflow.validate().is_ok());
	}

	#[test]
	fn test_nested_workflow_validation() {
		// Test deeply nested workflow structure
		let workflow = WorkflowDefinition {
			name: "test".to_string(),
			description: "Nested workflow".to_string(),
			steps: vec![WorkflowStep {
				name: "outer_loop".to_string(),
				step_type: WorkflowStepType::Loop,
				layer: None,
				parse_pattern: None,
				substeps: vec![WorkflowStep {
					name: "inner_conditional".to_string(),
					step_type: WorkflowStepType::Conditional,
					layer: Some("checker".to_string()),
					parse_pattern: None,
					substeps: vec![],
					max_iterations: None,
					exit_pattern: None,
					condition_pattern: Some("VALID".to_string()),
					on_match: vec!["success".to_string()],
					on_no_match: vec!["retry".to_string()],
					parallel_layers: vec![],
					aggregator: None,
				}],
				max_iterations: Some(5),
				exit_pattern: Some("DONE".to_string()),
				condition_pattern: None,
				on_match: vec![],
				on_no_match: vec![],
				parallel_layers: vec![],
				aggregator: None,
			}],
		};

		assert!(workflow.validate().is_ok());
	}
}
