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
		model: None,
		temperature: None,
		top_p: None,
		top_k: None,
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
		model: None,
		temperature: None,
		top_p: None,
		top_k: None,
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
