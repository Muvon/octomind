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

fn parse(body: &str) -> PluginConfig {
	toml::from_str(body).expect("plugin entry must parse")
}

#[test]
fn a_new_configuration_lists_the_default_plugin_and_the_template_lists_none() {
	let template: toml::Value = toml::from_str(DEFAULT_CONFIG_TEMPLATE).expect("the template parses");
	assert_eq!(template["plugins"].as_array().map(Vec::len), Some(0));

	let text = new_config_text();
	let config: crate::config::Config = toml::from_str(&text).expect("a new configuration parses");
	assert_eq!(config.plugins.len(), 1);
	let plugin = &config.plugins[0];
	assert_eq!(plugin.name, DEFAULT_PLUGIN_NAME);
	assert!(plugin.enabled);
	plugin.validate().expect("the default plugin entry is valid");

	// Three lines of the plugin list replace the one line of the empty list.
	assert_eq!(text.lines().count(), DEFAULT_CONFIG_TEMPLATE.lines().count() + 2);
}

#[test]
fn minimal_entries_take_the_item_defaults() {
	let plugin = parse("name = \"a\"\ntype = \"stdio\"\ncommand = \"prog\"\n");
	assert!(plugin.enabled);
	assert!(plugin.roles.is_empty());
	assert_eq!(plugin.timeout_seconds, None);
	plugin.validate().expect("minimal stdio entry is valid");
}

#[test]
fn unknown_keys_are_rejected() {
	let result: Result<PluginConfig, _> =
		toml::from_str("name = \"a\"\ntype = \"stdio\"\ncommand = \"p\"\ncomand = \"x\"\n");
	assert!(result.is_err());
}

#[test]
fn unknown_type_is_rejected() {
	let result: Result<PluginConfig, _> = toml::from_str("name = \"a\"\ntype = \"pipe\"\n");
	assert!(result.is_err());
}

#[test]
fn each_type_demands_its_own_field() {
	let missing = [
		("name = \"a\"\ntype = \"stdio\"\n", "command"),
		("name = \"a\"\ntype = \"http\"\n", "url"),
		("name = \"a\"\ntype = \"stdio\"\ncommand = \" \"\n", "command"),
	];
	for (body, field) in missing {
		let error = parse(body).validate().expect_err(body).to_string();
		assert!(error.contains(field), "{error}");
	}
	if cfg!(unix) {
		let error = parse("name = \"a\"\ntype = \"socket\"\n")
			.validate()
			.unwrap_err()
			.to_string();
		assert!(error.contains("path"), "{error}");
		parse("name = \"a\"\ntype = \"socket\"\npath = \"/tmp/a.sock\"\n")
			.validate()
			.expect("socket without command is valid");
	}
}

#[test]
fn socket_is_rejected_on_windows_with_a_hint() {
	let result = parse("name = \"a\"\ntype = \"socket\"\npath = \"/tmp/a.sock\"\n").validate();
	if cfg!(windows) {
		assert!(result.unwrap_err().to_string().contains("stdio"));
	} else {
		result.expect("socket is valid off Windows");
	}
}

#[test]
fn http_url_must_be_http_or_https() {
	parse("name = \"a\"\ntype = \"http\"\nurl = \"https://example.com/rpc\"\n")
		.validate()
		.expect("https url");
	for url in ["ftp://example.com", "not a url"] {
		let body = format!("name = \"a\"\ntype = \"http\"\nurl = \"{url}\"\n");
		assert!(parse(&body).validate().is_err(), "{url}");
	}
}

#[test]
fn timeout_bounds_are_enforced() {
	for (seconds, valid) in [
		(0, false),
		(1, true),
		(MAX_PLUGIN_TIMEOUT_SECONDS, true),
		(MAX_PLUGIN_TIMEOUT_SECONDS + 1, false),
	] {
		let body = format!("name = \"a\"\ntype = \"stdio\"\ncommand = \"p\"\ntimeout_seconds = {seconds}\n");
		assert_eq!(parse(&body).validate().is_ok(), valid, "{seconds}");
	}
}

#[test]
fn empty_name_is_rejected() {
	assert!(parse("name = \"\"\ntype = \"stdio\"\ncommand = \"p\"\n")
		.validate()
		.is_err());
}

#[test]
fn roles_use_the_guardrail_matching_rule() {
	let plugin = parse("name = \"a\"\ntype = \"stdio\"\ncommand = \"p\"\nroles = [\"developer\"]\n");
	assert!(plugin.applies_to_role("developer:general"));
	assert!(plugin.applies_to_role("developer"));
	assert!(!plugin.applies_to_role("assistant:chief"));
	let all = parse("name = \"a\"\ntype = \"stdio\"\ncommand = \"p\"\n");
	assert!(all.applies_to_role("anything"));
}

#[test]
fn serialization_round_trip() {
	let plugin = parse(
		"name = \"a\"\ntype = \"http\"\nurl = \"http://localhost:1/rpc\"\nheaders = { Authorization = \"Bearer {{ENV:T}}\" }\nenabled = false\nroles = [\"x\"]\ntimeout_seconds = 5\n",
	);
	let text = toml::to_string(&plugin).expect("serialize");
	assert_eq!(parse(&text), plugin);
}

const VALID_KEY: &str = "NP/qvbvD3tG3vqBlp+m6aMIzjM4U0p2QdWdpok15v9Y=";

fn install_entry(kind: &str, extra: &str) -> PluginConfig {
	parse(&format!(
		"name = \"a\"\ntype = \"{kind}\"\ncommand = \"prog\"\npath = \"/tmp/a.sock\"\nurl = \"https://example.com/rpc\"\n{extra}"
	))
}

#[test]
fn install_keys_are_accepted_together_and_survive_a_round_trip() {
	let both = format!("install_manifest = \"https://dl.example.com/latest.json\"\ninstall_key = \"{VALID_KEY}\"\n");
	let plugin = install_entry("stdio", &both);
	plugin.validate().expect("both keys are valid");
	assert_eq!(parse(&toml::to_string(&plugin).unwrap()), plugin);
	if cfg!(unix) {
		install_entry("socket", &both)
			.validate()
			.expect("socket may install too");
	}
	install_entry("stdio", "").validate().expect("neither key is valid");
}

#[test]
fn install_keys_come_in_pairs() {
	for extra in [
		"install_manifest = \"https://dl.example.com/latest.json\"\n".to_string(),
		format!("install_key = \"{VALID_KEY}\"\n"),
	] {
		let error = install_entry("stdio", &extra).validate().unwrap_err().to_string();
		assert!(error.contains("both"), "{error}");
	}
}

#[test]
fn the_manifest_must_be_an_https_url_and_the_key_32_bytes_of_base64() {
	let bad = [
		(
			"install_manifest = \"http://dl.example.com/latest.json\"",
			VALID_KEY,
			"https",
		),
		("install_manifest = \"not a url\"", VALID_KEY, "install_manifest"),
		(
			"install_manifest = \"https://dl.example.com/latest.json\"",
			"AAAA",
			"32",
		),
		(
			"install_manifest = \"https://dl.example.com/latest.json\"",
			"!!!",
			"install_key",
		),
	];
	for (manifest, key, expected) in bad {
		let plugin = install_entry("stdio", &format!("{manifest}\ninstall_key = \"{key}\"\n"));
		let error = format!("{:#}", plugin.validate().unwrap_err());
		assert!(error.contains(expected), "{error}");
	}
}

#[test]
fn http_plugins_cannot_install() {
	let both = format!("install_manifest = \"https://dl.example.com/latest.json\"\ninstall_key = \"{VALID_KEY}\"\n");
	let error = install_entry("http", &both).validate().unwrap_err().to_string();
	assert!(error.contains("stdio"), "{error}");
}
