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

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn test_command_listing_and_lookup_agree() {
	let config = template_config();
	let commands = list_available_commands(&config, "assistant");
	// The template ships command layers (review/explain/optimize/test)
	assert!(!commands.is_empty(), "template defines command layers");
	for name in &commands {
		assert!(
			command_exists(&config, "assistant", name),
			"listed command {name} must exist"
		);
	}
	assert!(!command_exists(&config, "assistant", "no-such-command"));
}

#[test]
fn test_command_help_names_every_command() {
	let config = template_config();
	let help = get_command_help(&config, "assistant");
	for name in list_available_commands(&config, "assistant") {
		assert!(help.contains(&name), "help must mention {name}: {help}");
	}
}
