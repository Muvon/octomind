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

fn sample_config() -> LayerConfig {
	LayerConfig {
		name: "task_refiner".to_string(),
		description: "Refines the user task".to_string(),
		command: "octomind --role developer:refiner".to_string(),
		workdir: ".".to_string(),
		input_mode: InputMode::Last,
		output_mode: OutputMode::Append,
		output_role: OutputRole::Assistant,
	}
}

#[test]
fn new_stores_config() {
	let config = sample_config();
	let processor = LayerProcessor::new(config.clone());

	assert_eq!(processor.config, config);
}

#[test]
fn name_returns_config_name() {
	let processor = LayerProcessor::new(sample_config());

	assert_eq!(processor.name(), "task_refiner");
}

#[test]
fn config_returns_reference_to_stored_config() {
	let processor = LayerProcessor::new(sample_config());

	let returned = processor.config();
	assert_eq!(returned.name, "task_refiner");
	assert_eq!(returned.command, "octomind --role developer:refiner");
	// Same stored value, not a copy of a different config
	assert!(std::ptr::eq(returned, &processor.config));
}
