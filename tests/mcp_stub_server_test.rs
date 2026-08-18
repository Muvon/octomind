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

//! End-to-end test of the MCP client/process layer against a real spawned
//! stdio server (tests/fixtures/mcp_stub_server.py). Exercises the pieces
//! unit tests cannot: process spawn, the rmcp initialize handshake,
//! capability storage, tool listing, a tool-call round trip, and
//! disconnect/cleanup.

use octomind::config::McpServerConfig;
use octomind::mcp::McpToolCall;

fn stub_server(name: &str) -> McpServerConfig {
	let script = concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/tests/fixtures/mcp_stub_server.py"
	);
	McpServerConfig::stdin(name, "python3", vec![script.to_string()], 30, Vec::new())
}

#[tokio::test]
async fn stub_stdio_server_end_to_end() {
	let name = "stub_e2e";
	let server = stub_server(name);

	// Connect (spawns the child process) + handshake + list tools
	let tools = octomind::mcp::client::list_tools(&server)
		.await
		.expect("list_tools against stub server");
	assert!(
		tools.iter().any(|t| t.name == "echo"),
		"stub must advertise the echo tool, got: {:?}",
		tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
	);

	// The handshake stored the negotiated server info + instructions
	assert_eq!(
		octomind::mcp::process::get_server_instructions(name).as_deref(),
		Some("stub server instructions")
	);

	// Tool call round trip: the argument comes back as text content
	let call = McpToolCall {
		tool_name: "echo".to_string(),
		parameters: serde_json::json!({"msg": "round-trip"}),
		tool_id: "t1".to_string(),
	};
	let result = octomind::mcp::client::call_tool(&server, &call, None)
		.await
		.expect("call_tool against stub server");
	let text: String = result
		.content
		.iter()
		.filter_map(|block| match block {
			rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
			_ => None,
		})
		.collect();
	assert_eq!(text, "round-trip");
	assert!(!result.is_error.unwrap_or(false));

	// Connected state is tracked, and disconnect tears it down
	assert!(octomind::mcp::client::is_connected(name));
	octomind::mcp::client::disconnect(name);
	assert!(!octomind::mcp::client::is_connected(name));
}
