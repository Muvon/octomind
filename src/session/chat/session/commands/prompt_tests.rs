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

//! Handler-level tests for the `/prompt` session command: bare listing,
//! unknown-name failure with the available prompts, and a successful
//! template lookup.

use super::*;
use crate::config::Config;

fn template_config() -> Config {
	let mut config: Config =
		toml::from_str(include_str!("../../../../../config-templates/default.toml"))
			.expect("parse default config template");
	config.build_role_map();
	config
}

/// Run one `/prompt` invocation and return its typed output.
async fn run(config: &Config, params: &[&str]) -> CommandOutput {
	let mut session = ChatSession::for_tests(Vec::new());
	let result = handle_prompt(&mut session, config, "core", params)
		.await
		.expect("prompt command is handled, not an error");
	let CommandResult::HandledWithOutput(output) = result else {
		panic!("expected typed output");
	};
	*output
}

#[tokio::test]
async fn empty_params_lists_configured_prompts() {
	let config = template_config();
	let CommandOutput::Prompt { data } = run(&config, &[]).await else {
		panic!("expected prompt output");
	};
	assert_eq!(data["action"], "list");
	let prompts = data["prompts"].as_array().expect("prompts array");
	assert_eq!(prompts.len(), config.prompts.len());
	assert!(prompts.iter().any(|p| p["name"] == "review"));
}

#[tokio::test]
async fn unknown_prompt_reports_error_and_available_names() {
	let config = template_config();
	let CommandOutput::Prompt { data } = run(&config, &["nope"]).await else {
		panic!("expected prompt output");
	};
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], false);
	assert!(
		data["error"].as_str().expect("error text").contains("nope"),
		"{}",
		data["error"]
	);
	let names: Vec<&str> = data["available_prompts"]
		.as_array()
		.expect("available prompts")
		.iter()
		.map(|v| v.as_str().expect("prompt name"))
		.collect();
	assert!(names.contains(&"review"), "{names:?}");
}

#[tokio::test]
async fn known_prompt_returns_processed_content() {
	let config = template_config();
	let expected = config
		.prompts
		.iter()
		.find(|p| p.name == "review")
		.expect("review prompt in template")
		.prompt
		.clone();
	let CommandOutput::Prompt { data } = run(&config, &["review"]).await else {
		panic!("expected prompt output");
	};
	assert_eq!(data["action"], "execute");
	assert_eq!(data["success"], true);
	assert_eq!(data["prompt_name"], "review");
	assert_eq!(data["prompt_content"], expected);
}
