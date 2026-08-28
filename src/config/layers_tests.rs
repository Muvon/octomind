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

use super::*;
use crate::session::layers::{InputMode, OutputMode, OutputRole};
use std::collections::HashMap;

fn layer_config(name: &str, command: &str) -> LayerConfig {
	LayerConfig {
		name: name.to_string(),
		description: format!("{name} layer"),
		command: command.to_string(),
		workdir: ".".to_string(),
		input_mode: InputMode::All,
		output_mode: OutputMode::Append,
		output_role: OutputRole::Assistant,
	}
}

#[test]
fn is_default_for_serialization_true_when_empty() {
	let config = LayersConfig {
		layers: HashMap::new(),
	};
	assert!(config.is_default_for_serialization());
}

#[test]
fn is_default_for_serialization_false_when_non_empty() {
	let mut layers = HashMap::new();
	layers.insert("reviewer".to_string(), layer_config("reviewer", "octomind"));
	let config = LayersConfig { layers };
	assert!(!config.is_default_for_serialization());
}

#[test]
fn get_all_layers_empty_returns_empty_vec() {
	let config = LayersConfig {
		layers: HashMap::new(),
	};
	assert!(config.get_all_layers().is_empty());
}

#[test]
fn get_all_layers_sets_name_from_registry_key() {
	let mut layers = HashMap::new();
	// name field deliberately differs from the registry key to prove override
	layers.insert(
		"reviewer".to_string(),
		layer_config("wrong-name", "octomind review"),
	);
	layers.insert(
		"tester".to_string(),
		layer_config("also-wrong", "octomind test"),
	);
	let config = LayersConfig { layers };

	let mut result = config.get_all_layers();
	result.sort_by(|a, b| a.name.cmp(&b.name));

	assert_eq!(result.len(), 2);
	assert_eq!(result[0].name, "reviewer");
	assert_eq!(result[0].command, "octomind review");
	assert_eq!(result[1].name, "tester");
	assert_eq!(result[1].command, "octomind test");
}
