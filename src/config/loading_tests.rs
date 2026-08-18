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

//! Path-based config load/save round trips against the shipped template in
//! a tempdir — the exact flow `--config <path>` and the setters use.

use super::*;

#[test]
fn test_load_from_path_roundtrip() {
	let tmp = tempfile::tempdir().expect("tempdir");
	let path = tmp.path().join("config.toml");
	std::fs::write(&path, include_str!("../../config-templates/default.toml"))
		.expect("write template");

	let mut config = Config::load_from_path(&path).expect("load template from path");
	assert!(!config.model.is_empty());
	assert!(!config.roles.is_empty());

	// Mutate, save to a new path, reload — the change must survive.
	config.model = "ollama:roundtrip-model".to_string();
	let out = tmp.path().join("saved.toml");
	config.save_to_path(&out).expect("save to path");
	let reloaded = Config::load_from_path(&out).expect("reload saved config");
	assert_eq!(reloaded.model, "ollama:roundtrip-model");

	// The clean copy used for saving parses back too
	let clean = reloaded.create_clean_copy_for_saving();
	let serialized = toml::to_string(&clean).expect("serialize clean copy");
	let reparsed: Config = toml::from_str(&serialized).expect("reparse clean copy");
	assert_eq!(reparsed.model, "ollama:roundtrip-model");
}

#[test]
fn test_load_from_path_failures() {
	let tmp = tempfile::tempdir().expect("tempdir");

	// Missing file
	assert!(Config::load_from_path(&tmp.path().join("absent.toml")).is_err());

	// Present but not valid config TOML
	let bad = tmp.path().join("bad.toml");
	std::fs::write(&bad, "this = [is not : valid").expect("write bad file");
	assert!(Config::load_from_path(&bad).is_err());
}
