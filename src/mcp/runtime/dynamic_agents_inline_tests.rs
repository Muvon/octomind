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
use std::sync::Mutex;

// Serialize all tests that mutate the global DYNAMIC_AGENT_MANAGER to prevent
// race conditions when tests run in parallel (RUST_TEST_THREADS > 1).
static TEST_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_agent_register_enable_disable() {
	let _guard = TEST_MUTEX.lock().unwrap();
	clear_all();

	let agent = DynamicAgentConfig {
		name: "test_agent".to_string(),
		description: "Test agent".to_string(),
		system: "You are a test agent.".to_string(),
		welcome: String::new(),
		model: Default::default(),
		server_refs: vec![],
		allowed_tools: vec![],
		workdir: ".".to_string(),
	};

	// Register
	register_agent(agent.clone()).unwrap();

	// List shows registered but not enabled
	let agents = list_agents();
	assert_eq!(agents.len(), 1);
	assert_eq!(agents[0].0.name, "test_agent");
	assert!(!agents[0].1); // not enabled

	// Enable
	enable_agent("test_agent").unwrap();
	let agents = list_agents();
	assert!(agents[0].1); // now enabled

	// Disable
	disable_agent("test_agent").unwrap();
	let agents = list_agents();
	assert!(!agents[0].1); // disabled again

	// Remove
	remove_agent("test_agent");
	let agents = list_agents();
	assert!(agents.is_empty());
}

#[test]
fn test_duplicate_agent() {
	let _guard = TEST_MUTEX.lock().unwrap();
	clear_all();

	let agent = DynamicAgentConfig {
		name: "dup_test".to_string(),
		description: "Test".to_string(),
		system: "You are a test.".to_string(),
		welcome: String::new(),
		model: Default::default(),
		server_refs: vec![],
		allowed_tools: vec![],
		workdir: ".".to_string(),
	};

	register_agent(agent.clone()).unwrap();
	let result = register_agent(agent);
	assert!(result.is_err());
}

#[test]
fn test_agent_function_definition() {
	let func = get_agent_tool_function();
	assert_eq!(func.name, "agent");
	assert!(func.parameters.get("properties").is_some());
}
fn fixture_agent(name: &str) -> DynamicAgentConfig {
	DynamicAgentConfig {
		name: name.to_string(),
		description: "Fixture agent".to_string(),
		system: "You are a fixture agent.".to_string(),
		welcome: String::new(),
		model: Default::default(),
		server_refs: vec![],
		allowed_tools: vec![],
		workdir: ".".to_string(),
	}
}

#[test]
fn test_agent_register_validation() {
	let _guard = TEST_MUTEX.lock().unwrap();
	clear_all();

	let err = register_agent(fixture_agent("")).expect_err("blank name must bail");
	assert!(err.to_string().contains("name is required"));

	let mut blank_system = fixture_agent("__dynagent_blank_system");
	blank_system.system = String::new();
	let err = register_agent(blank_system).expect_err("blank system must bail");
	assert!(err.to_string().contains("system prompt is required"));
}

#[test]
fn test_get_all_configs_and_functions_track_enabled_only() {
	let _guard = TEST_MUTEX.lock().unwrap();
	clear_all();

	register_agent(fixture_agent("__dynagent_a")).unwrap();
	register_agent(fixture_agent("__dynagent_b")).unwrap();
	enable_agent("__dynagent_a").unwrap();

	let enabled = get_all_configs();
	assert_eq!(enabled.len(), 1);
	assert_eq!(enabled[0].name, "__dynagent_a");

	let funcs = get_all_functions();
	assert_eq!(funcs.len(), 1);
	assert_eq!(funcs[0].name, "agent___dynagent_a");
	assert!(funcs[0].parameters.get("properties").is_some());

	// Disabled and unknown agents are not resolvable as enabled.
	disable_agent("__dynagent_a").unwrap();
	assert!(get_enabled_agent("__dynagent_a").is_none());
	assert!(get_enabled_agent("__dynagent_nope").is_none());
	assert!(get_all_configs().is_empty());
	assert!(get_all_functions().is_empty());
}

#[test]
fn test_dynamic_agent_tool_lookup_helpers() {
	let _guard = TEST_MUTEX.lock().unwrap();
	clear_all();

	register_agent(fixture_agent("__dynagent_lookup")).unwrap();
	assert!(is_dynamic_by_tool("agent___dynagent_lookup"));
	assert!(!is_dynamic_by_tool("agent_unknown_agent"));
	assert!(!is_dynamic_by_tool("notagent___dynagent_lookup"));
	assert_eq!(
		get_dynamic_agent_name_by_tool("agent___dynagent_lookup"),
		Some("__dynagent_lookup".to_string())
	);
	assert_eq!(
		get_dynamic_agent_name_by_tool("agent___dynagent_nope"),
		None
	);
	assert_eq!(get_dynamic_agent_name_by_tool("shell"), None);

	// Removing an unknown agent is a None, not an error.
	assert!(remove_agent("__dynagent_nope").is_none());
}

#[tokio::test]
async fn test_agent_session_scoped_registry_branches() {
	let sid = "__dynagent_session".to_string();
	crate::session::context::with_session_id(sid.clone(), async {
		// register/enable/disable/remove/list all take the session branch.
		register_agent(fixture_agent("__dynagent_sess")).expect("session register");
		assert!(is_dynamic("__dynagent_sess"));
		assert_eq!(list_agents().len(), 1);

		enable_agent("__dynagent_sess").expect("session enable");
		assert!(is_enabled("__dynagent_sess"));
		assert!(get_enabled_agent("__dynagent_sess").is_some());
		assert_eq!(get_all_configs().len(), 1);

		// Unknown agent inside a session errors through the session branch.
		let err = enable_agent("__dynagent_sess_nope").expect_err("must bail");
		assert!(err.to_string().contains("not registered"));
		let err = disable_agent("__dynagent_sess_nope").expect_err("must bail");
		assert!(err.to_string().contains("not found"));

		disable_agent("__dynagent_sess").expect("session disable");
		assert!(!is_enabled("__dynagent_sess"));

		let removed = remove_agent("__dynagent_sess");
		assert!(removed.is_some());
		assert!(!is_dynamic("__dynagent_sess"));
	})
	.await;
	crate::session::context::cleanup_session(&sid);
}
