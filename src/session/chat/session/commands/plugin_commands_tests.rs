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

//! `/command` dispatch and `/info` rows for plugins, driven through the real
//! `process_command` with a recording stub plugin.

use super::*;
use crate::plugins::protocol::event;
use crate::plugins::test_support::{connect_stub, StubPlugin};
use serde_json::{json, Value};

fn test_config() -> Config {
	let mut config: Config = toml::from_str(include_str!("../../../../../config-templates/default.toml"))
		.expect("parse default config template");
	config.build_role_map();
	config
}

/// A session with its own id and `stub` switched on for it.
async fn session_with(stub: &StubPlugin) -> ChatSession {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.name = uuid::Uuid::new_v4().to_string();
	connect_stub(&session.session.info.name, "assistant", std::path::Path::new("."), stub).await;
	session
}

fn stub_with_commands(commands: Value, answer: Value) -> StubPlugin {
	StubPlugin::new(&[event::COMMAND, event::STATUS])
		.init_field("commands", commands)
		.respond(move |_, _| Ok(answer.clone()))
}

async fn dispatch(session: &mut ChatSession, input: &str) -> CommandOutput {
	let mut config = test_config();
	let (_, cancel) = tokio::sync::watch::channel(false);
	match process_command(session, input, &mut config, "assistant", cancel)
		.await
		.expect("dispatch")
	{
		CommandResult::HandledWithOutput(output) => *output,
		other => panic!("expected typed output, got {other:?}"),
	}
}

#[tokio::test]
async fn a_declared_command_reaches_the_plugin_and_its_text_and_data_come_back() {
	let stub = stub_with_commands(
		json!([{"name": "recall", "hint": "[topic]", "description": "Recall lessons"}]),
		json!({"output": {"text": "two lessons", "data": {"count": 2}}}),
	);
	let mut session = session_with(&stub).await;

	let mut output = dispatch(&mut session, "/recall  auth tokens  ").await;

	let sent = &stub.sent(event::COMMAND)[0];
	assert_eq!(sent["name"], "recall");
	assert_eq!(sent["arguments"], "auth tokens  ");
	match &output {
		CommandOutput::Plugin {
			plugin, command, text, ..
		} => {
			assert_eq!(plugin, stub.name());
			assert_eq!(command, "/recall");
			assert_eq!(text, "two lessons");
		}
		other => panic!("expected plugin output, got {other:?}"),
	}
	let json = output.to_json();
	assert_eq!(json["command_type"], "plugin");
	assert_eq!(json["data"], json!({"count": 2}));

	// The terminal path renders the text without panicking.
	output.display_cli(&mut session, &test_config()).await;
	crate::plugins::end_session(&session.session.info.name);
}

#[tokio::test]
async fn a_command_without_data_is_serialised_as_its_text() {
	let stub = stub_with_commands(
		json!([{"name": "recall", "hint": "", "description": "Recall"}]),
		json!({"output": {"text": "plain words"}}),
	);
	let mut session = session_with(&stub).await;

	let output = dispatch(&mut session, "/recall").await;

	assert_eq!(output.to_json()["data"], json!({"text": "plain words"}));
	assert_eq!(stub.sent(event::COMMAND)[0]["arguments"], "");
	crate::plugins::end_session(&session.session.info.name);
}

#[tokio::test]
async fn a_plugin_command_shadows_a_built_in_of_the_same_name() {
	let stub = stub_with_commands(
		json!([{"name": "help", "hint": "", "description": "Plugin help"}]),
		json!({"output": {"text": "plugin help"}}),
	);
	let mut session = session_with(&stub).await;

	let output = dispatch(&mut session, "/help").await;

	assert!(matches!(output, CommandOutput::Plugin { .. }), "{output:?}");
	crate::plugins::end_session(&session.session.info.name);
}

#[tokio::test]
async fn an_unanswered_command_says_which_plugin_did_not_answer_which_command() {
	let stub = StubPlugin::new(&[event::COMMAND])
		.init_field(
			"commands",
			json!([{"name": "recall", "hint": "", "description": "Recall"}]),
		)
		.respond(|_, _| Err(anyhow::anyhow!("plugin is stuck")));
	let mut session = session_with(&stub).await;

	let output = dispatch(&mut session, "/recall now").await;

	let CommandOutput::Error { error, .. } = output else {
		panic!("expected an error line");
	};
	assert!(error.contains(stub.name()) && error.contains("/recall"), "{error}");
	crate::plugins::end_session(&session.session.info.name);
}

#[tokio::test]
async fn info_lists_each_plugins_rows_in_the_json() {
	let stub = stub_with_commands(
		json!([]),
		json!({"rows": [{"label": "lessons", "value": "12"}, {"label": "gate", "value": "3 blocked"}]}),
	);
	let mut session = session_with(&stub).await;

	let mut output = dispatch(&mut session, "/info").await;

	let json = output.to_json();
	assert_eq!(
		json["plugins"],
		json!([{"plugin": stub.name(), "rows": [
			{"label": "lessons", "value": "12"},
			{"label": "gate", "value": "3 blocked"},
		]}])
	);
	output.display_cli(&mut session, &test_config()).await;
	crate::plugins::end_session(&session.session.info.name);
}

#[tokio::test]
async fn without_a_plugin_info_has_no_plugin_block_and_a_slash_word_is_chat() {
	let mut session = ChatSession::for_tests(Vec::new());
	session.session.info.name = uuid::Uuid::new_v4().to_string();

	let output = dispatch(&mut session, "/info").await;
	assert!(output.to_json().get("plugins").is_none());

	let mut config = test_config();
	let (_, cancel) = tokio::sync::watch::channel(false);
	let result = process_command(&mut session, "/recall", &mut config, "assistant", cancel)
		.await
		.expect("dispatch");
	assert!(matches!(result, CommandResult::TreatAsUserInput));
}
