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

//! Session-scoped registry tests: workdir, role, config, hints, dynamic
//! agents/servers, capability refcounts and cleanup. Every registry is a
//! process global — each test uses a unique session id so parallel tests
//! never observe each other's state.

use super::*;
use crate::mcp::runtime::dynamic_agents::DynamicAgentConfig;

fn unique(label: &str) -> SessionId {
	format!("ctx-reg-test-{label}")
}

fn func(name: &str) -> crate::mcp::McpFunction {
	crate::mcp::McpFunction {
		name: name.to_string(),
		description: "test function".to_string(),
		parameters: serde_json::json!({}),
	}
}

#[test]
fn test_workdir_anchor_and_override() {
	let sid = unique("workdir");
	assert!(get_current_workdir(&sid).is_none());

	set_session_workdir(&sid, PathBuf::from("/anchor"));
	assert_eq!(get_current_workdir(&sid), Some(PathBuf::from("/anchor")));
	assert_eq!(
		get_session_workdir_anchor(&sid),
		Some(PathBuf::from("/anchor"))
	);

	// Mid-session override moves current but preserves the anchor
	set_current_workdir(&sid, PathBuf::from("/anchor/sub"));
	assert_eq!(
		get_current_workdir(&sid),
		Some(PathBuf::from("/anchor/sub"))
	);
	assert_eq!(
		get_session_workdir_anchor(&sid),
		Some(PathBuf::from("/anchor"))
	);

	clear_session_workdir(&sid);
	assert!(get_current_workdir(&sid).is_none());
}

#[tokio::test]
async fn test_role_and_domain() {
	let sid = unique("role");
	assert!(get_session_role(&sid).is_none());

	set_session_role(&sid, "doctor:blood");
	assert_eq!(get_session_role(&sid).as_deref(), Some("doctor:blood"));

	// Domain is the tag prefix, resolved through the task-local session id
	let domain = with_session_id(sid.clone(), async { current_session_domain() }).await;
	assert_eq!(domain.as_deref(), Some("doctor"));

	clear_session_role(&sid);
	assert!(get_session_role(&sid).is_none());
}

#[test]
fn test_hints_dedup_and_drain() {
	let sid = unique("hints");
	assert!(!has_hints_for_session(&sid));

	push_hint_for_session(&sid, "first".to_string());
	push_hint_for_session(&sid, "first".to_string());
	push_hint_for_session(&sid, "second".to_string());
	assert!(has_hints_for_session(&sid));

	let drained = drain_hints_for_session(&sid);
	assert_eq!(drained, vec!["first".to_string(), "second".to_string()]);
	// Draining empties the queue
	assert!(!has_hints_for_session(&sid));
	assert!(drain_hints_for_session(&sid).is_empty());
}

#[test]
fn test_task_start_index() {
	let sid = unique("task-idx");
	assert!(get_task_start_index(&sid).is_none());
	set_task_start_index(&sid, 7);
	assert_eq!(get_task_start_index(&sid), Some(7));
	clear_task_start_index(&sid);
	assert!(get_task_start_index(&sid).is_none());
}

#[test]
fn test_dynamic_agents_lifecycle() {
	let sid = unique("dyn-agent");
	let agent = DynamicAgentConfig {
		name: "helper".to_string(),
		description: "test agent".to_string(),
		system: "you help".to_string(),
		welcome: String::new(),
		model: None,
		temperature: None,
		top_p: None,
		top_k: None,
		server_refs: Vec::new(),
		allowed_tools: Vec::new(),
		workdir: ".".to_string(),
	};

	register_dynamic_agent_for_session(&sid, agent);
	assert!(has_dynamic_agent(&sid, "helper"));
	// Registered agents start disabled
	assert!(!is_dynamic_agent_enabled(&sid, "helper"));

	assert!(enable_dynamic_agent_for_session(&sid, "helper"));
	assert!(is_dynamic_agent_enabled(&sid, "helper"));
	assert!(disable_dynamic_agent_for_session(&sid, "helper"));
	assert!(!is_dynamic_agent_enabled(&sid, "helper"));

	// Unknown agent operations report failure, not panic
	assert!(!enable_dynamic_agent_for_session(&sid, "ghost"));

	let agents = get_dynamic_agents_for_session(&sid);
	assert_eq!(agents.len(), 1);

	let removed = remove_dynamic_agent_for_session(&sid, "helper");
	assert!(removed.is_some());
	assert!(!has_dynamic_agent(&sid, "helper"));

	clear_dynamic_agents_for_session(&sid);
}

#[test]
fn test_dynamic_servers_lifecycle() {
	let sid = unique("dyn-server");
	let server = crate::config::McpServerConfig::builtin("dynsrv", 30, Vec::new());

	register_dynamic_server_for_session(&sid, server);
	assert!(has_dynamic_server(&sid, "dynsrv"));
	// Registered servers start disabled: not part of the active tool surface
	assert!(get_all_dynamic_server_configs_for_session(&sid).is_empty());

	assert!(enable_dynamic_server_for_session(
		&sid,
		"dynsrv",
		vec![func("alpha"), func("beta")]
	));
	assert_eq!(get_all_dynamic_server_configs_for_session(&sid).len(), 1);
	assert!(is_dynamic_server_tool(&sid, "alpha"));
	assert_eq!(
		get_dynamic_server_name_by_tool(&sid, "alpha").as_deref(),
		Some("dynsrv")
	);
	assert!(!is_dynamic_server_tool(&sid, "gamma"));

	// Re-enabling with an overlapping function set does not duplicate
	assert!(enable_dynamic_server_for_session(
		&sid,
		"dynsrv",
		vec![func("alpha"), func("gamma")]
	));
	let funcs = get_dynamic_server_functions_for_session(&sid, "dynsrv").expect("functions");
	assert_eq!(funcs.len(), 3);
	assert_eq!(get_all_dynamic_server_functions_for_session(&sid).len(), 3);

	assert!(disable_dynamic_server_for_session(&sid, "dynsrv"));
	assert!(get_all_dynamic_server_configs_for_session(&sid).is_empty());

	// Enabling a server that was never registered fails
	assert!(!enable_dynamic_server_for_session(
		&sid,
		"ghost",
		Vec::new()
	));

	assert!(remove_dynamic_server_for_session(&sid, "dynsrv").is_some());
	assert!(!has_dynamic_server(&sid, "dynsrv"));
	clear_dynamic_servers_for_session(&sid);
}

#[test]
fn test_capability_refcounts() {
	let sid = unique("cap-ref");
	assert_eq!(increment_capability_refcount(&sid, "octofs"), 1);
	assert_eq!(increment_capability_refcount(&sid, "octofs"), 2);
	assert_eq!(decrement_capability_refcount(&sid, "octofs"), 1);
	assert_eq!(decrement_capability_refcount(&sid, "octofs"), 0);
	// Untracked server decrements are safe and report zero
	assert_eq!(decrement_capability_refcount(&sid, "never-seen"), 0);
	clear_capability_refcounts(&sid);
}

#[test]
fn test_skill_capability_servers_take_semantics() {
	let sid = unique("skill-cap");
	set_skill_capability_servers(&sid, "reviewer", vec!["octofs".to_string()]);
	// take removes the mapping
	assert_eq!(
		take_skill_capability_servers(&sid, "reviewer"),
		vec!["octofs".to_string()]
	);
	assert!(take_skill_capability_servers(&sid, "reviewer").is_empty());
	assert!(take_skill_capability_servers(&sid, "unknown-skill").is_empty());
	clear_skill_capability_servers(&sid);
}

#[tokio::test]
async fn test_notification_sender_registry() {
	let sid = unique("notify");
	assert!(get_notification_sender_by_id(&sid).is_none());

	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	set_notification_sender_for_session(&sid, tx);
	assert!(get_notification_sender_by_id(&sid).is_some());

	// send_notification resolves the sender through the task-local id
	with_session_id(sid.clone(), async {
		send_notification(crate::websocket::ServerMessage::Assistant(
			crate::websocket::AssistantPayload {
				content: "ping".to_string(),
				session_id: String::new(),
				step: None,
			},
		));
	})
	.await;
	match rx.try_recv().expect("notification delivered") {
		crate::websocket::ServerMessage::Assistant(payload) => {
			assert_eq!(payload.content, "ping");
		}
		other => panic!("unexpected message: {:?}", other),
	}

	clear_notification_sender_for_session(&sid);
	assert!(get_notification_sender_by_id(&sid).is_none());
}

#[test]
fn test_plan_and_schedule_storage_identity() {
	let sid = unique("storage");
	let plan_a = get_plan_storage(&sid);
	let plan_b = get_plan_storage(&sid);
	assert!(Arc::ptr_eq(&plan_a, &plan_b));
	clear_plan_storage(&sid);
	let plan_c = get_plan_storage(&sid);
	assert!(!Arc::ptr_eq(&plan_a, &plan_c));
	clear_plan_storage(&sid);

	let sched_a = get_schedule_storage(&sid);
	let sched_b = get_schedule_storage(&sid);
	assert!(Arc::ptr_eq(&sched_a, &sched_b));
	clear_schedule_storage(&sid);

	let notify_a = get_schedule_notify(&sid);
	// notify_schedule_change on a registered session must not panic
	notify_schedule_change(&sid);
	let notify_b = get_schedule_notify(&sid);
	assert!(Arc::ptr_eq(&notify_a, &notify_b));
	clear_schedule_notify(&sid);
}

#[test]
fn test_cleanup_session_clears_all_registries() {
	let sid = unique("cleanup");
	set_session_workdir(&sid, PathBuf::from("/x"));
	set_session_role(&sid, "tester");
	push_hint_for_session(&sid, "hint".to_string());
	set_task_start_index(&sid, 3);
	add_active_skill(&sid, "skill");
	increment_capability_refcount(&sid, "srv");

	cleanup_session(&sid);

	assert!(get_current_workdir(&sid).is_none());
	assert!(get_session_role(&sid).is_none());
	assert!(!has_hints_for_session(&sid));
	assert!(get_task_start_index(&sid).is_none());
	assert!(get_active_skills(&sid).is_empty());
}

// ---- SessionContext ----

#[test]
fn session_context_new_and_for_session_populate_fields() {
	let ctx = SessionContext::new(
		"s1".to_string(),
		"developer:general".to_string(),
		"proj".to_string(),
		PathBuf::from("/tmp"),
	);
	assert_eq!(ctx.session_id, "s1");
	assert_eq!(ctx.role, "developer:general");
	assert_eq!(ctx.project_id, "proj");
	assert_eq!(ctx.workdir, PathBuf::from("/tmp"));

	let for_session = SessionContext::for_session("s2", "assistant");
	assert_eq!(for_session.session_id, "s2");
	assert_eq!(for_session.role, "assistant");
	assert_eq!(
		for_session.workdir,
		std::env::current_dir().unwrap_or_default(),
		"for_session anchors the workdir at the current directory"
	);
}

#[tokio::test]
async fn expect_session_id_returns_the_scoped_id() {
	let sid = unique("expect");
	let seen = with_session_id(sid.clone(), async { expect_session_id() }).await;
	assert_eq!(seen, sid);
}

#[test]
#[should_panic(expected = "not in a session context")]
fn expect_session_id_panics_outside_any_scope() {
	// A bare test thread carries no task-local session id.
	let _ = expect_session_id();
}

// ---- notification registry init + aliases ----

#[test]
fn notification_registry_init_is_idempotent_and_aliases_roundtrip() {
	init_notification_registry();
	// A second init must not clear senders registered in between.
	init_notification_registry();

	let sid = unique("alias");
	let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
	register_notification_sender(sid.clone(), tx);
	assert!(get_notification_sender_by_id(&sid).is_some());

	unregister_notification_sender(sid.clone());
	assert!(get_notification_sender_by_id(&sid).is_none());
}

// ---- session config registry ----

#[test]
fn session_config_set_get_clear_roundtrip() {
	let sid = unique("cfg");
	assert!(get_session_config(&sid).is_none());

	let config: Config = toml::from_str(include_str!("../../config-templates/default.toml"))
		.expect("parse default config template");
	set_session_config(&sid, &config);

	let stored = get_session_config(&sid).expect("config stored per session");
	assert_eq!(stored.get_effective_model(), config.get_effective_model());

	clear_session_config(&sid);
	assert!(get_session_config(&sid).is_none());
}

// ---- env skills ----

#[test]
fn env_skills_lifecycle_is_session_scoped_and_idempotent() {
	let sid = unique("envskill");
	assert!(get_env_skills(&sid).is_empty());

	add_env_skill(&sid, "rust");
	add_env_skill(&sid, "rust");
	add_env_skill(&sid, "toml");
	assert_eq!(
		get_env_skills(&sid),
		vec!["rust".to_string(), "toml".to_string()]
	);

	clear_env_skills(&sid);
	assert!(get_env_skills(&sid).is_empty());
}

// ---- job manager ----

#[tokio::test]
async fn job_manager_initializes_resolves_and_clears_per_session() {
	let sid = unique("jobs");
	// No task-local session id on this bare test task.
	assert!(get_job_manager_for_session().is_none());

	init_job_manager_for_session(&sid);
	let inside = with_session_id(sid.clone(), async { get_job_manager_for_session() }).await;
	assert!(
		inside.is_some(),
		"manager must resolve inside the session scope"
	);

	clear_job_manager_for_session(&sid);
	let after = with_session_id(sid.clone(), async { get_job_manager_for_session() }).await;
	assert!(after.is_none(), "clear must remove the manager");
}

// ---- schedule notify ----

#[tokio::test]
async fn notify_schedule_change_wakes_a_registered_waiter() {
	let sid = unique("schednotify");
	let notify = get_schedule_notify(&sid);

	let waiter = notify.notified();
	tokio::pin!(waiter);
	waiter.as_mut().enable();

	notify_schedule_change(&sid);
	tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
		.await
		.expect("waiter must be woken by notify_schedule_change");
}

// ---- init_session_services ----

#[tokio::test]
#[serial_test::serial]
async fn init_session_services_runs_the_full_init_sequence_under_scope() {
	let sid = unique("services");
	with_session_id(sid.clone(), async {
		init_session_services("developer:general");
	})
	.await;
	// The sequence is registry/task-local init; the observable contract is
	// that it completes without panicking and cleanup stays symmetric.
	cleanup_session(&sid);
}
