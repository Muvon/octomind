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

//! Host behaviour against stub plugins that live in memory.

use super::*;
use crate::plugins::protocol::{
	BackgroundOutput, Command, CompactBefore, Judge, PromptSubmit, RequestBefore, RequestPhase, SessionStart,
	SubagentStart, ToolAfter, ToolBefore, ToolCall, ToolOutcome, TurnEnd, TurnKind,
};
use crate::plugins::state::{allowed_effects, FIRST_PAUSE, SESSION_END_WAIT};
use crate::plugins::test_support::StubPlugin as FakePlugin;
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakeConnector {
	plugins: HashMap<String, FakePlugin>,
	connects: Arc<AtomicUsize>,
}

#[async_trait]
impl Connector for FakeConnector {
	async fn connect(&self, config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		self.connects.fetch_add(1, Ordering::SeqCst);
		match self.plugins.get(&config.name) {
			Some(plugin) => {
				plugin.reopen();
				Ok(Arc::new(plugin.clone()))
			}
			None => bail!("nothing to connect to for {}", config.name),
		}
	}
}

fn config(name: &str) -> PluginConfig {
	toml::from_str(&format!(
		"name = \"{name}\"\ntype = \"stdio\"\ncommand = \"fake\"\ntimeout_seconds = 1\n"
	))
	.unwrap()
}

fn plugin(name: &str, fake: FakePlugin) -> (PluginConfig, FakePlugin) {
	(config(name), fake)
}

/// The sums of a report; a plugin must send every one of them.
fn totals(chat_calls: u64, cost: f64) -> Value {
	json!({"chat_calls": chat_calls, "decision_calls": 0, "wall_blocking_ms": 0, "api_ms": 0,
		"input_tokens": 0, "output_tokens": 0, "reasoning_tokens": 0, "cost": cost})
}

struct Rig {
	host: Host,
	fakes: Vec<FakePlugin>,
	connects: Arc<AtomicUsize>,
	dir: tempfile::TempDir,
}

impl Rig {
	fn fake(&self, index: usize) -> &FakePlugin {
		&self.fakes[index]
	}

	fn ctx<'a>(&'a self, messages: &'a [crate::session::Message]) -> EventContext<'a> {
		EventContext {
			session_id: SESSION,
			parent_session_id: None,
			role: ROLE,
			model: None,
			workdir: self.dir.path(),
			project: "proj",
			messages,
			detached: false,
			status: None,
			plan: None,
			tools: None,
			active_skills: Vec::new(),
		}
	}
}

const SESSION: &str = "s1";
const ROLE: &str = "developer";

/// Start session `s1` with these plugins, in config order.
async fn rig(plugins: Vec<(PluginConfig, FakePlugin)>) -> Rig {
	let dir = tempfile::tempdir().unwrap();
	let connects = Arc::new(AtomicUsize::new(0));
	let mut by_name = HashMap::new();
	let mut configs = Vec::new();
	let mut fakes = Vec::new();
	for (config, fake) in plugins {
		by_name.insert(config.name.clone(), fake.clone());
		fakes.push(fake);
		configs.push(config);
	}
	let host = Host::new(Arc::new(FakeConnector {
		plugins: by_name,
		connects: connects.clone(),
	}));
	host.start_session(SESSION, ROLE, dir.path(), &configs).await;
	Rig {
		host,
		fakes,
		connects,
		dir,
	}
}

fn user(text: &str) -> crate::session::Message {
	crate::session::Message {
		role: "user".into(),
		content: text.into(),
		..Default::default()
	}
}

fn request_before() -> RequestBefore {
	RequestBefore {
		phase: RequestPhase::TurnStart,
		turn: TurnKind::User,
		free_context_tokens: None,
		pending_async: false,
	}
}

fn prompt(text: &str) -> PromptSubmit {
	PromptSubmit {
		prompt: text.into(),
		original: None,
		carried_summary: None,
		goal: None,
	}
}

fn tool_before(call_id: &str) -> ToolBefore {
	ToolBefore {
		calls: vec![ToolCall {
			id: call_id.into(),
			tool: "shell".into(),
			server: "core".into(),
			capability: None,
			arguments: json!({}),
			denied: None,
			rules: Vec::new(),
			script_call_id: None,
		}],
	}
}

fn outcome(call_id: &str, result: &str, size_tokens: usize) -> ToolOutcome {
	ToolOutcome {
		id: call_id.into(),
		tool: "shell".into(),
		server: "core".into(),
		arguments: json!({}),
		error: false,
		denied: false,
		result: Some(result.into()),
		result_hash: "h".into(),
		size_tokens,
		size_bytes: result.len(),
		free_form: false,
		mutation: false,
		command: None,
		paths: Vec::new(),
		replaceable: true,
		returned: None,
		script_calls: Vec::new(),
	}
}

fn tool_after(calls: Vec<ToolOutcome>) -> ToolAfter {
	ToolAfter {
		intent: None,
		calls,
		tree: None,
		subagents: None,
		background: Vec::new(),
		stopping: None,
		context_files: Vec::new(),
	}
}

fn turn_end() -> TurnEnd {
	TurnEnd {
		turn: TurnKind::User,
		answers: vec!["done".into()],
		pending_async: false,
		changes: Vec::new(),
		tree_status: Vec::new(),
		tree: None,
		requests: 1,
		stopped: None,
		background: Vec::new(),
	}
}

fn session_start() -> SessionStart {
	SessionStart {
		source: "startup".into(),
		system_prompt: "system".into(),
		interactive: false,
		can_read_saved_output: false,
		inherited: None,
		capabilities: Vec::new(),
		context_files: Vec::new(),
	}
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handshake_sends_the_protocol_the_host_and_the_effect_table() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::TURN_END]))]).await;
	let init = rig.fake(0).sent(METHOD_INITIALIZE);
	assert_eq!(init.len(), 1);
	assert_eq!(init[0]["protocol"], PROTOCOL_VERSION);
	assert_eq!(init[0]["host"]["name"], "octomind");
	assert_eq!(init[0]["events"]["tool.before"], json!(allowed_effects("tool.before")));
	assert_eq!(init[0]["events"]["compact.after"], json!([]));
	assert_eq!(init[0]["events"]["session.end"], json!(allowed_effects("session.end")));
	assert!(rig.host.active(SESSION));
	assert!(rig.host.subscribed(SESSION, event::TURN_END));
	assert!(!rig.host.subscribed(SESSION, event::TOOL_BEFORE));
	assert!(rig.host.take_notices(SESSION).is_empty());
}

#[tokio::test]
async fn handshake_keeps_options_status_fields_and_commands() {
	let fake = FakePlugin::new(&[event::COMMAND])
		.init_field(
			"status_fields",
			json!([{"name": "relied_on", "instruction": "ids you used", "default": "x", "example": "y"}]),
		)
		.init_field(
			"commands",
			json!([{"name": "learn", "hint": "[x]", "description": "Learn"}]),
		);
	let rig = rig(vec![plugin("a", fake)]).await;
	assert_eq!(
		rig.host.status_fields(SESSION),
		vec![PluginStatusField {
			plugin: "a".into(),
			name: "relied_on".into(),
			instruction: "ids you used".into(),
			default: json!("x"),
			example: Some(json!("y")),
		}]
	);
	let commands = rig.host.commands(SESSION);
	assert_eq!(commands.len(), 1);
	assert_eq!(
		(commands[0].plugin.as_str(), commands[0].spec.name.as_str()),
		("a", "learn")
	);
}

#[tokio::test]
async fn another_protocol_number_turns_the_plugin_off_with_one_notice() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).init_field("protocol", json!(PROTOCOL_VERSION + 1));
	let rig = rig(vec![plugin("a", fake)]).await;
	assert!(!rig.host.active(SESSION));
	let notices = rig.host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(
		notices[0].contains("'a'") && notices[0].contains("protocol"),
		"{}",
		notices[0]
	);
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert!(rig.fake(0).sent(event::REQUEST_BEFORE).is_empty());
}

#[tokio::test]
async fn a_plugin_that_cannot_connect_is_off_with_one_notice() {
	let dir = tempfile::tempdir().unwrap();
	let host = Host::new(Arc::new(FakeConnector {
		plugins: HashMap::new(),
		connects: Arc::default(),
	}));
	host.start_session(SESSION, ROLE, dir.path(), &[config("ghost")]).await;
	assert!(!host.active(SESSION));
	let notices = host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(notices[0].contains("ghost"));
}

#[tokio::test]
async fn disabled_plugins_and_other_roles_are_not_connected() {
	let dir = tempfile::tempdir().unwrap();
	let mut disabled = config("off");
	disabled.enabled = false;
	let mut other_role = config("other");
	other_role.roles = vec!["assistant".into()];
	let mut same_domain = config("domain");
	same_domain.roles = vec!["developer".into()];
	let fakes: HashMap<String, FakePlugin> = ["off", "other", "domain"]
		.into_iter()
		.map(|name| (name.to_string(), FakePlugin::new(&[event::TURN_END])))
		.collect();
	let connects = Arc::new(AtomicUsize::new(0));
	let host = Host::new(Arc::new(FakeConnector {
		plugins: fakes.clone(),
		connects: connects.clone(),
	}));
	host.start_session(
		SESSION,
		"developer:general",
		dir.path(),
		&[disabled, other_role, same_domain],
	)
	.await;
	assert_eq!(connects.load(Ordering::SeqCst), 1);
	assert!(fakes["off"].sent(METHOD_INITIALIZE).is_empty());
	assert!(fakes["other"].sent(METHOD_INITIALIZE).is_empty());
	assert_eq!(fakes["domain"].sent(METHOD_INITIALIZE).len(), 1);
}

#[tokio::test]
async fn sessions_share_one_connection_per_plugin() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::TURN_END]))]).await;
	rig.host.start_session("s2", ROLE, rig.dir.path(), &[config("a")]).await;
	assert_eq!(rig.connects.load(Ordering::SeqCst), 1);
	assert_eq!(rig.fake(0).sent(METHOD_INITIALIZE).len(), 2);
	rig.host.end_session("s2");
	assert!(rig.host.active(SESSION));
	assert!(!rig.host.active("s2"));
}

#[tokio::test]
async fn a_lost_connection_is_made_again_for_the_next_session() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::TURN_END]))]).await;
	rig.fake(0).disconnect();
	rig.host.start_session("s2", ROLE, rig.dir.path(), &[config("a")]).await;
	assert_eq!(rig.connects.load(Ordering::SeqCst), 2);
}

// ---------------------------------------------------------------------------
// What is sent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_go_only_to_plugins_that_subscribed_in_config_order() {
	let rig = rig(vec![
		plugin("a", FakePlugin::new(&[event::TURN_END])),
		plugin("b", FakePlugin::new(&[event::PROMPT_SUBMIT, event::TURN_END])),
		plugin("c", FakePlugin::new(&[event::PROMPT_SUBMIT])),
	])
	.await;
	rig.host.prompt_submit(&rig.ctx(&[]), prompt("x")).await;
	assert!(rig.fake(0).sent(event::PROMPT_SUBMIT).is_empty());
	assert_eq!(rig.fake(1).sent(event::PROMPT_SUBMIT).len(), 1);
	assert_eq!(rig.fake(2).sent(event::PROMPT_SUBMIT).len(), 1);
	rig.host.turn_end(&rig.ctx(&[]), turn_end()).await;
	assert_eq!(rig.fake(0).sent(event::TURN_END).len(), 1);
	assert_eq!(rig.fake(1).sent(event::TURN_END).len(), 1);
	assert!(rig.fake(2).sent(event::TURN_END).is_empty());
}

#[tokio::test]
async fn common_fields_and_payload_arrive_in_one_object() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::PROMPT_SUBMIT]))]).await;
	let messages = [user("hello")];
	let mut ctx = rig.ctx(&messages);
	ctx.parent_session_id = Some("parent");
	rig.host.prompt_submit(&ctx, prompt("do it")).await;
	let sent = &rig.fake(0).sent(event::PROMPT_SUBMIT)[0];
	assert_eq!(sent["session_id"], SESSION);
	assert_eq!(sent["parent_session_id"], "parent");
	assert_eq!(sent["role"], ROLE);
	assert_eq!(sent["workdir"], rig.dir.path().to_string_lossy().as_ref());
	assert_eq!(sent["prompt"], "do it");
	assert_eq!(sent["messages"][0]["content"], "hello");
	assert_eq!(sent["messages"][0]["managed"], false);
}

#[tokio::test]
async fn messages_are_a_delta_and_then_a_reset() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	let mut messages = vec![user("one"), user("two")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.push(user("three"));
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	rig.host.mark_history_reset(SESSION);
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.truncate(1);
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;

	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	let lengths: Vec<usize> = sent
		.iter()
		.map(|params| params["messages"].as_array().unwrap().len())
		.collect();
	let resets: Vec<bool> = sent
		.iter()
		.map(|params| params["messages_reset"].as_bool().unwrap())
		.collect();
	assert_eq!(lengths, vec![2, 1, 3, 0, 1]);
	assert_eq!(resets, vec![false, false, true, false, true]);
	assert_eq!(sent[1]["messages"][0]["content"], "three");
}

#[tokio::test]
async fn each_plugin_has_its_own_message_cursor() {
	let rig = rig(vec![
		plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE, event::TURN_END])),
		plugin("b", FakePlugin::new(&[event::REQUEST_BEFORE])),
	])
	.await;
	let mut messages = vec![user("one")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.push(user("two"));
	rig.host.turn_end(&rig.ctx(&messages), turn_end()).await;
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	let count = |index: usize| -> Vec<usize> {
		rig.fake(index)
			.sent(event::REQUEST_BEFORE)
			.iter()
			.map(|params| params["messages"].as_array().unwrap().len())
			.collect()
	};
	assert_eq!(count(0), vec![1, 0]);
	assert_eq!(count(1), vec![1, 1]);
}

#[tokio::test]
async fn the_status_plan_and_git_remote_of_the_event_are_sent() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	let mut ctx = rig.ctx(&[]);
	ctx.status = Some(protocol::Status {
		state: "done".into(),
		..Default::default()
	});
	rig.host.request_before(&ctx, request_before()).await;
	let sent = &rig.fake(0).sent(event::REQUEST_BEFORE)[0];
	assert_eq!(sent["status"]["state"], "done");
	// The temporary directory is no git repository, so there is no remote to send.
	assert!(sent.get("git_remote").is_none());
}

#[test]
fn a_login_stored_in_the_git_remote_is_not_sent() {
	let stripped = |remote: &str| strip_remote_credentials(remote).expect("remote kept");
	assert_eq!(
		stripped("https://user:ghp_secret@github.com/o/r.git"),
		"https://github.com/o/r.git"
	);
	// A token alone sits in the user part.
	assert_eq!(
		stripped("https://ghp_secret@github.com/o/r.git"),
		"https://github.com/o/r.git"
	);
	// Over ssh the user names the account; only a password is a secret.
	assert_eq!(
		stripped("ssh://git:pw@github.com/o/r.git"),
		"ssh://git@github.com/o/r.git"
	);
	for untouched in [
		"https://github.com/o/r.git",
		"ssh://git@github.com/o/r.git",
		"git@github.com:o/r.git",
	] {
		assert_eq!(stripped(untouched), untouched);
	}
}

#[tokio::test]
async fn rule_matches_are_drained_into_the_next_event() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	rig.host.record_rule_match(SESSION, "a", "r1", RuleOutcome::Applied);
	rig.host.record_rule_match(SESSION, "a", "r2", RuleOutcome::Observed);
	rig.host
		.record_rule_match(SESSION, "nobody", "r3", RuleOutcome::Applied);
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(
		sent[0]["rule_matches"],
		json!([{"id": "r1", "outcome": "applied"}, {"id": "r2", "outcome": "observed"}])
	);
	assert_eq!(sent[1]["rule_matches"], json!([]));
}

#[tokio::test]
async fn tool_after_results_below_min_output_tokens_arrive_without_text() {
	let small = FakePlugin::new(&[event::TOOL_AFTER]).init_field("options", json!({"min_output_tokens": 100}));
	let rig = rig(vec![
		plugin("picky", small),
		plugin("all", FakePlugin::new(&[event::TOOL_AFTER])),
	])
	.await;
	let payload = tool_after(vec![outcome("c1", "tiny", 5), outcome("c2", "large", 500)]);
	rig.host.tool_after(&rig.ctx(&[]), payload).await;
	let picky = &rig.fake(0).sent(event::TOOL_AFTER)[0];
	assert!(picky["calls"][0].get("result").is_none());
	assert_eq!(picky["calls"][0]["result_hash"], "h");
	assert_eq!(picky["calls"][1]["result"], "large");
	let all = &rig.fake(1).sent(event::TOOL_AFTER)[0];
	assert_eq!(all["calls"][0]["result"], "tiny");
}

#[tokio::test]
async fn result_chars_cut_tool_messages_for_that_plugin_only() {
	let cutting = FakePlugin::new(&[event::REQUEST_BEFORE]).init_field("options", json!({"result_chars": 4}));
	let rig = rig(vec![
		plugin("cutting", cutting),
		plugin("full", FakePlugin::new(&[event::REQUEST_BEFORE])),
	])
	.await;
	let tool = crate::session::Message {
		role: "tool".into(),
		content: "abcdefgh".into(),
		..Default::default()
	};
	rig.host.request_before(&rig.ctx(&[tool]), request_before()).await;
	assert_eq!(
		rig.fake(0).sent(event::REQUEST_BEFORE)[0]["messages"][0]["content"],
		"abcd"
	);
	assert_eq!(
		rig.fake(1).sent(event::REQUEST_BEFORE)[0]["messages"][0]["content"],
		"abcdefgh"
	);
}

// ---------------------------------------------------------------------------
// Effects the host applies itself
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_pin_usage_notice_and_register_are_kept() {
	let calls = AtomicUsize::new(0);
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).respond(move |_, _| {
		Ok(match calls.fetch_add(1, Ordering::SeqCst) {
			0 => json!({
				"context": [{"name": "c1", "text": "ctx one"}, {"name": "c2", "text": "ctx two"}],
				"pin": [{"name": "p1", "text": "pinned"}],
				"usage": [{"label": "judge", "calls": 2, "input_tokens": 10, "output_tokens": 3, "cost": 0.5}],
				"notice": ["first notice", "second notice"],
				"register": {
					"rules": "[[guard]]\nid = \"r1\"\n",
					"skill_dirs": [{"id": "s1", "path": "/skills/one", "mode": "apply"}]
				},
				"note": ["note one"],
			}),
			1 => json!({
				"context": [{"name": "c1"}],
				"pin": [{"name": "p1", "text": "pinned again"}, {"name": "p2", "text": "second"}],
				"register": {"rules": "[[guard]]\nid = \"r2\"\n"},
			}),
			_ => json!({}),
		})
	});
	let rig = rig(vec![plugin("a", fake)]).await;

	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(effects.notes, vec!["note one".to_string()]);
	let block = |name: &str, text: &str| PluginBlock {
		plugin: "a".into(),
		name: name.into(),
		text: text.into(),
		items: Vec::new(),
	};
	assert_eq!(
		rig.host.context_blocks(SESSION),
		vec![block("c1", "ctx one"), block("c2", "ctx two")]
	);
	assert_eq!(rig.host.pinned_blocks(SESSION), vec![block("p1", "pinned")]);
	let usage = rig.host.take_usage(SESSION);
	assert_eq!(usage.len(), 1);
	assert_eq!((usage[0].0.as_str(), usage[0].1.calls, usage[0].1.cost), ("a", 2, 0.5));
	assert!(rig.host.take_usage(SESSION).is_empty());
	assert_eq!(
		rig.host.take_notices(SESSION),
		vec!["first notice".to_string(), "second notice".to_string()]
	);
	assert!(rig.host.take_notices(SESSION).is_empty());
	assert_eq!(
		rig.host.registered_rules(SESSION),
		vec![("a".to_string(), "[[guard]]\nid = \"r1\"\n".to_string())]
	);
	assert_eq!(rig.host.registered_skill_dirs(SESSION)[0].1.path, "/skills/one");

	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(rig.host.context_blocks(SESSION), vec![block("c2", "ctx two")]);
	assert_eq!(
		rig.host.pinned_blocks(SESSION),
		vec![block("p1", "pinned again"), block("p2", "second")]
	);
	// A later register replaces the earlier one as a whole, skill directories included.
	assert_eq!(rig.host.registered_rules(SESSION)[0].1, "[[guard]]\nid = \"r2\"\n");
	assert!(rig.host.registered_skill_dirs(SESSION).is_empty());
}

#[tokio::test]
async fn a_report_is_kept_and_a_later_report_of_the_plugin_replaces_it() {
	let calls = AtomicUsize::new(0);
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).respond(move |_, _| {
		Ok(match calls.fetch_add(1, Ordering::SeqCst) {
			0 => {
				json!({"report": {"mechanics": {"check": {"chat_calls": 1}}, "totals": totals(1, 0.5)}})
			}
			1 => {
				json!({"report": {"mechanics": {"check": {"chat_calls": 3}}, "totals": totals(3, 0.75)}})
			}
			_ => json!({}),
		})
	});
	let rig = rig(vec![plugin("a", fake)]).await;

	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let first = rig.host.take_reports(SESSION);
	assert_eq!(first.len(), 1);
	assert_eq!(first[0].0, "a");
	assert_eq!(first[0].1.totals.chat_calls, 1);
	assert!(rig.host.take_reports(SESSION).is_empty());

	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let latest = rig.host.take_reports(SESSION);
	assert_eq!(latest.len(), 1);
	assert_eq!(latest[0].1.totals.chat_calls, 3);
	assert_eq!(latest[0].1.mechanics["check"]["chat_calls"], 3);
}

#[tokio::test]
async fn the_second_report_replaces_the_first_when_none_was_taken_between() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).queue([
		json!({"report": {"mechanics": {}, "totals": totals(1, 0.0)}}),
		json!({"report": {"mechanics": {}, "totals": totals(2, 0.0)}}),
	]);
	let rig = rig(vec![plugin("a", fake)]).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let reports = rig.host.take_reports(SESSION);
	assert_eq!(reports.len(), 1);
	assert_eq!(reports[0].1.totals.chat_calls, 2);
}

#[test]
fn a_report_on_a_notification_is_ignored_and_every_answered_event_allows_it() {
	let mut answer: Answer =
		serde_json::from_value(json!({"report": {"mechanics": {}, "totals": totals(0, 0.0)}})).unwrap();
	sanitize("a", event::COMPACT_AFTER, &mut answer);
	assert!(answer.report.is_none());
	for name in event::ALL.iter().filter(|name| !event::NOTIFICATIONS.contains(name)) {
		assert!(allowed_effects(name).contains(&protocol::effect::REPORT), "{name}");
	}
}

#[tokio::test]
async fn history_effect_sends_the_full_list_as_a_notification() {
	let calls = AtomicUsize::new(0);
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).respond(move |_, _| {
		Ok(match calls.fetch_add(1, Ordering::SeqCst) {
			1 => json!({"history": true}),
			_ => json!({}),
		})
	});
	let rig = rig(vec![plugin("a", fake)]).await;
	let mut messages = vec![user("one"), user("two")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.push(user("three"));
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	let history = rig.fake(0).sent(METHOD_SESSION_HISTORY);
	assert_eq!(history.len(), 1);
	assert_eq!(history[0]["messages"].as_array().unwrap().len(), 3);
	assert_eq!(history[0]["messages_reset"], true);
	assert_eq!(history[0]["session_id"], SESSION);
}

#[tokio::test]
async fn pinned_blocks_and_status_fields_are_restored_only_for_connected_plugins() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE])
		.init_field("status_fields", json!([{"name": "kept", "instruction": "fresh"}]));
	let rig = rig(vec![plugin("a", fake)]).await;
	rig.host.restore_pinned_blocks(
		SESSION,
		vec![
			PluginBlock {
				plugin: "a".into(),
				name: "p".into(),
				text: "stored".into(),
				items: vec!["one".into()],
			},
			PluginBlock {
				plugin: "gone".into(),
				name: "p".into(),
				text: "dropped".into(),
				items: Vec::new(),
			},
		],
	);
	rig.host.restore_status_fields(
		SESSION,
		vec![
			PluginStatusField {
				plugin: "a".into(),
				name: "kept".into(),
				instruction: "stale".into(),
				default: Value::Null,
				example: None,
			},
			PluginStatusField {
				plugin: "a".into(),
				name: "extra".into(),
				instruction: "stored".into(),
				default: json!(7),
				example: Some(json!(8)),
			},
			PluginStatusField {
				plugin: "gone".into(),
				name: "x".into(),
				instruction: "dropped".into(),
				default: Value::Null,
				example: None,
			},
		],
	);
	let pinned = rig.host.pinned_blocks(SESSION);
	assert_eq!(pinned.len(), 1);
	assert_eq!((pinned[0].plugin.as_str(), pinned[0].text.as_str()), ("a", "stored"));
	assert_eq!(pinned[0].items, vec!["one".to_string()]);
	let fields = rig.host.status_fields(SESSION);
	let by_name: Vec<(&str, &str)> = fields
		.iter()
		.map(|field| (field.name.as_str(), field.instruction.as_str()))
		.collect();
	assert_eq!(by_name, vec![("kept", "fresh"), ("extra", "stored")]);
	assert_eq!(fields[1].default, json!(7));
	assert_eq!(fields[1].example, Some(json!(8)));
}

// ---------------------------------------------------------------------------
// Effects the caller applies, and how plugins combine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn notes_and_the_first_plan_come_back_to_the_caller() {
	let first = FakePlugin::new(&[event::COMPACT_BEFORE]).answer(json!({
		"note": ["from a"],
		"plan": {"action": "advance", "summary": "first"}
	}));
	let second = FakePlugin::new(&[event::COMPACT_BEFORE]).answer(json!({
		"note": ["from b"],
		"plan": {"action": "finish", "summary": "second"}
	}));
	let rig = rig(vec![plugin("a", first), plugin("b", second)]).await;
	let effects = rig
		.host
		.compact_before(&rig.ctx(&[]), CompactBefore { forced: false })
		.await;
	assert_eq!(effects.notes, vec!["from a".to_string(), "from b".to_string()]);
	assert_eq!(
		effects.plan,
		Some(protocol::PlanUpdate::Advance {
			summary: "first".into()
		})
	);
}

#[tokio::test]
async fn an_effect_the_event_does_not_allow_is_ignored() {
	let overreaching = FakePlugin::new(&[event::REQUEST_BEFORE, event::TOOL_BEFORE, event::COMMAND]).answer(json!({
		"deny": [{"call_id": "c1", "reason": "no"}],
		"notice": ["still heard"],
		"note": ["a note"],
		"plan": {"action": "finish", "summary": "done"}
	}));
	let rig = rig(vec![plugin("a", overreaching)]).await;
	let ctx = rig.ctx(&[]);
	let collect_denies = |denies: &mut Vec<protocol::Deny>, _: &str, answer: &mut Answer| {
		denies.append(&mut answer.deny);
		Flow::Next
	};

	let (denies, effects) = rig
		.host
		.run_event(
			event::REQUEST_BEFORE,
			&ctx,
			None,
			Vec::new(),
			|_, _| request_before(),
			collect_denies,
		)
		.await;
	assert!(denies.is_empty(), "request.before does not allow deny");
	assert_eq!(effects.notes, vec!["a note".to_string()]);

	let (denies, _) = rig
		.host
		.run_event(
			event::TOOL_BEFORE,
			&ctx,
			None,
			Vec::new(),
			|_, _| tool_before("c1"),
			collect_denies,
		)
		.await;
	assert_eq!(denies.len(), 1, "tool.before allows deny");

	// `command` allows neither `note` nor `plan`, but the common effects still count.
	let command = Command {
		name: "x".into(),
		arguments: String::new(),
	};
	let (_, effects) = rig
		.host
		.run_event(
			event::COMMAND,
			&ctx,
			Some("a"),
			(),
			|_, _| &command,
			|_, _, _| Flow::Next,
		)
		.await;
	assert_eq!(effects, Effects::default());
	assert_eq!(rig.host.take_notices(SESSION).len(), 3);
}

#[tokio::test]
async fn deny_is_honoured_on_tool_before_only() {
	let plugin_a = FakePlugin::new(&[event::REQUEST_BEFORE, event::TOOL_BEFORE]).answer(json!({
		"deny": [{"call_id": "c1", "reason": "blocked"}]
	}));
	let rig = rig(vec![plugin("a", plugin_a)]).await;
	let elsewhere = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(elsewhere, Effects::default());
	let tool = rig.host.tool_before(&rig.ctx(&[]), tool_before("c1")).await;
	assert_eq!(tool.denies.len(), 1);
	assert_eq!(tool.denies[0].reason, "blocked");
}

#[tokio::test]
async fn prompts_chain_through_plugins_in_config_order() {
	let chain = |tag: &'static str| {
		FakePlugin::new(&[event::PROMPT_SUBMIT]).respond(move |_, params| {
			let seen = params["prompt"].as_str().unwrap();
			Ok(json!({"replace_prompt": format!("{tag}({seen})")}))
		})
	};
	let rig = rig(vec![plugin("a", chain("A")), plugin("b", chain("B"))]).await;
	let result = rig.host.prompt_submit(&rig.ctx(&[]), prompt("x")).await;
	assert_eq!(result.prompt, "B(A(x))");
	assert!(result.block.is_none());
	assert_eq!(rig.fake(1).sent(event::PROMPT_SUBMIT)[0]["prompt"], "A(x)");
}

#[tokio::test]
async fn the_first_block_wins_and_later_plugins_are_not_asked() {
	let blocker = FakePlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"block": "not now", "note": ["why"]}));
	let rig = rig(vec![
		plugin("a", blocker),
		plugin(
			"b",
			FakePlugin::new(&[event::PROMPT_SUBMIT]).answer(json!({"block": "second"})),
		),
	])
	.await;
	let result = rig.host.prompt_submit(&rig.ctx(&[]), prompt("x")).await;
	assert_eq!(result.block.as_deref(), Some("not now"));
	assert_eq!(result.effects.notes, vec!["why".to_string()]);
	assert!(rig.fake(1).sent(event::PROMPT_SUBMIT).is_empty());
}

#[tokio::test]
async fn the_first_deny_wins_and_later_plugins_are_not_asked() {
	let deny = |reason: &str| json!({"deny": [{"call_id": "c1", "reason": reason}]});
	let rig = rig(vec![
		plugin("quiet", FakePlugin::new(&[event::TOOL_BEFORE])),
		plugin("a", FakePlugin::new(&[event::TOOL_BEFORE]).answer(deny("first"))),
		plugin("b", FakePlugin::new(&[event::TOOL_BEFORE]).answer(deny("second"))),
	])
	.await;
	let result = rig.host.tool_before(&rig.ctx(&[]), tool_before("c1")).await;
	assert_eq!(result.denies.len(), 1);
	assert_eq!(result.denies[0].reason, "first");
	assert!(rig.fake(2).sent(event::TOOL_BEFORE).is_empty());
}

#[tokio::test]
async fn lift_is_honoured_only_for_rules_the_same_plugin_registered() {
	let owner = FakePlugin::new(&[event::REQUEST_BEFORE, event::TOOL_BEFORE]).respond(|method, _| {
		Ok(match method {
			event::REQUEST_BEFORE => json!({"register": {"rules": "[[guard]]\nid = \"mine\"\n"}}),
			_ => json!({"lift": [
				{"call_id": "c1", "rules": ["mine", "theirs"]},
				{"call_id": "c2", "rules": ["theirs"]}
			]}),
		})
	});
	let other = FakePlugin::new(&[event::REQUEST_BEFORE, event::TOOL_BEFORE]).respond(|method, _| {
		Ok(match method {
			event::REQUEST_BEFORE => json!({"register": {"rules": "[[guard]]\nid = \"theirs\"\n"}}),
			_ => json!({"lift": [{"call_id": "c1", "rules": ["mine", "theirs"]}]}),
		})
	});
	let rig = rig(vec![plugin("owner", owner), plugin("other", other)]).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let result = rig.host.tool_before(&rig.ctx(&[]), tool_before("c1")).await;
	assert_eq!(
		result.lifts,
		vec![
			protocol::Lift {
				call_id: "c1".into(),
				rules: vec!["mine".into()]
			},
			protocol::Lift {
				call_id: "c1".into(),
				rules: vec!["theirs".into()]
			},
		]
	);
}

#[tokio::test]
async fn tool_results_chain_and_the_last_text_per_call_is_returned() {
	let first = FakePlugin::new(&[event::TOOL_AFTER]).answer(json!({
		"replace": [{"call_id": "c1", "text": "A", "keep_original": false}]
	}));
	let second = FakePlugin::new(&[event::TOOL_AFTER]).respond(|_, params| {
		let seen = params["calls"][0]["result"].as_str().unwrap();
		Ok(json!({"replace": [{"call_id": "c1", "text": format!("B({seen})"), "keep_original": true}]}))
	});
	let rig = rig(vec![plugin("a", first), plugin("b", second)]).await;
	let result = rig
		.host
		.tool_after(&rig.ctx(&[]), tool_after(vec![outcome("c1", "original", 10)]))
		.await;
	assert_eq!(rig.fake(1).sent(event::TOOL_AFTER)[0]["calls"][0]["result"], "A");
	assert_eq!(
		result.replaces,
		vec![protocol::Replace {
			call_id: "c1".into(),
			text: "B(A)".into(),
			keep_original: true
		}]
	);
	assert!(result.stop.is_none());
}

#[tokio::test]
async fn the_first_stop_wins_and_later_plugins_are_not_asked() {
	let rig = rig(vec![
		plugin(
			"a",
			FakePlugin::new(&[event::TOOL_AFTER]).answer(json!({"stop": "enough"})),
		),
		plugin(
			"b",
			FakePlugin::new(&[event::TOOL_AFTER]).answer(json!({"stop": "more"})),
		),
	])
	.await;
	let result = rig
		.host
		.tool_after(&rig.ctx(&[]), tool_after(vec![outcome("c1", "x", 1)]))
		.await;
	assert_eq!(result.stop.as_deref(), Some("enough"));
	assert!(rig.fake(1).sent(event::TOOL_AFTER).is_empty());
}

#[tokio::test]
async fn the_first_continue_wins_and_verified_needs_every_vote() {
	let rig = rig(vec![
		plugin(
			"a",
			FakePlugin::new(&[event::TURN_END]).answer(json!({"verified": true})),
		),
		plugin(
			"b",
			FakePlugin::new(&[event::TURN_END]).answer(json!({"verified": false})),
		),
		plugin(
			"c",
			FakePlugin::new(&[event::TURN_END]).answer(json!({"continue": "keep going"})),
		),
		plugin(
			"d",
			FakePlugin::new(&[event::TURN_END]).answer(json!({"continue": "later"})),
		),
	])
	.await;
	let result = rig.host.turn_end(&rig.ctx(&[]), turn_end()).await;
	assert_eq!(result.continue_with.as_deref(), Some("keep going"));
	assert_eq!(result.verified, Some(false));
	assert!(rig.fake(3).sent(event::TURN_END).is_empty());

	let agreeing = self::rig(vec![
		plugin(
			"a",
			FakePlugin::new(&[event::TURN_END]).answer(json!({"verified": true})),
		),
		plugin("b", FakePlugin::new(&[event::TURN_END])),
	])
	.await;
	let result = agreeing.host.turn_end(&agreeing.ctx(&[]), turn_end()).await;
	assert_eq!(result.verified, Some(true));
	assert!(result.continue_with.is_none());
}

#[tokio::test]
async fn session_start_hands_each_plugin_only_its_own_inherited_value() {
	let rig = rig(vec![
		plugin("a", FakePlugin::new(&[event::SESSION_START])),
		plugin("b", FakePlugin::new(&[event::SESSION_START])),
	])
	.await;
	let inherited = [("b".to_string(), json!({"lessons": [1, 2]}))];
	rig.host.session_start(&rig.ctx(&[]), session_start(), &inherited).await;
	assert!(rig.fake(0).sent(event::SESSION_START)[0].get("inherited").is_none());
	assert_eq!(
		rig.fake(1).sent(event::SESSION_START)[0]["inherited"],
		json!({"lessons": [1, 2]})
	);
	assert_eq!(rig.fake(1).sent(event::SESSION_START)[0]["system_prompt"], "system");
}

#[tokio::test]
async fn subagent_start_collects_inherit_values_by_plugin() {
	let rig = rig(vec![
		plugin(
			"a",
			FakePlugin::new(&[event::SUBAGENT_START]).answer(json!({"inherit": {"x": 1}})),
		),
		plugin("b", FakePlugin::new(&[event::SUBAGENT_START])),
		plugin(
			"c",
			FakePlugin::new(&[event::SUBAGENT_START]).answer(json!({"inherit": "text"})),
		),
	])
	.await;
	let payload = SubagentStart {
		child_session_id: None,
		prompt: "p".into(),
		system_prompt: None,
	};
	let inherited = rig.host.subagent_start(&rig.ctx(&[]), payload).await;
	assert_eq!(
		inherited,
		vec![("a".to_string(), json!({"x": 1})), ("c".to_string(), json!("text"))]
	);
}

#[tokio::test]
async fn a_command_goes_to_the_named_plugin_only() {
	let rig = rig(vec![
		plugin(
			"a",
			FakePlugin::new(&[event::COMMAND]).answer(json!({"output": {"text": "from a"}})),
		),
		plugin(
			"b",
			FakePlugin::new(&[event::COMMAND]).answer(json!({"output": {"text": "from b"}})),
		),
		plugin("c", FakePlugin::new(&[event::TURN_END])),
	])
	.await;
	let command = || Command {
		name: "learn".into(),
		arguments: "now".into(),
	};
	let output = rig.host.command(&rig.ctx(&[]), "b", command()).await;
	assert_eq!(output.unwrap().text.as_deref(), Some("from b"));
	assert!(rig.fake(0).sent(event::COMMAND).is_empty());
	assert_eq!(rig.fake(1).sent(event::COMMAND)[0]["arguments"], "now");
	assert!(rig.host.command(&rig.ctx(&[]), "c", command()).await.is_none());
	assert!(rig.host.command(&rig.ctx(&[]), "nobody", command()).await.is_none());
}

#[tokio::test]
async fn status_collects_rows_per_plugin() {
	let rig = rig(vec![
		plugin(
			"a",
			FakePlugin::new(&[event::STATUS]).answer(json!({"rows": [{"label": "Lessons", "value": "4"}]})),
		),
		plugin("b", FakePlugin::new(&[event::STATUS])),
	])
	.await;
	let rows = rig.host.status(&rig.ctx(&[])).await;
	assert_eq!(rows.len(), 1);
	assert_eq!(rows[0].0, "a");
	assert_eq!(
		(rows[0].1[0].label.as_str(), rows[0].1[0].value.as_str()),
		("Lessons", "4")
	);
}

#[tokio::test]
async fn notifications_are_sent_without_waiting_for_an_answer() {
	let rig = rig(vec![
		plugin("a", FakePlugin::new(&[event::SESSION_END, event::COMPACT_AFTER])),
		plugin("b", FakePlugin::new(&[event::TURN_END])),
	])
	.await;
	rig.host
		.session_end(&rig.ctx(&[]), protocol::SessionEnd { reason: "exit".into() })
		.await;
	rig.host
		.compact_after(
			&rig.ctx(&[]),
			protocol::CompactAfter {
				carried_summary: None,
				goal: None,
			},
		)
		.await;
	rig.host
		.subagent_end(&rig.ctx(&[]), protocol::SubagentEnd { child_session_id: None })
		.await;
	assert_eq!(rig.fake(0).sent(event::SESSION_END)[0]["reason"], "exit");
	assert_eq!(rig.fake(0).sent(event::COMPACT_AFTER).len(), 1);
	assert!(rig.fake(0).sent(event::SUBAGENT_END).is_empty());
	assert!(rig.fake(1).sent(event::SESSION_END).is_empty());
}

// ---------------------------------------------------------------------------
// Failure policy
// ---------------------------------------------------------------------------

fn failing_until(failures: usize) -> FakePlugin {
	let calls = AtomicUsize::new(0);
	FakePlugin::new(&[event::REQUEST_BEFORE]).respond(move |_, _| {
		if calls.fetch_add(1, Ordering::SeqCst) < failures {
			bail!("boom")
		}
		Ok(json!({"note": ["alive"]}))
	})
}

#[tokio::test]
async fn an_error_counts_as_unanswered_and_queues_one_notice() {
	let rig = rig(vec![plugin("a", failing_until(1))]).await;
	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(effects, Effects::default());
	let notices = rig.host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(notices[0].contains("'a'") && notices[0].contains(event::REQUEST_BEFORE) && notices[0].contains("boom"));
	assert!(rig.host.active(SESSION));
	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(effects.notes, vec!["alive".to_string()]);
}

#[tokio::test]
async fn an_answer_that_is_not_an_answer_counts_as_a_failure() {
	let rig = rig(vec![plugin(
		"a",
		FakePlugin::new(&[event::REQUEST_BEFORE]).answer(json!({"notice": "not a list"})),
	)])
	.await;
	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(effects, Effects::default());
	assert_eq!(rig.host.take_notices(SESSION).len(), 1);
}

#[tokio::test]
async fn an_answer_past_the_time_limit_counts_as_unanswered() {
	let slow = FakePlugin::new(&[event::REQUEST_BEFORE])
		.answer(json!({"note": ["too late"]}))
		.delay(Duration::from_secs(3));
	let rig = rig(vec![plugin("a", slow)]).await;
	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(effects, Effects::default());
	let notices = rig.host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(notices[0].contains("within 1s"), "{}", notices[0]);
}

/// Make the pause of plugin `index` over, as if its time had passed.
fn end_the_pause(rig: &Rig, index: usize) {
	let mut sessions = rig.host.sessions.lock();
	sessions.get_mut(SESSION).unwrap().slots[index].paused_until = Some(Instant::now() - Duration::from_secs(1));
}

fn pause_seconds(rig: &Rig) -> u64 {
	rig.host.sessions.lock()[SESSION].slots[0].pause.as_secs()
}

/// A plugin that fails the first `MAX_CONSECUTIVE_FAILURES` events and answers every later one.
async fn paused_rig() -> Rig {
	let rig = rig(vec![plugin("a", failing_until(MAX_CONSECUTIVE_FAILURES as usize))]).await;
	for _ in 0..MAX_CONSECUTIVE_FAILURES {
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	}
	rig
}

#[tokio::test]
async fn three_failures_in_a_row_pause_the_plugin_and_its_events_are_skipped_with_one_notice() {
	let rig = rig(vec![
		plugin("a", failing_until(usize::MAX)),
		plugin("b", FakePlugin::new(&[event::REQUEST_BEFORE])),
	])
	.await;
	for _ in 0..MAX_CONSECUTIVE_FAILURES {
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	}
	let notices = rig.host.take_notices(SESSION);
	assert_eq!(notices.len() as u32, MAX_CONSECUTIVE_FAILURES);
	assert!(
		notices.last().unwrap().contains("did not answer 3 events in a row")
			&& notices.last().unwrap().contains("tries again in 30s"),
		"{:?}",
		notices
	);

	let sent_before = rig.fake(0).sent(event::REQUEST_BEFORE).len();
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;

	assert_eq!(rig.fake(0).sent(event::REQUEST_BEFORE).len(), sent_before);
	assert!(!rig.host.sessions.lock()[SESSION].slots[0].takes(event::REQUEST_BEFORE));
	assert!(rig.host.active(SESSION), "a paused plugin is still part of the session");
	assert!(rig.host.take_notices(SESSION).is_empty());
	// The other plugin carries on.
	assert_eq!(
		rig.fake(1).sent(event::REQUEST_BEFORE).len() as u32,
		MAX_CONSECUTIVE_FAILURES + 1
	);
}

#[tokio::test]
async fn a_paused_plugin_keeps_its_blocks_and_commands() {
	let calls = AtomicUsize::new(0);
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE])
		.init_field(
			"commands",
			json!([{"name": "learn", "hint": "", "description": "Learn"}]),
		)
		.respond(move |_, _| {
			if calls.fetch_add(1, Ordering::SeqCst) == 0 {
				Ok(json!({"context": [{"name": "c", "text": "t"}], "pin": [{"name": "p", "text": "x"}]}))
			} else {
				bail!("boom")
			}
		});
	let rig = rig(vec![plugin("a", fake)]).await;
	for _ in 0..=MAX_CONSECUTIVE_FAILURES {
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	}

	assert!(rig.host.sessions.lock()[SESSION].slots[0].is_paused());
	assert_eq!(rig.host.context_blocks(SESSION).len(), 1);
	assert_eq!(rig.host.pinned_blocks(SESSION).len(), 1);
	assert_eq!(rig.host.commands(SESSION).len(), 1);
}

#[tokio::test]
async fn once_the_pause_is_over_the_next_event_is_sent_and_an_answer_brings_the_plugin_back() {
	let rig = paused_rig().await;
	rig.host.take_notices(SESSION);
	end_the_pause(&rig, 0);

	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;

	assert_eq!(effects.notes, vec!["alive".to_string()]);
	assert_eq!(
		rig.host.take_notices(SESSION),
		vec!["plugin 'a' answers again".to_string()]
	);
	{
		let sessions = rig.host.sessions.lock();
		let slot = &sessions[SESSION].slots[0];
		assert!(slot.paused_until.is_none() && slot.failures == 0);
		assert_eq!(slot.pause, FIRST_PAUSE);
	}
	// Back for good: the next event is a plain one and tells nobody.
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(
		rig.fake(0).sent(event::REQUEST_BEFORE).len() as u32,
		MAX_CONSECUTIVE_FAILURES + 2
	);
	assert!(rig.host.take_notices(SESSION).is_empty());
}

#[tokio::test]
async fn a_failed_trial_doubles_the_pause_up_to_the_cap() {
	let rig = rig(vec![plugin("a", failing_until(usize::MAX))]).await;
	for _ in 0..MAX_CONSECUTIVE_FAILURES {
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	}
	rig.host.take_notices(SESSION);
	assert_eq!(pause_seconds(&rig), FIRST_PAUSE.as_secs());

	let mut pauses = Vec::new();
	for _ in 0..6 {
		end_the_pause(&rig, 0);
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
		pauses.push(pause_seconds(&rig));
	}

	assert_eq!(
		pauses,
		vec![60, 120, 240, 480, LONGEST_PAUSE.as_secs(), LONGEST_PAUSE.as_secs()]
	);
	assert!(rig.host.sessions.lock()[SESSION].slots[0].is_paused());
	assert!(
		rig.host.take_notices(SESSION).is_empty(),
		"a failed trial is logged, not shown"
	);
	assert_eq!(
		rig.fake(0).sent(event::REQUEST_BEFORE).len() as u32,
		MAX_CONSECUTIVE_FAILURES + 6
	);
}

#[tokio::test]
async fn a_failure_that_arrives_during_a_pause_does_not_lengthen_it() {
	let rig = paused_rig().await;

	rig.host.reject(SESSION, "a", event::REQUEST_BEFORE, &anyhow!("late"));

	assert_eq!(pause_seconds(&rig), FIRST_PAUSE.as_secs());
}

#[tokio::test]
async fn a_closed_connection_is_opened_again_and_the_next_event_carries_the_full_history() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	let mut messages = vec![user("one"), user("two")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.push(user("three"));
	rig.fake(0).disconnect();

	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.push(user("four"));
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;

	assert_eq!(rig.connects.load(Ordering::SeqCst), 2);
	assert_eq!(rig.fake(0).sent(METHOD_INITIALIZE).len(), 2);
	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(sent[1]["messages"].as_array().unwrap().len(), 3);
	assert_eq!(sent[1]["messages_reset"], true);
	assert_eq!(sent[2]["messages"].as_array().unwrap().len(), 1);
	assert_eq!(sent[2]["messages_reset"], false);
	assert!(rig.host.take_notices(SESSION).is_empty());
}

/// A connector that can be told to refuse, to see what a failed reconnect does.
struct RefusingConnector {
	plugin: FakePlugin,
	refuse: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Connector for RefusingConnector {
	async fn connect(&self, _config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		if self.refuse.load(Ordering::SeqCst) {
			bail!("nobody listens");
		}
		self.plugin.reopen();
		Ok(Arc::new(self.plugin.clone()))
	}
}

#[tokio::test]
async fn a_connection_that_cannot_be_opened_again_counts_as_a_failed_event() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]);
	let refuse = Arc::new(std::sync::atomic::AtomicBool::new(false));
	let host = Host::new(Arc::new(RefusingConnector {
		plugin: fake.clone(),
		refuse: refuse.clone(),
	}));
	let dir = tempfile::tempdir().unwrap();
	host.start_session(SESSION, ROLE, dir.path(), &[config("a")]).await;
	let ctx = EventContext {
		session_id: SESSION,
		parent_session_id: None,
		role: ROLE,
		model: None,
		workdir: dir.path(),
		project: "proj",
		messages: &[],
		detached: false,
		status: None,
		plan: None,
		tools: None,
		active_skills: Vec::new(),
	};
	fake.disconnect();
	refuse.store(true, Ordering::SeqCst);

	host.request_before(&ctx, request_before()).await;

	assert!(fake.sent(event::REQUEST_BEFORE).is_empty());
	let notices = host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(notices[0].contains("nobody listens"), "{}", notices[0]);

	refuse.store(false, Ordering::SeqCst);
	host.request_before(&ctx, request_before()).await;
	assert_eq!(fake.sent(event::REQUEST_BEFORE).len(), 1);
}

#[tokio::test]
async fn a_success_resets_the_failure_count() {
	let calls = AtomicUsize::new(0);
	// fail, fail, ok, fail, fail, ok: never three in a row.
	let flaky = FakePlugin::new(&[event::REQUEST_BEFORE]).respond(move |_, _| {
		if calls.fetch_add(1, Ordering::SeqCst) % 3 == 2 {
			Ok(json!({}))
		} else {
			bail!("boom")
		}
	});
	let rig = rig(vec![plugin("a", flaky)]).await;
	for _ in 0..6 {
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	}
	assert!(rig.host.active(SESSION));
	assert_eq!(rig.fake(0).sent(event::REQUEST_BEFORE).len(), 6);
}

#[tokio::test]
async fn rule_matches_of_a_failed_event_go_out_with_the_next_one() {
	let rig = rig(vec![plugin("a", failing_until(1))]).await;
	rig.host.record_rule_match(SESSION, "a", "r1", RuleOutcome::Applied);
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(sent[0]["rule_matches"][0]["id"], "r1");
	assert_eq!(sent[1]["rule_matches"][0]["id"], "r1");
}

#[tokio::test]
async fn messages_of_a_failed_event_are_sent_again() {
	let rig = rig(vec![plugin("a", failing_until(1))]).await;
	let messages = [user("one")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(sent[0]["messages"].as_array().unwrap().len(), 1);
	assert_eq!(sent[1]["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn ending_the_session_forgets_everything() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	rig.host.end_session(SESSION);
	assert!(!rig.host.active(SESSION));
	assert!(rig.host.context_blocks(SESSION).is_empty());
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert!(rig.fake(0).sent(event::REQUEST_BEFORE).is_empty());
}

// ---------------------------------------------------------------------------
// Time limits, tool list, plan errors, dropped blocks, background output
// ---------------------------------------------------------------------------

fn config_without_limit(name: &str) -> PluginConfig {
	toml::from_str(&format!("name = \"{name}\"\ntype = \"stdio\"\ncommand = \"fake\"\n")).unwrap()
}

#[tokio::test]
async fn the_limit_of_an_event_is_the_one_the_plugin_declared() {
	let slow = FakePlugin::new(&[event::REQUEST_BEFORE])
		.init_field("timeouts", json!({"request.before": 1}))
		.answer(json!({"note": ["too late"]}))
		.delay(Duration::from_secs(3));
	let rig = rig(vec![(config_without_limit("a"), slow)]).await;
	let effects = rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	assert_eq!(effects, Effects::default());
	let notices = rig.host.take_notices(SESSION);
	assert!(notices[0].contains("within 1s"), "{}", notices[0]);
}

#[tokio::test]
async fn the_tool_list_is_sent_again_only_when_it_changed() {
	let tool = |name: &str| ToolInfo {
		name: name.into(),
		server: "core".into(),
		description: String::new(),
		parameters: Value::Null,
		read_only: None,
	};
	let rig = rig(vec![plugin(
		"a",
		FakePlugin::new(&[event::SESSION_START, event::REQUEST_BEFORE]),
	)])
	.await;
	let mut ctx = rig.ctx(&[]);
	ctx.tools = Some(vec![tool("one")]);
	rig.host.session_start(&ctx, session_start(), &[]).await;
	rig.host.request_before(&ctx, request_before()).await;
	ctx.tools = Some(vec![tool("one"), tool("two")]);
	rig.host.request_before(&ctx, request_before()).await;
	rig.host.request_before(&ctx, request_before()).await;

	assert_eq!(
		rig.fake(0).sent(event::SESSION_START)[0]["tools"]
			.as_array()
			.unwrap()
			.len(),
		1
	);
	// Without a list the caller has, session.start still carries one, empty.
	let empty = self::rig(vec![plugin("b", FakePlugin::new(&[event::SESSION_START]))]).await;
	empty.host.session_start(&empty.ctx(&[]), session_start(), &[]).await;
	assert_eq!(empty.fake(0).sent(event::SESSION_START)[0]["tools"], json!([]));
	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert!(
		sent[0].get("tools").is_none(),
		"the list of session.start is the reference"
	);
	assert_eq!(sent[1]["tools"].as_array().unwrap().len(), 2);
	assert!(sent[2].get("tools").is_none());
}

#[tokio::test]
async fn a_plan_error_goes_to_the_plugin_whose_plan_won_and_only_once() {
	let planner = FakePlugin::new(&[event::COMPACT_BEFORE, event::REQUEST_BEFORE]).respond(|method, _| {
		Ok(match method {
			event::COMPACT_BEFORE => json!({"plan": {"action": "finish", "summary": "s"}}),
			_ => json!({}),
		})
	});
	let rig = rig(vec![
		plugin("planner", planner),
		plugin("bystander", FakePlugin::new(&[event::REQUEST_BEFORE])),
	])
	.await;
	rig.host
		.compact_before(&rig.ctx(&[]), CompactBefore { forced: false })
		.await;
	rig.host.record_plan_error(SESSION, "no plan is open");
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(sent[0]["plan_error"], "no plan is open");
	assert!(sent[1].get("plan_error").is_none());
	for params in rig.fake(1).sent(event::REQUEST_BEFORE) {
		assert!(params.get("plan_error").is_none());
	}
}

#[tokio::test]
async fn blocks_keep_their_order_and_items() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).answer(json!({
		"context": [
			{"name": "zeta", "text": "1"},
			{"name": "alpha", "text": "2"}
		],
		"pin": [{"name": "p", "text": "t", "items": ["i1", "i2"]}]
	}));
	let rig = rig(vec![plugin("a", fake)]).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let context = rig.host.context_blocks(SESSION);
	assert_eq!(
		context.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
		vec!["zeta", "alpha"]
	);
	assert_eq!(
		rig.host.pinned_blocks(SESSION)[0].items,
		vec!["i1".to_string(), "i2".to_string()]
	);
}

#[tokio::test]
async fn project_skills_and_goal_reach_the_plugin_and_options_are_readable() {
	let fake = FakePlugin::new(&[event::PROMPT_SUBMIT]).init_field("options", json!({"diff_chars": 50}));
	let rig = rig(vec![plugin("a", fake)]).await;
	let mut ctx = rig.ctx(&[]);
	ctx.active_skills = vec!["review".into()];
	let mut submit = prompt("x");
	submit.goal = Some("ship it".into());
	rig.host.prompt_submit(&ctx, submit).await;
	let sent = &rig.fake(0).sent(event::PROMPT_SUBMIT)[0];
	assert_eq!(sent["project"], "proj");
	assert_eq!(sent["active_skills"], json!(["review"]));
	assert_eq!(sent["goal"], "ship it");
	let options = rig.host.options(SESSION);
	assert_eq!(options.len(), 1);
	assert_eq!(options[0].0, "a");
	assert_eq!(options[0].1.diff_chars, Some(50));
}

#[tokio::test]
async fn a_replace_may_name_a_background_output_and_the_next_plugin_sees_it() {
	let first = FakePlugin::new(&[event::TOOL_AFTER]).answer(json!({
		"replace": [{"call_id": "job1", "text": "short"}]
	}));
	let rig = rig(vec![
		plugin("a", first),
		plugin("b", FakePlugin::new(&[event::TOOL_AFTER])),
	])
	.await;
	let mut payload = tool_after(Vec::new());
	payload.background = vec![BackgroundOutput {
		call_id: "job1".into(),
		label: "build".into(),
		output: "very long output".into(),
	}];
	let result = rig.host.tool_after(&rig.ctx(&[]), payload).await;
	assert_eq!(
		rig.fake(1).sent(event::TOOL_AFTER)[0]["background"][0]["output"],
		"short"
	);
	assert_eq!(
		rig.fake(1).sent(event::TOOL_AFTER)[0]["background"][0]["label"],
		"build"
	);
	assert_eq!(result.replaces.len(), 1);
	assert_eq!(result.replaces[0].call_id, "job1");
}

// ---------------------------------------------------------------------------
// The test hook for other modules
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_stub_on_the_process_wide_host_answers_the_free_functions() {
	use crate::plugins::test_support::connect_stub;
	const HOOK_SESSION: &str = "hook-test-session";
	let stub =
		FakePlugin::new(&[event::TOOL_BEFORE]).queue([json!({"deny": [{"call_id": "c1", "reason": "stubbed"}]})]);
	let dir = tempfile::tempdir().unwrap();
	connect_stub(HOOK_SESSION, ROLE, dir.path(), &stub).await;
	assert!(crate::plugins::active(HOOK_SESSION));
	assert!(crate::plugins::subscribed(HOOK_SESSION, event::TOOL_BEFORE));

	let ctx = EventContext {
		session_id: HOOK_SESSION,
		parent_session_id: None,
		role: ROLE,
		model: None,
		workdir: dir.path(),
		project: "proj",
		messages: &[],
		detached: false,
		status: None,
		plan: None,
		tools: None,
		active_skills: Vec::new(),
	};
	let first = crate::plugins::tool_before(&ctx, tool_before("c1")).await;
	assert_eq!(first.denies.len(), 1);
	assert_eq!(first.denies[0].reason, "stubbed");
	// The queue is used up: the stub falls back to an empty answer.
	let second = crate::plugins::tool_before(&ctx, tool_before("c1")).await;
	assert!(second.denies.is_empty());
	assert_eq!(stub.sent(event::TOOL_BEFORE).len(), 2);
	assert_eq!(stub.sent(event::TOOL_BEFORE)[0]["calls"][0]["id"], "c1");

	crate::plugins::end_session(HOOK_SESSION);
	assert!(!crate::plugins::active(HOOK_SESSION));
}

// ---------------------------------------------------------------------------
// Time limits of the history send and of connecting, events dropped mid-send
// ---------------------------------------------------------------------------

/// A plugin that takes every message except the session history, which it never takes.
struct StuckOnHistory(FakePlugin);

#[async_trait]
impl Transport for StuckOnHistory {
	async fn request(&self, method: &str, params: Value, limit: WaitLimit) -> Result<Value> {
		self.0.request(method, params, limit).await
	}

	async fn notify(&self, method: &str, params: Value) -> Result<()> {
		if method == METHOD_SESSION_HISTORY {
			std::future::pending::<()>().await;
		}
		self.0.notify(method, params).await
	}

	fn is_connected(&self) -> bool {
		true
	}
}

struct FixedConnector(Arc<dyn Transport>);

#[async_trait]
impl Connector for FixedConnector {
	async fn connect(&self, _config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		Ok(self.0.clone())
	}
}

struct NeverConnects;

#[async_trait]
impl Connector for NeverConnects {
	async fn connect(&self, _config: &PluginConfig) -> Result<Arc<dyn Transport>> {
		std::future::pending().await
	}
}

/// A test that waits on something it expects to end fails instead of hanging.
const TEST_PATIENCE: Duration = Duration::from_secs(20);

#[tokio::test]
async fn a_history_send_the_plugin_never_takes_times_out_and_counts_as_a_failure() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]).answer(json!({"history": true}));
	let dir = tempfile::tempdir().unwrap();
	let host = Host::new(Arc::new(FixedConnector(Arc::new(StuckOnHistory(fake.clone())))));
	host.start_session(SESSION, ROLE, dir.path(), &[config("a")]).await;

	let ctx = EventContext {
		session_id: SESSION,
		parent_session_id: None,
		role: ROLE,
		model: None,
		workdir: dir.path(),
		project: "proj",
		messages: &[],
		detached: false,
		status: None,
		plan: None,
		tools: None,
		active_skills: Vec::new(),
	};
	tokio::time::timeout(TEST_PATIENCE, host.request_before(&ctx, request_before()))
		.await
		.expect("the history send is bounded by the plugin's time limit");
	let notices = host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(notices[0].contains("not taken within 1s"), "{}", notices[0]);
}

#[tokio::test]
async fn a_connect_that_never_ends_turns_the_plugin_off_with_a_notice() {
	let dir = tempfile::tempdir().unwrap();
	let host = Host::new(Arc::new(NeverConnects));
	tokio::time::timeout(
		TEST_PATIENCE,
		host.start_session(SESSION, ROLE, dir.path(), &[config("slow")]),
	)
	.await
	.expect("connecting is bounded by the plugin's time limit");
	assert!(!host.active(SESSION));
	let notices = host.take_notices(SESSION);
	assert_eq!(notices.len(), 1);
	assert!(
		notices[0].contains("'slow'") && notices[0].contains("not connected within 1s"),
		"{}",
		notices[0]
	);
}

/// A plugin that completes the handshake and then takes long to answer an event.
struct SlowAnswers(FakePlugin);

#[async_trait]
impl Transport for SlowAnswers {
	async fn request(&self, method: &str, params: Value, limit: WaitLimit) -> Result<Value> {
		let answer = self.0.request(method, params, limit).await;
		if method != METHOD_INITIALIZE {
			tokio::time::sleep(Duration::from_secs(3)).await;
		}
		answer
	}

	async fn notify(&self, method: &str, params: Value) -> Result<()> {
		self.0.notify(method, params).await
	}

	fn is_connected(&self) -> bool {
		true
	}
}

#[tokio::test]
async fn what_the_plugin_is_due_to_hear_survives_a_send_that_is_dropped() {
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]);
	let dir = tempfile::tempdir().unwrap();
	let host = Host::new(Arc::new(FixedConnector(Arc::new(SlowAnswers(fake.clone())))));
	host.start_session(SESSION, ROLE, dir.path(), &[config("a")]).await;
	assert!(host.active(SESSION));
	let ctx = EventContext {
		session_id: SESSION,
		parent_session_id: None,
		role: ROLE,
		model: None,
		workdir: dir.path(),
		project: "proj",
		messages: &[],
		detached: false,
		status: None,
		plan: None,
		tools: None,
		active_skills: Vec::new(),
	};
	host.record_rule_match(SESSION, "a", "r1", RuleOutcome::Applied);

	// The user cancels while the plugin is still working on the first event.
	let cancelled = tokio::time::timeout(Duration::from_millis(100), host.request_before(&ctx, request_before())).await;
	assert!(cancelled.is_err(), "the first event is dropped before it is answered");

	host.request_before(&ctx, request_before()).await;
	let sent = fake.sent(event::REQUEST_BEFORE);
	assert_eq!(sent.len(), 2);
	assert_eq!(sent[1]["rule_matches"][0]["id"], "r1");
}

// ---------------------------------------------------------------------------
// Events from places without a message list, judgements, the count of judged answers
// ---------------------------------------------------------------------------

fn detached(rig: &Rig) -> EventContext<'_> {
	EventContext {
		detached: true,
		..rig.ctx(&[])
	}
}

fn message_counts(sent: &[Value]) -> Vec<usize> {
	sent.iter()
		.map(|params| params["messages"].as_array().unwrap().len())
		.collect()
}

#[tokio::test]
async fn an_event_without_a_message_list_carries_none_and_the_next_one_sends_only_what_is_new() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	let mut messages = vec![user("one"), user("two")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	messages.push(user("three"));

	rig.host.request_before(&detached(&rig), request_before()).await;
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;

	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(message_counts(&sent), vec![2, 0, 1]);
	assert_eq!(sent[1]["messages_reset"], false);
	assert_eq!(sent[2]["messages"][0]["content"], "three");
	assert_eq!(sent[2]["messages_reset"], false);
}

#[tokio::test]
async fn an_event_without_a_message_list_leaves_a_pending_reset_for_the_next_event_with_one() {
	let rig = rig(vec![plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]))]).await;
	let messages = vec![user("one"), user("two")];
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;
	rig.host.mark_history_reset(SESSION);

	rig.host.request_before(&detached(&rig), request_before()).await;
	assert!(rig.host.sessions.lock()[SESSION].slots[0].reset_pending);
	rig.host.request_before(&rig.ctx(&messages), request_before()).await;

	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(message_counts(&sent), vec![2, 0, 2]);
	assert_eq!(sent[1]["messages_reset"], false);
	assert_eq!(sent[2]["messages_reset"], true);
}

fn judge_payload() -> Judge {
	Judge {
		purpose: "skills".into(),
		subject: json!({}),
		questions: BTreeMap::new(),
		items: None,
	}
}

fn judging_plugin() -> FakePlugin {
	FakePlugin::new(&[event::JUDGE]).answer(json!({"judgements": {"q": {"type": "yes_no", "yes": 0.8}}}))
}

#[tokio::test]
async fn the_first_plugin_that_judges_wins_and_a_plugin_with_no_judgement_is_skipped() {
	let rig = rig(vec![
		plugin("silent", FakePlugin::new(&[event::JUDGE])),
		plugin("first", judging_plugin()),
		plugin("second", judging_plugin()),
	])
	.await;

	let judged = rig
		.host
		.judge(&rig.ctx(&[]), judge_payload())
		.await
		.expect("a plugin judged");

	assert_eq!(judged.plugin, "first");
	assert!(judged.judgements.contains_key("q"));
	assert_eq!(rig.fake(0).sent(event::JUDGE).len(), 1, "the silent plugin was asked");
	assert!(
		rig.fake(2).sent(event::JUDGE).is_empty(),
		"the second judge was not asked"
	);
}

#[tokio::test]
async fn a_judge_event_that_no_plugin_answers_with_a_judgement_gives_none() {
	let rig = rig(vec![plugin("silent", FakePlugin::new(&[event::JUDGE]))]).await;

	assert!(rig.host.judge(&rig.ctx(&[]), judge_payload()).await.is_none());

	let nobody = self::rig(Vec::new()).await;
	assert!(nobody.host.judge(&nobody.ctx(&[]), judge_payload()).await.is_none());
}

#[tokio::test]
async fn judged_counts_go_out_with_the_next_event_and_stay_until_one_is_answered() {
	let rig = rig(vec![plugin("a", failing_until(1))]).await;
	rig.host.record_judged(SESSION, "a", "skills", 2);
	rig.host.record_judged(SESSION, "a", "skills", 1);
	rig.host.record_judged(SESSION, "a", "capabilities", 4);

	for _ in 0..3 {
		rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	}

	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(sent[0]["judged"], json!({"skills": 3, "capabilities": 4}));
	assert_eq!(
		sent[1]["judged"],
		json!({"skills": 3, "capabilities": 4}),
		"the first event failed"
	);
	assert!(sent[2].get("judged").is_none(), "the second event was answered");
}

#[tokio::test]
async fn counts_added_while_an_event_runs_stay_for_the_next_one() {
	let slow = FakePlugin::new(&[event::REQUEST_BEFORE]).delay(Duration::from_millis(300));
	let rig = rig(vec![plugin("a", slow)]).await;
	rig.host.record_judged(SESSION, "a", "skills", 2);

	let ctx = rig.ctx(&[]);
	tokio::join!(rig.host.request_before(&ctx, request_before()), async {
		tokio::time::sleep(Duration::from_millis(100)).await;
		rig.host.record_judged(SESSION, "a", "skills", 1);
	});
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;

	let sent = rig.fake(0).sent(event::REQUEST_BEFORE);
	assert_eq!(sent[0]["judged"], json!({"skills": 2}));
	assert_eq!(sent[1]["judged"], json!({"skills": 1}));
	assert!(sent[2].get("judged").is_none());
}

// ---------------------------------------------------------------------------
// Time the agent waited
// ---------------------------------------------------------------------------

const SLOW_ANSWER: Duration = Duration::from_millis(300);

#[tokio::test]
async fn the_time_of_an_awaited_event_is_counted_once_and_a_notification_counts_nothing() {
	let slow = FakePlugin::new(&[event::REQUEST_BEFORE, event::COMPACT_AFTER]).delay(SLOW_ANSWER);
	let rig = rig(vec![plugin("a", slow)]).await;

	rig.host.request_before(&rig.ctx(&[]), request_before()).await;
	let waited = rig.host.take_waited_ms(SESSION);
	assert!(waited >= SLOW_ANSWER.as_millis() as u64, "{waited}");
	assert_eq!(rig.host.take_waited_ms(SESSION), 0, "taking clears the time");

	rig.host
		.compact_after(
			&rig.ctx(&[]),
			protocol::CompactAfter {
				carried_summary: None,
				goal: None,
			},
		)
		.await;
	assert_eq!(rig.fake(0).sent(event::COMPACT_AFTER).len(), 1);
	assert_eq!(rig.host.take_waited_ms(SESSION), 0);
}

#[tokio::test]
async fn events_that_run_at_the_same_time_count_their_union() {
	let slow = FakePlugin::new(&[event::REQUEST_BEFORE]).delay(SLOW_ANSWER);
	let rig = rig(vec![plugin("a", slow)]).await;

	let (first, second) = (rig.ctx(&[]), rig.ctx(&[]));
	tokio::join!(
		rig.host.request_before(&first, request_before()),
		rig.host.request_before(&second, request_before())
	);

	let waited = rig.host.take_waited_ms(SESSION);
	let one_answer = SLOW_ANSWER.as_millis() as u64;
	assert!(waited >= one_answer && waited < 2 * one_answer, "{waited}");
}

#[tokio::test]
async fn an_event_that_is_dropped_still_stops_the_clock() {
	// The stub delays `initialize` too and would fail the handshake; this one answers it at once.
	let fake = FakePlugin::new(&[event::REQUEST_BEFORE]);
	let host = Host::new(Arc::new(FixedConnector(Arc::new(SlowAnswers(fake)))));
	let dir = tempfile::tempdir().unwrap();
	host.start_session(SESSION, ROLE, dir.path(), &[config("a")]).await;
	let ctx = EventContext {
		session_id: SESSION,
		parent_session_id: None,
		role: ROLE,
		model: None,
		workdir: dir.path(),
		project: "proj",
		messages: &[],
		detached: false,
		status: None,
		plan: None,
		tools: None,
		active_skills: Vec::new(),
	};

	let cancelled = tokio::time::timeout(Duration::from_millis(100), host.request_before(&ctx, request_before())).await;
	assert!(cancelled.is_err());
	let waited = host.take_waited_ms(SESSION);
	assert!(waited >= 100, "{waited}");

	tokio::time::sleep(Duration::from_millis(200)).await;
	assert_eq!(host.take_waited_ms(SESSION), 0, "the clock kept running after the drop");
}

#[tokio::test]
async fn usage_passes_to_the_same_plugin_of_another_session_and_the_rest_is_dropped() {
	let answer = json!({"usage": [{"label": "judge", "calls": 1, "input_tokens": 1, "output_tokens": 1, "cost": 0.5}]});
	let rig = rig(vec![
		plugin("a", FakePlugin::new(&[event::REQUEST_BEFORE]).answer(answer.clone())),
		plugin("b", FakePlugin::new(&[event::REQUEST_BEFORE]).answer(answer)),
	])
	.await;
	rig.host
		.start_session("parent", ROLE, rig.dir.path(), &[config("a")])
		.await;
	rig.host.request_before(&rig.ctx(&[]), request_before()).await;

	rig.host.pass_usage(SESSION, "parent");

	assert!(rig.host.take_usage(SESSION).is_empty());
	let passed = rig.host.take_usage("parent");
	assert_eq!(passed.len(), 1);
	assert_eq!((passed[0].0.as_str(), passed[0].1.cost), ("a", 0.5));
}

#[tokio::test]
async fn session_end_waits_five_seconds_at_most_whatever_the_plugin_declares() {
	let fake = FakePlugin::new(&[event::SESSION_END, event::TURN_END])
		.init_field("timeouts", json!({"session.end": 3600, "turn.end": 120}));
	let rig = rig(vec![(config_without_limit("a"), fake)]).await;
	let limit_of = |event_name| rig.host.prepare(&rig.ctx(&[]), "a", event_name).unwrap().limit;

	assert_eq!(
		limit_of(event::SESSION_END),
		WaitLimit {
			silence: SESSION_END_WAIT,
			total: SESSION_END_WAIT
		}
	);
	assert_eq!(limit_of(event::TURN_END).silence, Duration::from_secs(120));
}
