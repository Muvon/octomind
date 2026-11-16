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

//! Rules a plugin registers: compiled into the session's rule set, replaced by
//! the next registration, and reported back to the plugin as matches. A stub
//! plugin registers them through the process-wide host, as a real one would.

use super::*;
use crate::mcp::McpToolCall;
use crate::plugins::protocol::{event, RequestBefore, RequestPhase, TurnKind};
use crate::plugins::test_support::{connect_stub, StubPlugin};
use crate::plugins::EventContext;
use serde_json::json;
use std::path::Path;

const SERVER: &str = "plugin-rules-test";
const WRITE_TOOL: &str = "plug_write";
const WRITE_CAPABILITY: &str = "write";

pub(crate) fn register(rules: &str) -> Value {
	json!({"register": {"rules": rules}})
}

/// A stub plugin that answers every `request.before` with `answer`.
pub(crate) fn stub_answering(answer: Value) -> StubPlugin {
	StubPlugin::new(&[event::REQUEST_BEFORE]).answer(answer)
}

/// A session with a stub plugin switched on, removed again when dropped.
pub(crate) struct PluginSession {
	pub sid: String,
	pub stub: StubPlugin,
	dir: tempfile::TempDir,
}

impl PluginSession {
	pub(crate) async fn start(label: &str, stub: StubPlugin) -> Self {
		let sid = format!("plugin-rules-{label}");
		let dir = tempfile::tempdir().expect("temp workdir");
		crate::session::context::set_session_workdir(&sid, dir.path().to_path_buf());
		connect_stub(&sid, "developer", dir.path(), &stub).await;
		Self { sid, stub, dir }
	}

	pub(crate) fn workdir(&self) -> &Path {
		self.dir.path()
	}

	/// Send the plugin one event and apply what it answered, as the agent
	/// loop does. Returns what the plugin was sent.
	pub(crate) async fn send_event(&self) -> Value {
		let ctx = EventContext {
			session_id: &self.sid,
			parent_session_id: None,
			role: "developer",
			workdir: self.dir.path(),
			project: "proj",
			messages: &[],
			detached: false,
			status: None,
			plan: None,
			tools: None,
			active_skills: Vec::new(),
		};
		let payload = RequestBefore {
			phase: RequestPhase::TurnStart,
			turn: TurnKind::User,
			free_context_tokens: None,
			pending_async: false,
		};
		crate::plugins::request_before(&ctx, payload).await;
		crate::plugins::apply::settle(&self.sid, None);
		self.stub
			.sent(event::REQUEST_BEFORE)
			.pop()
			.expect("the plugin was sent the event")
	}

	/// The matches the plugin hears of with its next event.
	pub(crate) async fn reported_matches(&self) -> Value {
		self.send_event().await["rule_matches"].clone()
	}
}

impl Drop for PluginSession {
	fn drop(&mut self) {
		crate::plugins::end_session(&self.sid);
		clear_for_session(&self.sid);
		crate::mcp::runtime::skill_auto::clear_pool_for_session(&self.sid);
	}
}

fn guard(id: &str, mode: &str, message: &str) -> String {
	format!("[[guard]]\nid = \"{id}\"\nmode = \"{mode}\"\nmatch = \"{WRITE_CAPABILITY}\"\nmessage = \"{message}\"\n")
}

fn default_config() -> Config {
	toml::from_str(include_str!("../../config-templates/default.toml")).expect("default config parses")
}

fn rule_ids(sid: &str) -> Vec<String> {
	get_rules(&sid.to_string())
		.map(|rules| rules.bindings().map(|binding| binding.id.clone()).collect())
		.unwrap_or_default()
}

/// A tool whose capability is `write`, so a guard can match it.
async fn write_call(sid: &str) -> (Config, Vec<McpToolCall>) {
	let mut config = default_config();
	let server = crate::config::McpServerConfig::builtin(SERVER, 30, vec![]);
	config.mcp.servers = vec![server.clone()];
	crate::mcp::tool_map::initialize_tool_map(&config).await.unwrap();
	crate::mcp::tool_map::register_dynamic_server_tools(SERVER, &server, &[WRITE_TOOL.to_string()]);
	CAP_LOOKUP.write().unwrap().get_or_insert_with(HashMap::new).insert(
		sid.to_string(),
		Arc::new(CapLookup {
			exact: HashMap::from([((SERVER.into(), WRITE_TOOL.into()), WRITE_CAPABILITY.into())]),
			..Default::default()
		}),
	);
	let calls = vec![McpToolCall {
		tool_name: WRITE_TOOL.into(),
		tool_id: "1".into(),
		parameters: json!({}),
	}];
	(config, calls)
}

fn forget_write_tool() {
	crate::mcp::tool_map::unregister_dynamic_server_tools(SERVER, &[WRITE_TOOL.to_string()]);
}

#[tokio::test]
#[serial_test::serial]
async fn an_apply_guard_denies_the_call_and_is_reported_as_applied() {
	let stub = stub_answering(register(&guard("no-write", "apply", "plugin says no")));
	let session = PluginSession::start("guard-apply", stub).await;
	session.send_event().await;
	let (config, calls) = write_call(&session.sid).await;

	let denied = check_batch_admitted(&session.sid, &config, &calls, &[]);
	assert_eq!(denied, vec![Some("plugin says no".to_string())]);
	assert_eq!(
		plugin_of_rule(&session.sid, "no-write").as_deref(),
		Some(session.stub.name())
	);
	assert_eq!(
		session.reported_matches().await,
		json!([{"id": "no-write", "outcome": "applied"}])
	);
	forget_write_tool();
}

#[tokio::test]
#[serial_test::serial]
async fn an_observe_guard_does_not_deny_and_is_reported_as_observed() {
	let stub = stub_answering(register(&guard("watch-write", "observe", "plugin says no")));
	let session = PluginSession::start("guard-observe", stub).await;
	session.send_event().await;
	let (config, calls) = write_call(&session.sid).await;

	let preview = preview_batch(&session.sid, &config, &calls);
	assert_eq!(preview.blocked, vec![None]);
	assert_eq!(preview.generated, vec![Vec::<(String, String)>::new()]);
	assert_eq!(check_batch_admitted(&session.sid, &config, &calls, &[]), vec![None]);
	assert_eq!(
		session.reported_matches().await,
		json!([{"id": "watch-write", "outcome": "observed"}])
	);
	forget_write_tool();
}

#[tokio::test]
async fn a_second_registration_replaces_the_first() {
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE]).queue([
		register(&guard("first", "apply", "one")),
		register(&format!(
			"{}{}",
			guard("second", "apply", "two"),
			guard("third", "observe", "three")
		)),
	]);
	let session = PluginSession::start("replace", stub).await;

	session.send_event().await;
	assert_eq!(rule_ids(&session.sid), vec!["first"]);
	session.send_event().await;
	assert_eq!(rule_ids(&session.sid), vec!["second", "third"]);
	assert_eq!(plugin_of_rule(&session.sid, "first"), None);
	// An event that registers nothing leaves the registration as it is.
	session.send_event().await;
	assert_eq!(rule_ids(&session.sid), vec!["second", "third"]);
}

#[tokio::test]
async fn a_registration_without_rules_removes_the_rules_of_the_one_before() {
	let stub = StubPlugin::new(&[event::REQUEST_BEFORE])
		.queue([register(&guard("first", "apply", "one")), json!({"register": {}})]);
	let session = PluginSession::start("unregister", stub).await;

	session.send_event().await;
	assert_eq!(rule_ids(&session.sid), vec!["first"]);
	session.send_event().await;
	assert!(rule_ids(&session.sid).is_empty());
}

#[tokio::test]
async fn text_that_is_not_toml_is_ignored_and_bad_entries_are_skipped() {
	let invalid = PluginSession::start("invalid", stub_answering(register("[[guard"))).await;
	invalid.send_event().await;
	assert!(rule_ids(&invalid.sid).is_empty());

	let entries = format!(
		"[[guard]]\nmode = \"apply\"\nmatch = \"{WRITE_CAPABILITY}\"\nmessage = \"no id\"\n\n\
		 [[guard]]\nid = \"odd\"\nmode = \"sometimes\"\nmatch = \"{WRITE_CAPABILITY}\"\nmessage = \"odd mode\"\n\n\
		 [[guard]]\nid = \"broken\"\nmode = \"apply\"\nmatch = \"write(\"\nmessage = \"bad pattern\"\n\n{}",
		guard("fine", "apply", "ok")
	);
	let mixed = PluginSession::start("mixed", stub_answering(register(&entries))).await;
	mixed.send_event().await;
	assert_eq!(rule_ids(&mixed.sid), vec!["fine"]);
}

#[tokio::test]
async fn a_registered_pipe_is_dropped_when_the_user_has_a_pipe_of_their_own() {
	let rules = format!(
		"[[pipe]]\nid = \"p\"\nmode = \"apply\"\nname = \"p\"\ncommand = \"p.sh\"\n\n{}",
		guard("g", "apply", "no")
	);
	let session = PluginSession::start("pipe-filter", stub_answering(register(&rules))).await;
	std::fs::create_dir_all(session.workdir().join(".agents")).unwrap();
	std::fs::write(
		session.workdir().join(crate::config::guardrails::FILE_PATH),
		"[[pipe]]\nname = \"mine\"\ncommand = \"mine.sh\"\n",
	)
	.unwrap();
	crate::session::context::with_session_id(session.sid.clone(), async { init_for_session() }).await;

	session.send_event().await;
	let rules = get_rules(&session.sid).unwrap();
	assert_eq!(rules.pipes.len(), 1);
	assert_eq!(rules.pipes[0].name, "mine");
	assert_eq!(rule_ids(&session.sid), vec!["g"]);
}

#[tokio::test]
async fn rules_registered_before_the_session_rules_load_are_compiled_again() {
	let session = PluginSession::start("reload", stub_answering(register(&guard("g", "apply", "no")))).await;
	session.send_event().await;
	assert_eq!(rule_ids(&session.sid), vec!["g"]);

	crate::session::context::with_session_id(session.sid.clone(), async { init_for_session() }).await;
	assert_eq!(rule_ids(&session.sid), vec!["g"]);
}

#[test]
fn without_a_plugin_nothing_is_compiled() {
	let sid = "plugin-rules-no-plugin".to_string();
	sync_plugin_rules(&sid);
	assert!(get_rules(&sid).is_none());
	assert_eq!(plugin_of_rule(&sid, "anything"), None);
}

// Scripts of pipes, hooks and validators run only in apply mode.

#[cfg(unix)]
fn write_script(dir: &Path, rel: &str, body: &str) {
	// Written by a child so concurrent test subprocesses cannot inherit a
	// writable script fd and make the later spawn fail with ETXTBSY.
	let status = std::process::Command::new("/bin/sh")
		.args(["-c", "printf '%s' \"$2\" > \"$1\" && chmod 755 \"$1\"", "write-script"])
		.arg(dir.join(rel))
		.arg(format!("#!/bin/sh\n{body}\n"))
		.status()
		.expect("spawn script writer");
	assert!(status.success());
}

#[cfg(unix)]
const MODES: [(&str, &str); 2] = [("apply", "applied"), ("observe", "observed")];

#[cfg(unix)]
#[tokio::test]
async fn a_plugin_pipe_runs_in_apply_mode_and_only_reports_in_observe_mode() {
	for (mode, outcome) in MODES {
		let rules = format!("[[pipe]]\nid = \"shout\"\nmode = \"{mode}\"\nname = \"shout\"\ncommand = \"shout.sh\"\n");
		let session = PluginSession::start(&format!("pipe-{mode}"), stub_answering(register(&rules))).await;
		write_script(session.workdir(), "shout.sh", "tr a-z A-Z");
		session.send_event().await;

		let output = crate::session::pipe::run_pipe(&session.sid, "developer", "hello", false)
			.await
			.expect("the pipe does not fail");
		assert_eq!(output, (mode == "apply").then(|| "HELLO".to_string()), "{mode}");
		assert_eq!(
			session.reported_matches().await,
			json!([{"id": "shout", "outcome": outcome}]),
			"{mode}"
		);
	}
}

#[cfg(unix)]
#[tokio::test]
async fn a_plugin_hook_runs_in_apply_mode_and_only_reports_in_observe_mode() {
	for (mode, outcome) in MODES {
		let rules = format!("[[hook]]\nid = \"nag\"\nmode = \"{mode}\"\nscript = \"nag.sh\"\n");
		let session = PluginSession::start(&format!("hook-{mode}"), stub_answering(register(&rules))).await;
		write_script(session.workdir(), "nag.sh", "echo HOOK-FIRED\nexit 1");
		session.send_event().await;

		let sid = session.sid.clone();
		let fired = crate::session::context::with_session_id(sid.clone(), async {
			crate::session::inbox::init_inbox_for_session();
			let call = McpToolCall {
				tool_name: "shell".into(),
				tool_id: "t".into(),
				parameters: json!({"cmd": "make"}),
			};
			let result = crate::mcp::McpToolResult::success("shell".into(), "t".into(), "ok".into());
			crate::session::hooks::run_hooks(&sid, &default_config(), &[call], &[result], &[false]).await;
			let message = crate::session::inbox::try_pop_inbox_message();
			crate::session::inbox::clear_inbox_for_session(&sid);
			message.is_some()
		})
		.await;
		assert_eq!(fired, mode == "apply", "{mode}");
		assert_eq!(
			session.reported_matches().await,
			json!([{"id": "nag", "outcome": outcome}]),
			"{mode}"
		);
	}
}

#[cfg(unix)]
#[tokio::test]
async fn a_plugin_validator_runs_in_apply_mode_and_only_reports_in_observe_mode() {
	for (mode, outcome) in MODES {
		// No `match`: an observe-only validator must stay quiet without one too.
		let rules =
			format!("[[validator]]\nid = \"check\"\nmode = \"{mode}\"\nname = \"check\"\nscript = \"check.sh\"\n");
		let session = PluginSession::start(&format!("validator-{mode}"), stub_answering(register(&rules))).await;
		write_script(session.workdir(), "check.sh", "echo VALIDATOR-FIRED\nexit 1");
		session.send_event().await;

		let sid = session.sid.clone();
		let fired = crate::session::context::with_session_id(sid.clone(), async {
			crate::session::inbox::init_inbox_for_session();
			crate::session::hooks::run_turn_validators(&sid, "developer", "shipped").await;
			let message = crate::session::inbox::try_pop_inbox_message();
			crate::session::inbox::clear_inbox_for_session(&sid);
			message.is_some()
		})
		.await;
		assert_eq!(fired, mode == "apply", "{mode}");
		assert_eq!(
			session.reported_matches().await,
			json!([{"id": "check", "outcome": outcome}]),
			"{mode}"
		);
	}
}
