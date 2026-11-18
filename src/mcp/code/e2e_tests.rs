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

//! End-to-end tests of the `code` tool: a real stdio MCP server process (a
//! python stub, like the ones in `src/mcp/server_tests.rs`), the real tool
//! map, the type checker, the script engine and guardrail admission.
//! Like the neighbouring stdio tests they need `python3` on PATH.

use crate::config::{Config, McpServerConfig};
use crate::mcp::{
	execute_tool_call, get_available_functions, get_model_functions, tool_map, McpToolCall, McpToolResult,
};
use serde_json::json;
use serial_test::serial;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SERVER: &str = "e2e_stub";
const CALLS_FILE_ENV: &str = "STUB_CALLS_FILE";
const SERVER_TIMEOUT_SECONDS: u64 = 10;
const CORE_TIMEOUT_SECONDS: u64 = 30;
const REJECTION_PREFIX: &str = "Script rejected by the type check before it ran; no tool was called.";

/// Newline-delimited JSON-RPC MCP server over stdio with two tools: `greet`
/// answers, `explode` fails. Every `tools/call` appends the tool name to the
/// file named by `STUB_CALLS_FILE`, so a test can tell whether a tool ran.
const STUB_SERVER: &str = r#"
import json, os, sys

def result(rid, res):
    return json.dumps({"jsonrpc": "2.0", "id": rid, "result": res})

TOOLS = [
    {"name": "greet", "description": "greet someone by name", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}},
    {"name": "explode", "description": "always fails", "inputSchema": {"type": "object", "properties": {}}},
]

def handle(req):
    method = req.get("method")
    rid = req.get("id")
    if method == "server/discover":
        return result(rid, {"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": {}, "ttlMs": 0, "cacheScope": "private"})
    if method == "initialize":
        return result(rid, {"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "octomind-e2e-stub", "version": "1.0"}, "instructions": "stub"})
    if method == "tools/list":
        return result(rid, {"tools": TOOLS})
    if method == "tools/call":
        params = req.get("params") or {}
        name = params.get("name")
        with open(os.environ["STUB_CALLS_FILE"], "a") as calls:
            calls.write(name + "\n")
        if name == "greet":
            greeting = "hello " + (params.get("arguments") or {}).get("name", "")
            return result(rid, {"content": [{"type": "text", "text": greeting}], "isError": False})
        return result(rid, {"content": [{"type": "text", "text": "stub refused: boom"}], "isError": True})
    if method == "ping":
        return result(rid, {})
    if rid is not None:
        return json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "method not found"}})
    return None

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except ValueError:
        continue
    out = handle(req)
    if out is not None:
        sys.stdout.write(out + "\n")
        sys.stdout.flush()
"#;

/// The stub's files: its script and the list of tool calls it received.
struct Stub {
	dir: tempfile::TempDir,
}

impl Stub {
	fn new() -> Self {
		let dir = tempfile::tempdir().expect("tempdir");
		std::fs::write(dir.path().join("stub.py"), STUB_SERVER).expect("write stub script");
		Self { dir }
	}

	fn calls_file(&self) -> PathBuf {
		self.dir.path().join("calls")
	}

	/// Tool names the stub received, in order; empty when it was never called.
	fn calls(&self) -> Vec<String> {
		match std::fs::read_to_string(self.calls_file()) {
			Ok(text) => text.lines().map(str::to_string).collect(),
			Err(_) => Vec::new(),
		}
	}

	/// The builtin `core` server (it carries `code`) and the stub as an
	/// external server.
	fn config(&self) -> Config {
		let mut config: Config = toml::from_str(include_str!("../../../config-templates/default.toml"))
			.expect("parse default config template");
		config.mcp.servers = vec![
			McpServerConfig::builtin("core", CORE_TIMEOUT_SECONDS, vec![]),
			McpServerConfig::Stdin {
				name: SERVER.to_string(),
				command: "python3".to_string(),
				args: vec![self.dir.path().join("stub.py").to_string_lossy().into_owned()],
				timeout_seconds: SERVER_TIMEOUT_SECONDS,
				tools: vec![],
				env: HashMap::from([(
					CALLS_FILE_ENV.to_string(),
					self.calls_file().to_string_lossy().into_owned(),
				)]),
				cwd: None,
				auto_bind: None,
			},
		];
		config
	}
}

fn code_call(script: &str) -> McpToolCall {
	McpToolCall {
		tool_name: super::TOOL_NAME.to_string(),
		parameters: json!({ "script": script }),
		tool_id: "id-code".to_string(),
	}
}

/// One `code` call, the way the session loop makes it. Scripts run only
/// inside a session, so the caller is in one.
async fn call_code(config: &Config, script: &str) -> McpToolResult {
	let (result, _elapsed) = execute_tool_call(&code_call(script), config, None)
		.await
		.expect("the code tool reports script failures as results");
	result
}

/// One `code` call in a session of its own.
async fn run_script(config: &Config, script: &str) -> McpToolResult {
	let session = format!("code-e2e-{}", uuid::Uuid::new_v4());
	let result = crate::session::context::with_session_id(session.clone(), async {
		crate::session::context::init_session_services("assistant");
		call_code(config, script).await
	})
	.await;
	crate::session::context::cleanup_session(&session);
	result
}

fn names(functions: &[crate::mcp::McpFunction]) -> Vec<&str> {
	functions.iter().map(|function| function.name.as_str()).collect()
}

/// Start from a clean function cache and the real tool map for `config`.
async fn start(config: &Config) {
	crate::mcp::clear_function_cache();
	// Production order: a stdio server lists its tools only once it runs.
	crate::mcp::initialize_servers_for_role(config)
		.await
		.expect("start servers");
	tool_map::initialize_tool_map(config).await.expect("init tool map");
}

fn stop() {
	// Starting servers also starts the process-wide health monitor; left
	// running it would watch this stub through every later test.
	crate::mcp::health_monitor::stop_health_monitor();
	crate::mcp::client::disconnect(SERVER);
}

#[serial]
#[tokio::test]
async fn the_model_sees_the_code_tool_and_none_of_the_stub_tools() {
	let stub = Stub::new();
	let config = stub.config();
	start(&config).await;

	let model = get_model_functions(&config).await;
	let model_names = names(&model);
	assert!(model_names.contains(&"code"), "{model_names:?}");
	assert!(!model_names.contains(&"greet"), "{model_names:?}");
	assert!(!model_names.contains(&"explode"), "{model_names:?}");

	let code = model
		.iter()
		.find(|function| function.name == "code")
		.expect("code tool");
	assert!(
		code.description
			.contains(&format!("Tools, called like `await {SERVER}.greet({{...}})`:")),
		"{}",
		code.description
	);
	assert!(
		code.description.contains(&format!("{SERVER}.greet(")),
		"{}",
		code.description
	);
	assert!(
		code.description.contains(&format!("{SERVER}.explode(")),
		"{}",
		code.description
	);

	let available = get_available_functions(&config).await;
	let available_names = names(&available);
	assert!(available_names.contains(&"greet"), "{available_names:?}");
	assert!(available_names.contains(&"explode"), "{available_names:?}");
	stop();
}

/// A script tool called as a tool of its own, by either name, becomes a call
/// of that tool when its arguments fit and is answered when they do not. The
/// model is shown the tool's description once, until its context loses it.
#[serial]
#[tokio::test]
async fn a_direct_call_of_a_script_tool_runs_and_brings_its_description() {
	let stub = Stub::new();
	let config = stub.config();
	start(&config).await;

	let call = |name: &str, parameters: serde_json::Value| McpToolCall {
		tool_name: name.to_string(),
		parameters,
		tool_id: "id".to_string(),
	};
	let listed = format!("{SERVER}.greet");
	let description =
		format!("{SERVER}.greet({{name: string}})\n\ngreet someone by name\n\nArguments:\n  name: string");
	let unfit =
		format!("`{listed}` was not called: missing required argument `name`; unknown argument `nam` (known: name)");
	let session = format!("code-e2e-{}", uuid::Uuid::new_v4());
	crate::session::context::with_session_id(session.clone(), async {
		crate::session::context::init_session_services("assistant");
		let (calls, answers) = super::settle_direct_calls(
			&config,
			vec![
				call("greet", json!({"name": "ada"})),
				call(&listed, json!({"name": "ada"})),
				call(&listed, json!({"nam": "ada"})),
				call("code", json!({"script": "1"})),
				call("nope", json!({})),
			],
		)
		.await;
		let called: Vec<&str> = calls.iter().map(|call| call.tool_name.as_str()).collect();
		assert_eq!(called, vec!["greet", "greet", listed.as_str(), "code", "nope"]);
		assert_eq!(answers, vec![None, None, Some(unfit.clone()), None, None]);
		assert_eq!(
			crate::mcp::hint_accumulator::drain_hints(),
			vec![format!(
				"You called `greet` without its description. Read it before the next call:\n{description}"
			)]
		);

		// The context lost what it was shown: the description comes again,
		// here with the answer of a call that does not fit.
		crate::session::context::bump_context_epoch();
		let (_, answers) = super::settle_direct_calls(&config, vec![call(&listed, json!({"nam": "ada"}))]).await;
		assert_eq!(answers, vec![Some(format!("{unfit}\n\n{description}"))]);
		assert!(crate::mcp::hint_accumulator::drain_hints().is_empty());
	})
	.await;
	crate::session::context::cleanup_session(&session);
	stop();
}

#[serial]
#[tokio::test]
async fn a_script_calls_a_stub_tool_and_returns_its_output() {
	let stub = Stub::new();
	let config = stub.config();
	start(&config).await;

	let result = run_script(&config, &format!("{SERVER}.greet({{name: 'ada'}})")).await;
	let text = result.extract_content();
	assert!(!result.is_error(), "{text}");
	assert!(text.contains("hello ada"), "{text}");
	assert_eq!(stub.calls(), vec!["greet"]);
	stop();
}

#[serial]
#[tokio::test]
async fn a_type_error_rejects_the_whole_script_and_no_tool_runs() {
	let stub = Stub::new();
	let config = stub.config();
	start(&config).await;

	let script = format!("await {SERVER}.greet({{name: 'ada'}});\nawait {SERVER}.greet({{nam: 'ada'}});");
	let result = run_script(&config, &script).await;
	let text = result.extract_content();
	assert!(result.is_error(), "{text}");
	assert!(text.starts_with(REJECTION_PREFIX), "{text}");
	assert!(
		stub.calls().is_empty(),
		"the valid first call must not run: {:?}",
		stub.calls()
	);
	stop();
}

#[serial]
#[tokio::test]
async fn a_failing_tool_call_stops_the_script_and_lists_what_ran() {
	let stub = Stub::new();
	let config = stub.config();
	start(&config).await;

	let script = format!("print(await {SERVER}.greet({{name: 'ada'}}));\nawait {SERVER}.explode({{}});");
	let result = run_script(&config, &script).await;
	let text = result.extract_content();
	assert!(result.is_error(), "{text}");
	// The reason leads and names the call that failed; what the script
	// printed before it follows.
	assert_eq!(
		text,
		format!(
			"Error: {SERVER}.explode failed: stub refused: boom\n  at line 2\n\
			 Tool calls made before the script stopped: {SERVER}.greet, {SERVER}.explode\n\n\
			 Printed before the script stopped:\nhello ada"
		)
	);
	assert_eq!(stub.calls(), vec!["greet", "explode"]);
	stop();
}

#[serial]
#[tokio::test]
async fn direct_tools_wildcard_turns_the_code_tool_off() {
	let stub = Stub::new();
	let mut config = stub.config();
	config.mcp.direct_tools = vec!["*".to_string()];
	start(&config).await;

	let model = get_model_functions(&config).await;
	let model_names = names(&model);
	assert!(!model_names.contains(&"code"), "{model_names:?}");
	assert!(model_names.contains(&"greet"), "{model_names:?}");
	assert!(model_names.contains(&"explode"), "{model_names:?}");
	stop();
}

fn write_guardrails(project: &Path, rules: &str) {
	std::fs::create_dir_all(project.join(".agents")).expect(".agents");
	std::fs::write(project.join(".agents/guardrails.toml"), rules).expect("write guardrails.toml");
}

#[serial]
#[tokio::test]
async fn a_guardrail_denial_makes_the_script_call_fail_and_the_tool_does_not_run() {
	let stub = Stub::new();
	let config = stub.config();
	let project = tempfile::tempdir().expect("project dir");
	let session = format!("code-e2e-{}", uuid::Uuid::new_v4());
	// A guard matches a capability, and a tool belongs to one only through
	// the runtime overlay: name a capability after the session for the stub's
	// `greet` tool.
	write_guardrails(
		project.path(),
		&format!("[[guard]]\nmatch = '{session}'\nmessage = 'stub greetings forbidden'\n"),
	);
	crate::config::runtime_overlay::set_capability_extras(
		&session,
		HashMap::from([(SERVER.to_string(), vec!["greet".to_string()])]),
	);

	crate::session::context::with_session_id(session.clone(), async {
		crate::mcp::set_session_working_directory(project.path().to_path_buf());
		tool_map::isolate_session_tool_map(&session);
		crate::mcp::clear_function_cache();
		crate::session::context::init_session_services("assistant");
		crate::mcp::initialize_servers_for_role(&config)
			.await
			.expect("start servers");
		tool_map::initialize_tool_map(&config)
			.await
			.expect("init session tool map");

		let result = call_code(&config, &format!("{SERVER}.greet({{name: 'ada'}})")).await;
		let text = result.extract_content();
		assert!(result.is_error(), "{text}");
		assert!(text.contains("stub greetings forbidden"), "{text}");
		assert!(
			stub.calls().is_empty(),
			"a denied call must not reach the server: {:?}",
			stub.calls()
		);
	})
	.await;

	crate::config::runtime_overlay::set_capability_extras(&session, HashMap::new());
	crate::session::context::cleanup_session(&session);
	stop();
}
