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
use crate::plugins::{PluginBlock, PluginStatusField};

fn pin() -> PluginBlock {
	PluginBlock {
		plugin: "besydr".into(),
		name: "goal".into(),
		text: "keep the API stable".into(),
		items: vec!["no breaking changes".into()],
	}
}

fn field() -> PluginStatusField {
	PluginStatusField {
		plugin: "besydr".into(),
		name: "risk".into(),
		instruction: "how risky the change is".into(),
		default: serde_json::json!("low"),
		example: None,
	}
}

#[test]
fn pins_and_status_fields_come_back_after_save_and_load() {
	let file = tempfile::Builder::new().suffix(".jsonl.zst").tempfile().unwrap();
	let mut session = Session {
		info: SessionInfo {
			name: "plugin-pins-resume".into(),
			plugin_pins: vec![pin()],
			plugin_status_fields: vec![field()],
			..Default::default()
		},
		messages: Vec::new(),
		session_file: Some(file.path().to_path_buf()),
	};
	session.save().unwrap();

	let resumed = load_session(&file.path().to_path_buf()).unwrap();
	assert_eq!(resumed.info.plugin_pins, vec![pin()]);
	assert_eq!(resumed.info.plugin_status_fields, vec![field()]);

	session.info.plugin_pins.clear();
	session.save().unwrap();
	let resumed = load_session(&file.path().to_path_buf()).unwrap();
	assert!(resumed.info.plugin_pins.is_empty(), "the newest save wins");
}

#[test]
fn a_session_without_plugins_is_saved_as_before_and_old_files_load() {
	let mut value = serde_json::to_value(SessionInfo::default()).unwrap();
	let fields = value.as_object_mut().unwrap();
	assert!(!fields.contains_key("plugin_pins") && !fields.contains_key("plugin_status_fields"));

	let info: SessionInfo = serde_json::from_value(value).unwrap();
	assert!(info.plugin_pins.is_empty() && info.plugin_status_fields.is_empty());
}
