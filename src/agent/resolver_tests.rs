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

#[test]
fn test_inject_role_name_overrides_first_role() {
	let manifest = "[[roles]]\nname = \"old\"\nsystem = \"do things\"\n";
	let injected = inject_role_name(manifest, "doctor:blood").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert_eq!(value["roles"][0]["name"].as_str(), Some("doctor:blood"));
	// Other fields survive the roundtrip
	assert_eq!(value["roles"][0]["system"].as_str(), Some("do things"));
}

#[test]
fn test_inject_role_name_without_roles_is_noop() {
	let injected = inject_role_name("version = 1\n", "tag").expect("inject");
	let value: toml::Value = toml::from_str(&injected).expect("valid toml");
	assert!(value.get("roles").is_none());
	assert_eq!(value["version"].as_integer(), Some(1));
}

#[test]
fn test_inject_role_name_invalid_toml_errors() {
	assert!(inject_role_name("not = = toml", "tag").is_err());
}
