# MCP Server Development

Add a built-in MCP server when you are extending Octomind itself. This contributor guide covers tool definitions,
routing, role activation, error handling, and verification with a complete example.

## When to Add a New Server

Add a built-in server when you need:

- Deep integration with Octomind internals (session state, config)
- Functionality that doesn't make sense as an external process
- Tools that require access to the MCP coordinator

For external tools, use a configured `stdio` or `http` server; see [MCP Tools](../usage/07-mcp-tools.md). For a project
script, see [Local Tools](../usage/17-local-tools.md).

> Terminology: the codebase defines a tool with the `McpFunction` struct (a `name`, `description`, and JSON-Schema
> `parameters`), and calls the runtime invocation an `McpToolCall` that returns an `McpToolResult`. "Function" is the
> static definition; "tool" is the callable runtime surface.

> Smallest reference to copy: read `src/mcp/core/functions.rs`, `src/mcp/core/recall.rs`, and how the `core` server is
> wired into `src/mcp/mod.rs`. Planning is supervisor-internal and is not a tool-definition example.

## Built-in Servers

Four built-in servers are shipped as `[[mcp.servers]]` entries in `config-templates/default.toml` (`core`,
`orchestration`, `runtime`, `agent`):

| Server | Location | Tools |
|--------|----------|-------|
| `core` | `src/mcp/core/` | `recall` when attention or attention governance is enabled |
| `orchestration` | `src/mcp/orchestration/` | `tap`, `schedule`, `monitor` |
| `runtime` | `src/mcp/runtime/` | `mcp`, `agent`, `skill`, `capability` |
| `agent` | `src/mcp/agent/` | one `agent_<name>` tool per configured agent (built from `config.agents`) |

`filesystem` is referenced by shipped roles but is not declared in the default server array. External tool surfaces come
from the resolved tap/capability or your configuration; inspect their discovered schemas instead of assuming a fixed
tool list (`src/agent/registry.rs`, `src/mcp/server.rs`).

## Step-by-Step Guide

### 1. Create Server Module

The example below adds `text_metrics`, which exposes `text_length` to count Unicode scalar values in a string. These
names are example code to implement, not existing shipped tools.

```text
src/mcp/
  text_metrics/
    mod.rs        # Function definitions + execute fn
```

Start every new Rust file with the repository's Apache 2.0 header.

### 2. Implement `get_all_functions()`

Return a list of `McpFunction` definitions. The `parameters` field is a JSON Schema built with `serde_json::json!`.
`get_recall_function()` in `src/mcp/core/recall.rs` is the smallest current example:

```rust
use crate::mcp::McpFunction;
use serde_json::json;

pub fn get_all_functions() -> Vec<McpFunction> {
    vec![McpFunction {
        name: "text_length".to_string(),
        description: "Count Unicode scalar values in a non-empty string.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to count",
                    "minLength": 1
                }
            },
            "required": ["text"],
            "additionalProperties": false
        }),
    }]
}
```

If your function list depends on configuration, take `&Config` directly as shown under [Config-Dependent
Functions](#config-dependent-functions).

### 3. Register Listing, Routing, and the Tool Map

Add a module declaration and **three** match arms: listing and execution in `src/mcp/mod.rs`, plus mapping in
`src/mcp/tool_map.rs`. Listing-only or execution-only wiring leaves a tool unavailable or unroutable.

**(a)** Declare the module near the other `pub mod` lines (`src/mcp/mod.rs`):

```rust
pub mod text_metrics;
```

**(b)** Add a listing arm in `server_functions_for` so the tool shows up in the system prompt. For a stateless server,
use the cache helper `get_filtered_server_functions`:

```rust
"text_metrics" => {
    get_filtered_server_functions("text_metrics", server.tools(), text_metrics::get_all_functions)
}
```

**(c)** Add an execution arm in `route_builtin_tool` (`src/mcp/mod.rs`) that dispatches to your execute function and
maps a hard error into a soft error result:

```rust
"text_metrics" => {
    let result = text_metrics::execute_tool(call)
        .await
        .map_err(|e| format!("text_metrics tool failed: {}", e));
    match result {
        Ok(mut r) => {
            r.tool_id = call.tool_id.clone();
            Ok(r)
        }
        Err(msg) => Ok(McpToolResult::error(
            call.tool_name.clone(),
            call.tool_id.clone(),
            msg,
        )),
    }
}
```

**(d)** Add the builtin arm in `build_tool_server_map_impl` in `src/mcp/tool_map.rs`:

```rust
"text_metrics" => crate::mcp::get_filtered_server_functions(
    "text_metrics",
    server.tools(),
    crate::mcp::text_metrics::get_all_functions,
),
```

Both maps use the first configured server for a duplicate tool name. Choose a distinct tool name to avoid collisions
(`build_tool_server_map` and `build_tool_server_map_impl`).

### 4. Implement Tool Execution

Execute functions take the `&McpToolCall` (which carries `tool_name`, `parameters`, and `tool_id`) and usually
`&Config`. They return `anyhow::Result<McpToolResult>`. Read parameters off `call.parameters`:

```rust
use crate::mcp::{McpToolCall, McpToolResult};
use anyhow::Result;

pub async fn execute_tool(call: &McpToolCall) -> Result<McpToolResult> {
    match call.tool_name.as_str() {
        "text_length" => {
            let text = match call.parameters.get("text").and_then(|v| v.as_str()) {
                Some(v) if !v.is_empty() => v,
                // Soft (user-facing) failure: return Ok(error result), do NOT bail with Err.
                _ => return Ok(McpToolResult::error(
                    call.tool_name.clone(),
                    call.tool_id.clone(),
                    "text must be a non-empty string".to_string(),
                )),
            };

            Ok(McpToolResult::success(
                call.tool_name.clone(),
                call.tool_id.clone(),
                text.chars().count().to_string(),
            ))
        }
        other => Ok(McpToolResult::error(
            call.tool_name.clone(),
            call.tool_id.clone(),
            format!("Unknown tool: {}", other),
        )),
    }
}
```

Existing references for the exact signatures:

- `pub async fn execute_recall(call: &McpToolCall) -> Result<McpToolResult>` (`src/mcp/core/recall.rs`)
- `pub async fn execute_tap_command(call: &McpToolCall, config: &Config) -> Result<McpToolResult>`
  (`src/mcp/orchestration/tap.rs`)
- `pub async fn execute_runtime_tool(call: &McpToolCall, config: &Config) -> Result<McpToolResult>`
  (`src/mcp/runtime/mod.rs`)

**Error-handling contract:**

- **Soft / user-facing failures** (missing param, bad input, tool-level rejection): return
  `Ok(McpToolResult::error(name, tool_id, msg))`. The model sees the error text and can retry.
- **Unexpected handler failures** should also be converted to `Ok(McpToolResult::error(...))` at the handler boundary.
  `route_builtin_tool` defensively wraps a returned `Err`, but a tool implementation should not rely on that fallback
  for normal failures.

### 5. Add Config Registration

Register your server in `config-templates/default.toml` when shipping it. For a development run, add the following to a
sibling `mcp-text-metrics.toml` in your config directory after compiling the new handler. `auto_bind` enables it for the
exact role tag shown:

```toml
[[mcp.servers]]
name = "text_metrics"
type = "builtin"
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

`tools = []` means **all** tools from this server are exposed. A non-empty array filters by exact name or trailing-`*`
prefix pattern (`is_tool_allowed_by_patterns`), e.g. `tools = ["text_*"]`. For external servers, the runtime overlay
(the `capability` tool with `action = "enable"`) can additionally unlock tools that the static filter would otherwise
hide.

Alternatively, bind the server through a role's `server_refs` and `allowed_tools`. Add this new role to a sibling
`roles.toml` in your config directory:

```toml
[[roles]]
name = "text_counter"
system = "Use text_length to count the supplied text."
welcome = "Send text to count."

[roles.mcp]
server_refs = ["text_metrics"]
allowed_tools = ["text_metrics:*"]
```

```bash
octomind run text_counter
```

A server declaration alone does not enable it for every role. Do not add a partial duplicate `[[roles]]` entry:
same-name entries replace the whole role.

```bash
octomind config --validate
octomind run developer:general
```

In that session:

```text
/mcp
Use text_length to count the characters in "hello".
```

The handler receives this internal `McpToolCall` shape and returns text `5`:

```json
{"tool_name":"text_length","parameters":{"text":"hello"},"tool_id":"demo-1"}
```

### 6. Surface Misuse Hints (Optional)

There is no static hint table to declare. To nudge the model when it misuses a tool, push a hint imperatively from
inside your execute function:

```rust
if crate::mcp::tool_map::get_server_for_tool("text_length").is_some() {
    crate::mcp::hint_accumulator::push_hint(
        "Use text_length when you need a Unicode scalar count of supplied text.",
    );
}
```

Hints are session-scoped, deduplicated, and drained once per tool round by the session layer (`drain_hints()`), then
injected as a single user-role message — so they guide the model without polluting individual tool-result strings
(`src/mcp/hint_accumulator.rs`).

## Protocol Compliance

All tools must follow the MCP protocol:

- Return `McpToolResult::error(...)` instead of panicking.
- Validate all parameters with clear error messages.
- Handle missing/empty/wrong-type parameters gracefully.
- Long-running tools may accept `cancellation_token: Option<tokio::sync::watch::Receiver<bool>>` (as the `agent` server
  does). Otherwise cancellation is enforced centrally by `try_execute_tool_call`, which races your future against the
  cancel signal via `tokio::select!`. `execute_recall` and `execute_tap_command` take no token.
- `McpToolResult` is an internal wrapper with `tool_name`, `tool_id`, and a nested rmcp `CallToolResult`.
  Return the success/error helpers; the nested protocol result carries content blocks and an optional `isError`.
  Builtin dispatch is in-process, so this wrapper is not itself an external JSON-RPC response.

### Returning metadata alongside text

To attach structured metadata to a successful result, use `McpToolResult::success_with_metadata(name, tool_id, text,
json_value)`. The metadata is stored as the result's `structured_content`; `extract_content()` appends it to the text as
a `[Metadata: ...]` block (`src/mcp/mod.rs`). For example, this can replace the success return inside `text_length`:

```rust
Ok(McpToolResult::success_with_metadata(
    call.tool_name.clone(),
    call.tool_id.clone(),
    text.chars().count().to_string(),
    serde_json::json!({"unit": "unicode_scalar", "count": text.chars().count()}),
))
```

## Testing

Build a real `McpToolCall` and pass it to your execute function. Keep test bodies in a sibling file, matching the
repository convention. The production module only declares it:

```rust
#[cfg(test)]
#[path = "text_metrics_tests.rs"]
mod tests;
```

In `text_metrics_tests.rs`, prepend the same license header and check both successful work and invalid parameters.
`is_error()` is a method:

```rust
use super::*;
use crate::mcp::McpToolCall;

#[tokio::test]
async fn counts_unicode_scalars() {
    let call = McpToolCall {
        tool_name: "text_length".to_string(),
        parameters: serde_json::json!({"text": "aé"}),
        tool_id: "test-id".to_string(),
    };
    let result = execute_tool(&call).await.unwrap();
    assert!(!result.is_error());
    assert!(matches!(
        &result.result.content[0],
        rmcp::model::ContentBlock::Text(text) if text.text == "2"
    ));
    assert_eq!(result.tool_id, "test-id");
}

#[tokio::test]
async fn invalid_text_is_a_tool_error() {
    for parameters in [
        serde_json::json!({}),
        serde_json::json!({"text": ""}),
        serde_json::json!({"text": 42}),
    ] {
        let call = McpToolCall {
            tool_name: "text_length".to_string(),
            parameters,
            tool_id: "test-id".to_string(),
        };
        let result = execute_tool(&call).await.unwrap();
        assert!(result.is_error());
        assert_eq!(result.extract_content(), "text must be a non-empty string");
    }
}
```

After adding the example module and wiring, run:

```bash
cargo test mcp::text_metrics::tests
cargo check --all-targets --all-features
```

Also verify the tool is listed and routed through the configured role using the session example above; handler unit
tests alone do not exercise all three registration arms.

## Common Questions

| Symptom | Check |
|---------|-------|
| Tool never appears | Both listing arms, role `server_refs`/`auto_bind`, and tool filters |
| Listed tool cannot execute | `route_builtin_tool` and `build_tool_server_map_impl` |
| Wrong server handles the call | Duplicate tool names; the first configured server wins |
| `recall` is absent | Both attention and governance are disabled, or the role excludes `core` |
| External server fails to start | Command/URL, missing environment substitutions, and captured stderr |

Use the session diagnostics before changing routing:

```text
/mcp
/loglevel debug
```

## Reference Patterns

### Config-Dependent Functions

When a server's function list depends on config, implement `get_all_functions(config: &Config)` directly — there is no
separate wrapper. The `agent` server is the canonical example: it maps over `config.agents` to produce one
`agent_<name>` tool each (`src/mcp/agent/functions.rs`), and is wired in `server_functions_for` as:

```rust
"agent" => {
    let fns = agent::get_all_functions(config);
    filter_tools_by_patterns(fns, server.tools())
}
```

### Function Caching

Stateless built-in function lists are memoized through `get_filtered_server_functions(...)`, which caches per
`server_type` + allowed-tools key in `INTERNAL_FUNCTION_CACHE` (`src/mcp/mod.rs`). The current `runtime` and
`orchestration` lists use this cache. Config-dependent servers (`core` and `agent`) skip it and call
`get_all_functions(config)` each time. `clear_function_cache()` empties the cache for tests or configuration changes.

### Timeouts and Cancellation

`timeout_seconds` is a server field, not `config.timeout`. External MCP calls use an idle deadline refreshed by progress
notifications in `src/mcp/client.rs`. Builtin routing supplies cancellation, but does not automatically wrap every
handler in that server timeout. External calls also have an absolute cap of twenty times the idle timeout. A builtin
that owns a long operation must implement its deadline and return a soft error on expiry; copy its duration from the
resolved server configuration.

This wrapper is a complete example around the handler above; call it from the execution arm if you need that policy:

```rust
pub async fn execute_with_timeout(
    call: &crate::mcp::McpToolCall,
    config: &crate::config::Config,
) -> anyhow::Result<crate::mcp::McpToolResult> {
    use crate::mcp::McpToolResult;
    let Some(server) = config.get_server_config("text_metrics") else {
        return Ok(McpToolResult::error(
            call.tool_name.clone(), call.tool_id.clone(), "text_metrics is not configured".into(),
        ));
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(server.timeout_seconds()),
        execute_tool(call),
    ).await {
        Ok(result) => result,
        Err(_) => Ok(McpToolResult::error(
            call.tool_name.clone(), call.tool_id.clone(), "text_metrics timed out".into(),
        )),
    }
}
```

### Session-Ownership Checks for Dynamic Tools

If you add dynamic or runtime-registered tools (e.g. `agent_*` tools or dynamic servers), be aware that
`execute_tool_without_cancellation` (`src/mcp/mod.rs`) enforces session ownership: the global tool map can contain tools
registered by other sessions, so in a session context it verifies that a dynamic tool either is config-defined or
belongs to the current session, returning a "belongs to another session" error otherwise. Static built-in tools
(`recall`, `tap`, `schedule`, etc.) and project-local tools under the synthetic `local` server bypass this check.

### Server Stderr Capture

For stdio servers, stderr lines are drained into a private `SERVER_STDERR` map (`Arc<RwLock<HashMap<String,
StderrBuffer>>>` in `src/mcp/process.rs`) by background reader tasks and surfaced in initialization diagnostics. There
is no public `get_server_stderr` accessor.

### Initialization Progress

To surface init progress to the UI, pass a callback to `initialize_servers_for_role_with_callback(config,
Some(&callback))` (`src/mcp/mod.rs`). It receives `McpInitProgress::Starting { servers }` before initialization and
`McpInitProgress::Completed { server, success, function_count }` as each external server finishes. Builtins need no
process startup. For example, inside an async function returning `anyhow::Result<()>`:

```rust
let callback = |progress: crate::mcp::McpInitProgress| {
    crate::log_debug!("MCP initialization: {:?}", progress);
};
crate::mcp::initialize_servers_for_role_with_callback(config, Some(&callback)).await?;
```

## See also

- [Architecture](02-architecture.md)
- [Building from Source](01-building-from-source.md)
- [MCP Tools](../usage/07-mcp-tools.md)
- [Local Tools](../usage/17-local-tools.md)
- [Configuration Reference](../reference/03-config-reference.md)
