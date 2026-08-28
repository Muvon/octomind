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
fn input_mode_as_str_returns_variant_names() {
	assert_eq!(InputMode::Last.as_str(), "last");
	assert_eq!(InputMode::All.as_str(), "all");
	assert_eq!(InputMode::Summary.as_str(), "summary");
}

#[test]
fn input_mode_from_str_parses_all_variants() {
	assert_eq!("last".parse::<InputMode>().unwrap(), InputMode::Last);
	assert_eq!("all".parse::<InputMode>().unwrap(), InputMode::All);
	assert_eq!("summary".parse::<InputMode>().unwrap(), InputMode::Summary);
	// Parsing is case-insensitive
	assert_eq!("LAST".parse::<InputMode>().unwrap(), InputMode::Last);
}

#[test]
fn input_mode_from_str_rejects_unknown() {
	let err = "everything".parse::<InputMode>().expect_err("must reject");
	assert!(
		err.contains("Unknown input mode"),
		"unexpected error: {err}"
	);
}

#[test]
fn output_mode_as_str_returns_variant_names() {
	assert_eq!(OutputMode::None.as_str(), "none");
	assert_eq!(OutputMode::Append.as_str(), "append");
	assert_eq!(OutputMode::Replace.as_str(), "replace");
	assert_eq!(OutputMode::Last.as_str(), "last");
	assert_eq!(OutputMode::Restart.as_str(), "restart");
}

#[test]
fn output_mode_from_str_parses_all_variants() {
	assert_eq!("none".parse::<OutputMode>().unwrap(), OutputMode::None);
	assert_eq!("append".parse::<OutputMode>().unwrap(), OutputMode::Append);
	assert_eq!(
		"replace".parse::<OutputMode>().unwrap(),
		OutputMode::Replace
	);
	assert_eq!("last".parse::<OutputMode>().unwrap(), OutputMode::Last);
	assert_eq!(
		"restart".parse::<OutputMode>().unwrap(),
		OutputMode::Restart
	);
}

#[test]
fn output_mode_from_str_rejects_unknown() {
	let err = "overwrite".parse::<OutputMode>().expect_err("must reject");
	assert!(
		err.contains("Unknown output mode"),
		"unexpected error: {err}"
	);
}

#[test]
fn output_role_as_str_returns_variant_names() {
	assert_eq!(OutputRole::Assistant.as_str(), "assistant");
	assert_eq!(OutputRole::User.as_str(), "user");
}

#[test]
fn output_role_from_str_parses_both_variants() {
	assert_eq!(
		"assistant".parse::<OutputRole>().unwrap(),
		OutputRole::Assistant
	);
	assert_eq!("user".parse::<OutputRole>().unwrap(), OutputRole::User);
}

#[test]
fn output_role_from_str_rejects_unknown() {
	let err = "system".parse::<OutputRole>().expect_err("must reject");
	assert!(
		err.contains("Unknown output role"),
		"unexpected error: {err}"
	);
}

#[test]
fn resolved_workdir_absolute_path_returned_as_is() {
	let config = LayerConfig {
		workdir: "/opt/layers".to_string(),
		..sample_config()
	};

	let resolved = config.get_resolved_workdir(std::path::Path::new("/srv/sessions"));
	assert_eq!(resolved, std::path::PathBuf::from("/opt/layers"));
}

#[test]
fn resolved_workdir_relative_path_joins_session_workdir() {
	let config = LayerConfig {
		workdir: "sub/dir".to_string(),
		..sample_config()
	};

	let resolved = config.get_resolved_workdir(std::path::Path::new("/srv/sessions"));
	assert_eq!(resolved, std::path::PathBuf::from("/srv/sessions/sub/dir"));
}

#[test]
fn layer_config_serde_json_roundtrip() {
	let config = LayerConfig {
		input_mode: InputMode::Summary,
		output_mode: OutputMode::Restart,
		output_role: OutputRole::User,
		..sample_config()
	};

	let json = serde_json::to_string(&config).expect("serialize LayerConfig");
	let back: LayerConfig = serde_json::from_str(&json).expect("deserialize LayerConfig");

	assert_eq!(back, config);
}

#[test]
fn layer_config_deserializes_string_modes_from_config() {
	// Config files carry the modes as plain strings; the custom
	// deserializers must accept exactly the as_str() spellings.
	let json = r#"{
		"name": "task_refiner",
		"description": "Refines the user task",
		"command": "octomind --role developer:refiner",
		"input_mode": "all",
		"output_mode": "replace",
		"output_role": "user"
	}"#;

	let config: LayerConfig = serde_json::from_str(json).expect("deserialize LayerConfig");
	assert_eq!(config.workdir, "."); // default_workdir
	assert_eq!(config.input_mode, InputMode::All);
	assert_eq!(config.output_mode, OutputMode::Replace);
	assert_eq!(config.output_role, OutputRole::User);
}
