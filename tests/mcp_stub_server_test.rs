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

	// A JSON-RPC error from the server surfaces as a call_tool error, not
	// a hang or a fabricated success
	let exploding = McpToolCall {
		tool_name: "explode".to_string(),
		parameters: serde_json::json!({}),
		tool_id: "t2".to_string(),
	};
	let error = octomind::mcp::client::call_tool(&server, &exploding, None)
		.await
		.expect_err("server error must propagate");
	assert!(
		error.to_string().contains("exploded"),
		"error should carry the server message: {error}"
	);

	// Connected state is tracked, and disconnect tears it down
	assert!(octomind::mcp::client::is_connected(name));
	octomind::mcp::client::disconnect(name);
	assert!(!octomind::mcp::client::is_connected(name));

	// Instructions for a server that never connected are absent
	assert!(octomind::mcp::process::get_server_instructions("__mcp_stub_nope").is_none());
}

/// A tool that outlives the server's timeout must produce a client-side
/// error in bounded time — never a hang, never a fabricated success.
#[tokio::test]
async fn stub_stdio_server_call_timeout() {
	let name = "stub_timeout";
	let script = concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/tests/fixtures/mcp_stub_server.py"
	);
	let server = McpServerConfig::stdin(name, "python3", vec![script.to_string()], 2, Vec::new());

	let call = McpToolCall {
		tool_name: "sleep".to_string(),
		parameters: serde_json::json!({}),
		tool_id: "t-sleep".to_string(),
	};
	let started = std::time::Instant::now();
	let result = octomind::mcp::client::call_tool(&server, &call, None).await;
	assert!(result.is_err(), "sleeping tool call must fail on timeout");
	assert!(
		started.elapsed() < std::time::Duration::from_secs(15),
		"timeout must fire well before the tool's 20s sleep"
	);
	octomind::mcp::client::disconnect(name);
}
