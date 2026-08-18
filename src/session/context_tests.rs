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
