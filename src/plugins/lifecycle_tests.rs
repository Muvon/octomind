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
use crate::plugins::state::PluginBlock;
use crate::plugins::test_support::{connect_stub, stub_config, StubPlugin};
use crate::session::chat::test_support::fake_provider_config;
use serde_json::json;

const ROLE: &str = "developer";

fn unique(prefix: &str) -> String {
	format!("{prefix}-{}", uuid::Uuid::new_v4())
}

fn message(role: &str, content: &str) -> crate::session::Message {
	crate::session::Message {
		role: role.into(),
		content: content.into(),
		..Default::default()
	}
}

fn config_for(stub: &StubPlugin) -> Config {
	let mut config = fake_provider_config();
	config.plugins = vec![stub_config(stub)];
	config
}

fn chat(id: &str) -> ChatSession {
	let mut chat = ChatSession::for_tests(vec![message("system", "You are the agent."), message("user", "hi")]);
	chat.session.info.name = id.to_string();
	chat.role = ROLE.to_string();
	chat
}

fn methods(stub: &StubPlugin) -> Vec<String> {
	stub.received().into_iter().map(|(method, _)| method).collect()
}

#[tokio::test]
async fn session_start_carries_the_payload_and_its_register_reaches_the_session() {
	let id = unique("life-start");
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(dir.path().join(AGENTS_FILE), "Be careful.").unwrap();
	let stub = StubPlugin::new(&[event::SESSION_START])
		.answer(json!({"register": {"rules": "the rules"}}))
		.init_field(
			"status_fields",
			json!([{"name": "risk", "instruction": "How risky is the next step.", "default": "low"}]),
		);
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let config = config_for(&stub);
	let mut chat = chat(&id);

	crate::session::context::with_session_id(id.clone(), async {
		crate::mcp::set_session_working_directory(dir.path().to_path_buf());
		session_started(&mut chat, &config, "startup").await.unwrap();
	})
	.await;

	let sent = stub.sent(event::SESSION_START);
	assert_eq!(sent.len(), 1);
	assert_eq!(sent[0]["source"], "startup");
	assert_eq!(sent[0]["session_id"], id.as_str());
	assert_eq!(sent[0]["system_prompt"], "You are the agent.");
	assert_eq!(sent[0]["interactive"], config.output_mode().is_interactive());
	assert!(sent[0]["tools"].is_array());
	assert_eq!(
		sent[0]["context_files"],
		json!([dir.path().join(AGENTS_FILE).to_string_lossy()])
	);
	assert!(sent[0].get("inherited").is_none());

	assert_eq!(
		crate::plugins::registered_rules(&id),
		vec![(stub.name().to_string(), "the rules".to_string())]
	);
	let fields = &chat.session.info.plugin_status_fields;
	assert_eq!(fields.len(), 1);
	assert_eq!(fields[0].name, "risk");
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn resume_restores_the_pins_and_sends_the_full_message_list() {
	let id = unique("life-resume");
	let dir = tempfile::tempdir().unwrap();
	let stub = StubPlugin::new(&[event::SESSION_START]);
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let config = config_for(&stub);
	let mut chat = chat(&id);
	let pin = PluginBlock {
		plugin: stub.name().to_string(),
		name: "goal".into(),
		text: "Ship it.".into(),
		keep: false,
		items: Vec::new(),
	};
	chat.session.info.plugin_pins = vec![pin.clone()];

	crate::session::context::with_session_id(id.clone(), async {
		session_started(&mut chat, &config, "resume").await.unwrap();
	})
	.await;

	assert_eq!(crate::plugins::pinned_blocks(&id), vec![pin]);
	let sent = stub.sent(event::SESSION_START);
	assert_eq!(sent[0]["source"], "resume");
	assert_eq!(sent[0]["messages_reset"], true);
	assert_eq!(sent[0]["messages"].as_array().unwrap().len(), 2);
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn nothing_is_sent_without_a_plugin_in_the_config() {
	let id = unique("life-none");
	let dir = tempfile::tempdir().unwrap();
	let stub = StubPlugin::new(&[event::SESSION_START]);
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let mut chat = chat(&id);

	session_started(&mut chat, &fake_provider_config(), "startup")
		.await
		.unwrap();

	assert!(stub.sent(event::SESSION_START).is_empty());
	crate::plugins::end_session(&id);
}

#[tokio::test]
async fn session_end_is_sent_once_and_drops_the_plugin_state() {
	let id = unique("life-end");
	let dir = tempfile::tempdir().unwrap();
	let stub = StubPlugin::new(&[event::SESSION_END]);
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let mut chat = chat(&id);

	session_ended(&mut chat, REASON_SWITCH).await;
	session_ended(&mut chat, REASON_EXIT).await;

	let sent = stub.sent(event::SESSION_END);
	assert_eq!(sent.len(), 1);
	assert_eq!(sent[0]["reason"], "switch");
	assert!(!crate::plugins::active(&id));
}

#[tokio::test]
async fn what_the_plugin_answers_to_session_end_is_booked_and_saved_with_the_session() {
	let id = unique("life-end-saved");
	let dir = tempfile::tempdir().unwrap();
	let stub = StubPlugin::new(&[event::SESSION_END]).answer(json!({
		"usage": [{"label": "learn", "calls": 1, "input_tokens": 5, "output_tokens": 1, "cost": 0.25}],
	}));
	connect_stub(&id, ROLE, dir.path(), &stub).await;
	let file = tempfile::Builder::new().suffix(".jsonl.zst").tempfile().unwrap();
	let mut chat = chat(&id);
	chat.session.session_file = Some(file.path().to_path_buf());

	session_ended(&mut chat, REASON_EXIT).await;

	let resumed = crate::session::load_session(&file.path().to_path_buf()).unwrap();
	let learn = &resumed.info.supervisor_accounting.plugin_usage["learn"];
	assert_eq!((learn.calls, learn.cost), (1, 0.25));
	assert_eq!(resumed.info.total_cost, 0.25);
}

#[tokio::test]
async fn usage_a_child_plugin_reports_when_it_ends_reaches_the_parents_books() {
	let parent = unique("life-usage-parent");
	let child = unique("life-usage-child");
	let dir = tempfile::tempdir().unwrap();
	let stub =
		StubPlugin::new(&[event::SUBAGENT_START, event::SESSION_START, event::SESSION_END]).respond(|method, _| {
			Ok(if method == event::SESSION_END {
				json!({"usage": [{"label": "learn", "calls": 1, "input_tokens": 5, "output_tokens": 1, "cost": 0.5}]})
			} else {
				json!({})
			})
		});
	connect_stub(&parent, ROLE, dir.path(), &stub).await;
	let config = config_for(&stub);

	crate::session::context::with_session_id(parent.clone(), async {
		crate::mcp::set_session_working_directory(dir.path().to_path_buf());
		let running = start_in_process_child(&parent, &child, "find it", "You search.", &config)
			.await
			.expect("a plugin is on for the parent");
		end_in_process_child(running).await;
	})
	.await;

	let usage = crate::plugins::take_usage(&parent);
	assert_eq!(usage.len(), 1);
	assert_eq!(
		(usage[0].0.as_str(), usage[0].1.label.as_str(), usage[0].1.cost),
		(stub.name(), "learn", 0.5)
	);
	assert!(crate::plugins::take_usage(&child).is_empty());
	crate::plugins::end_session(&parent);
}

#[tokio::test]
async fn a_subagent_is_announced_before_its_session_starts_and_inherits_the_answer() {
	let parent = unique("life-parent");
	let child = unique("life-child");
	let dir = tempfile::tempdir().unwrap();
	let stub = StubPlugin::new(&[
		event::SUBAGENT_START,
		event::SUBAGENT_END,
		event::SESSION_START,
		event::SESSION_END,
	])
	.respond(|method, _| {
		Ok(if method == event::SUBAGENT_START {
			json!({"inherit": {"lesson": 1}})
		} else {
			json!({})
		})
	});
	connect_stub(&parent, ROLE, dir.path(), &stub).await;
	let config = config_for(&stub);

	crate::session::context::with_session_id(parent.clone(), async {
		crate::mcp::set_session_working_directory(dir.path().to_path_buf());
		let running = start_in_process_child(&parent, &child, "find it", "You search.", &config)
			.await
			.expect("a plugin is on for the parent");
		assert!(crate::plugins::active(&child));
		assert_eq!(parent_session_id(&child).as_deref(), Some(parent.as_str()));
		end_in_process_child(running).await;
	})
	.await;

	let order: Vec<String> = methods(&stub)
		.into_iter()
		.filter(|method| method != crate::plugins::protocol::METHOD_INITIALIZE)
		.collect();
	assert_eq!(
		order,
		vec![
			event::SUBAGENT_START,
			event::SESSION_START,
			event::SESSION_END,
			event::SUBAGENT_END
		]
	);

	let announced = &stub.sent(event::SUBAGENT_START)[0];
	assert_eq!(announced["session_id"], parent.as_str());
	assert_eq!(announced["child_session_id"], child.as_str());
	assert_eq!(announced["prompt"], "find it");
	assert_eq!(announced["system_prompt"], "You search.");

	let started = &stub.sent(event::SESSION_START)[0];
	assert_eq!(started["session_id"], child.as_str());
	assert_eq!(started["parent_session_id"], parent.as_str());
	assert_eq!(started["inherited"], json!({"lesson": 1}));
	assert_eq!(started["system_prompt"], "You search.");

	assert_eq!(stub.sent(event::SESSION_END)[0]["session_id"], child.as_str());
	assert_eq!(stub.sent(event::SUBAGENT_END)[0]["child_session_id"], child.as_str());
	assert!(!crate::plugins::active(&child));
	assert_eq!(parent_session_id(&child), None);
	crate::plugins::end_session(&parent);
}

#[tokio::test]
async fn no_subagent_event_is_sent_when_no_plugin_is_on_for_the_parent() {
	let parent = unique("life-quiet-parent");
	let stub = StubPlugin::new(&[event::SUBAGENT_START]);
	let config = config_for(&stub);

	let running = start_in_process_child(&parent, "child", "task", "system", &config).await;

	assert!(running.is_none());
	assert!(stub.sent(event::SUBAGENT_START).is_empty());
}

#[test]
fn the_parent_and_the_inherited_values_travel_in_the_meta_of_session_new() {
	let mut params = json!({"cwd": "/work", "mcpServers": [], "_meta": {"octomind.authorization": {"kept": true}}});

	write_child_meta(
		&mut params,
		"parent-1",
		&[("besydr".to_string(), json!({"lessons": [1]}))],
	);

	let meta = params["_meta"].as_object().unwrap();
	assert_eq!(meta["octomind.authorization"], json!({"kept": true}));
	let (parent, inherited) = parentage_from_meta(meta).expect("the parent is named");
	assert_eq!(parent, "parent-1");
	assert_eq!(inherited, vec![("besydr".to_string(), json!({"lessons": [1]}))]);
}

#[test]
fn a_meta_without_a_parent_names_no_parentage() {
	let meta = json!({"octomind.plugin.besydr": 1});

	assert!(parentage_from_meta(meta.as_object().unwrap()).is_none());
}

#[test]
fn a_child_session_start_takes_the_inherited_values_once() {
	let id = unique("life-take");
	set_parentage(&id, "parent-2", vec![("a".to_string(), json!(1))]);

	assert_eq!(take_inherited(&id), vec![("a".to_string(), json!(1))]);
	assert!(take_inherited(&id).is_empty());
	assert_eq!(parent_session_id(&id).as_deref(), Some("parent-2"));
	parentage().lock().remove(&id);
}

#[test]
fn only_a_filled_instruction_file_is_listed() {
	let dir = tempfile::tempdir().unwrap();
	assert!(context_files(dir.path()).is_empty());

	std::fs::write(dir.path().join(AGENTS_FILE), "  \n").unwrap();
	assert!(context_files(dir.path()).is_empty());

	std::fs::write(dir.path().join(AGENTS_FILE), "Rules.").unwrap();
	assert_eq!(
		context_files(dir.path()),
		vec![dir.path().join(AGENTS_FILE).to_string_lossy().into_owned()]
	);
}
