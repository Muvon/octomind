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

//! Read-only paths of the `octomind config` command (list-themes / show /
//! validate all return before any save), rendered against the template
//! config. Setter paths write the user's real config file and are
//! deliberately not exercised here.

use super::*;

fn args() -> ConfigArgs {
	ConfigArgs {
		model: None,
		api_key: None,
		log_level: None,
		mcp_providers: None,
		mcp_server: None,
		system: None,
		markdown_enable: None,
		markdown_theme: None,
		list_themes: false,
		show: false,
		validate: false,
		upgrade: false,
	}
}

fn template_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

#[test]
fn test_list_themes_renders() {
	let mut a = args();
	a.list_themes = true;
	execute(&a, template_config()).expect("list themes is read-only");
}

#[test]
fn test_show_configuration_renders() {
	let mut a = args();
	a.show = true;
	execute(&a, template_config()).expect("show is read-only");
}

#[test]
fn test_validate_template_config() {
	let mut a = args();
	a.validate = true;
	execute(&a, template_config()).expect("template config must validate");
}
