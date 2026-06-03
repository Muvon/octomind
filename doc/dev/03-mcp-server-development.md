# MCP Server Development

Guide for adding new built-in MCP servers to Octomind.

> Terminology: the codebase defines a tool with the `McpFunction` struct (a `name`, `description`, and a JSON-Schema `parameters` value), and calls the runtime invocation an `McpToolCall` that returns an `McpToolResult`. "Function" = the static definition, "tool" = the thing the model calls. This guide uses both deliberately.

## When to Add a New Server

Add a built-in server when you need:
- Deep integration with Octomind internals (session state, config)
- Functionality that doesn't make sense as an external process
- Tools that require access to the MCP coordinator

For external tools, prefer configuring a `stdio` or `http` server in config.

> Smallest reference to copy: read `src/mcp/core/functions.rs` (which collects two functions) together with `src/mcp/core/plan/` and how the `core` server is wired into `src/mcp/mod.rs`. Every code block below is modeled on that real `plan` tool.

## Built-in Servers

Three built-in servers are shipped as `[[mcp.servers]]` entries in `config-templates/default.toml` (`core`, `runtime`, `agent`):

| Server | Location | Tools |
|--------|----------|-------|
| `core` | `src/mcp/core/` | `plan`, `tap` |
| `runtime` | `src/mcp/runtime/` | `mcp`, `agent`, `skill`, `schedule`, `capability` |
| `agent` | `src/mcp/agent/` | one `agent_<name>` tool per configured agent (built from `config.agents`) |

External:
| Server | Type | Purpose |
|--------|------|---------|
| `filesystem` (octofs) | stdio | `view`, `text_editor`, `batch_edit`, `extract_lines`, `shell`, `ast_grep`, `list_files`, `workdir` |

> `filesystem` is **not** declared in `default.toml`'s `[[mcp.servers]]`. It is supplied by a tap and referenced by name in roles' `server_refs` / `allowed_tools`. Only `core`, `runtime`, and `agent` are shipped as built-in server entries.

## Step-by-Step Guide

### 1. Create Server Module

```
src/mcp/
  your_server/
    mod.rs        # Function definitions + execute fn
```

### 2. Implement `get_all_functions()`

Return a list of `McpFunction` definitions. The `parameters` field is a JSON Schema built with `serde_json::json!`. Model it on `get_plan_function()` (`src/mcp/core/plan/mod.rs`):

```rust
use crate::mcp::McpFunction;
use serde_json::json;

pub fn get_all_functions() -> Vec<McpFunction> {
    vec![McpFunction {
        name: "your_tool".to_string(),
        description: "Description shown to the model".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "param1": {
                    "type": "string",
                    "description": "Parameter description"
                }
            },
            "required": ["param1"],
            "additionalProperties": false
        }),
    }]
}
```

If your function list depends on configuration, take `&Config` directly instead — that is the real config-dependent pattern (see [Config-Dependent Functions](#config-dependent-functions)):

```rust
pub fn get_all_functions(config: &crate::config::Config) -> Vec<McpFunction> { /* ... */ }
```

### 3. Register in `src/mcp/mod.rs` (TWO match arms)

This is the most error-prone step: a new built-in server must be wired into **two** separate match arms in `src/mcp/mod.rs`, plus a module declaration. Listing-only or execution-only wiring is a silent bug.

**(a)** Declare the module near the other `pub mod` lines (`src/mcp/mod.rs`):

```rust
pub mod your_server;
```

**(b)** Add a listing arm in `server_functions_for` so the tool shows up in the system prompt. For a stateless server, use the cache helper `get_filtered_server_functions`:

```rust
"your_server" => {
    get_filtered_server_functions("your_server", server.tools(), your_server::get_all_functions)
}
```

If your function list is config-dependent, call it directly and filter (like the `agent` arm):

```rust
"your_server" => {
    let fns = your_server::get_all_functions(config);
    filter_tools_by_patterns(fns, server.tools())
}
```

**(c)** Add an execution arm in `route_builtin_tool` (`src/mcp/mod.rs`) that dispatches to your execute function and maps a hard error into a soft error result:

```rust
"your_server" => {
    let result = your_server::execute_tool(call, config)
        .await
        .map_err(|e| format!("your_server tool failed: {}", e));
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

### 4. Implement Tool Execution

Execute functions take the `&McpToolCall` (which carries `tool_name`, `parameters`, and `tool_id`) and usually `&Config`. They return `anyhow::Result<McpToolResult>`. Read parameters off `call.parameters`:

```rust
use crate::config::Config;
use crate::mcp::{McpToolCall, McpToolResult};
use anyhow::Result;

pub async fn execute_tool(call: &McpToolCall, config: &Config) -> Result<McpToolResult> {
    match call.tool_name.as_str() {
        "your_tool" => {
            let param1 = match call.parameters.get("param1").and_then(|v| v.as_str()) {
                Some(v) => v,
                // Soft (user-facing) failure: return Ok(error result), do NOT bail with Err.
                None => return Ok(McpToolResult::error(
                    call.tool_name.clone(),
                    call.tool_id.clone(),
                    "Missing required parameter: param1".to_string(),
                )),
            };

            // Do work with `param1` and `config`...
            Ok(McpToolResult::success(
                call.tool_name.clone(),
                call.tool_id.clone(),
                "Result text".to_string(),
            ))
        }
        // Unexpected internal/routing failure: Err is fine — route_builtin_tool wraps it.
        other => Err(anyhow::anyhow!("Unknown tool: {}", other)),
    }
}
```

Existing references for the exact signatures:
- `pub async fn execute_plan(call: &McpToolCall) -> Result<McpToolResult>` (`src/mcp/core/plan/core.rs`)
- `pub async fn execute_tap_command(call: &McpToolCall, config: &Config) -> Result<McpToolResult>` (`src/mcp/core/tap.rs`)
- `pub async fn execute_runtime_tool(call: &McpToolCall, config: &Config) -> Result<McpToolResult>` (`src/mcp/runtime/mod.rs`)

#### Error-handling contract

This is non-obvious and worth stating up front:

- **Soft / user-facing failures** (missing param, bad input, tool-level rejection): return `Ok(McpToolResult::error(name, tool_id, msg))`. The model sees the error text and can retry.
- **Hard / internal failures** (unexpected routing/internal errors): return `Err(anyhow!...)`. `route_builtin_tool` catches it and wraps it into an error result for you (it never propagates a panic to the wire).

### 5. Add Config Registration

Register your server as a builtin in the config:

```toml
[[mcp.servers]]
name = "your_server"
type = "builtin"
timeout_seconds = 30
tools = []
```

`tools = []` means **all** tools from this server are exposed. A non-empty array filters by exact name or wildcard pattern (`is_tool_allowed_by_patterns`), e.g. `tools = ["your_tool", "prefix_*"]`. For external servers, the runtime overlay (`capability enable ...`) can additionally unlock tools that the static filter would otherwise hide.

### 6. Surface Misuse Hints (Optional)

There is no static hint table to declare. To nudge the model when it misuses a tool, push a hint imperatively from inside your execute function:

```rust
crate::mcp::hint_accumulator::push_hint("Use param1 for X, not Y");
```

Hints are session-scoped, deduplicated, and drained once per tool round by the session layer (`drain_hints()`), then injected as a single user-role message — so they guide the model without polluting individual tool-result strings (`src/mcp/hint_accumulator.rs`).

## Protocol Compliance

All tools must follow the MCP protocol:

- Return `McpToolResult::error(...)` instead of panicking.
- Validate all parameters with clear error messages.
- Handle missing/empty/wrong-type parameters gracefully.
- Long-running tools may accept `cancellation_token: Option<tokio::sync::watch::Receiver<bool>>` (as the `agent` server does). Otherwise cancellation is enforced centrally by `try_execute_tool_call`, which races your future against the cancel signal via `tokio::select!`. `execute_plan` / `execute_tap_command` take no token.
- The wire response shape is `{content: [{type: "text", text: "..."}], isError: bool}`, but you never build it by hand. Return `McpToolResult::success(name, tool_id, text)` or `McpToolResult::error(name, tool_id, msg)`; the content-array + `isError` serialization (wrapping `rmcp::model::CallToolResult`) is handled for you.

### Returning metadata alongside text

To attach structured metadata to a successful result, use `McpToolResult::success_with_metadata(name, tool_id, text, json_value)`. The metadata is stored as the result's `structured_content`; `extract_content()` appends it to the text as a `[Metadata: ...]` block (`src/mcp/mod.rs`).

## Testing

Build a real `McpToolCall` and pass it (plus a `&Config`) to your execute function. `is_error()` is a method, not a field:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpToolCall;

    #[tokio::test]
    async fn test_your_tool() {
        let config = crate::config::Config::load().unwrap();
        let call = McpToolCall {
            tool_name: "your_tool".to_string(),
            parameters: serde_json::json!({"param1": "test"}),
            tool_id: "test-id".to_string(),
        };
        let result = execute_tool(&call, &config).await.unwrap();
        assert!(!result.is_error());
    }

    #[tokio::test]
    async fn test_missing_params() {
        let config = crate::config::Config::load().unwrap();
        let call = McpToolCall {
            tool_name: "your_tool".to_string(),
            parameters: serde_json::json!({}),
            tool_id: "test-id".to_string(),
        };
        let result = execute_tool(&call, &config).await.unwrap();
        assert!(result.is_error());
    }
}
```

## Reference Patterns

### Config-Dependent Functions

When a server's function list depends on config, implement `get_all_functions(config: &Config)` directly — there is no separate wrapper. The `agent` server is the canonical example: it maps over `config.agents` to produce one `agent_<name>` tool each (`src/mcp/agent/functions.rs`), and is wired in `server_functions_for` as:

```rust
"agent" => {
    let fns = agent::get_all_functions(config);
    filter_tools_by_patterns(fns, server.tools())
}
```

### Function Caching

Stateless built-in function lists are memoized through `get_filtered_server_functions(...)`, which caches per `server_type` + allowed-tools key in `INTERNAL_FUNCTION_CACHE` (`src/mcp/mod.rs`). Use it for servers whose tool list never changes (like `core`/`runtime`). Config-dependent servers (like `agent`) skip the cache and call `get_all_functions(config)` each time. `clear_function_cache()` empties the cache — call it in tests or when the tools configuration changes.

### Async Operations with Timeout

```rust
use tokio::time::timeout;
use std::time::Duration;

let result = timeout(
    Duration::from_secs(config.timeout),
    async_operation()
).await
.map_err(|_| anyhow!("Operation timed out"))?;
```

### Session-Ownership Checks for Dynamic Tools

If you add dynamic or runtime-registered tools (e.g. `agent_*` tools or dynamic servers), be aware that `execute_tool_without_cancellation` (`src/mcp/mod.rs`) enforces session ownership: the global tool map can contain tools registered by other sessions, so in a session context it verifies that a dynamic tool either is config-defined or belongs to the current session, returning a "belongs to another session" error otherwise. Static built-in tools (`plan`, `tap`, etc.) and project-local tools under the synthetic `local` server bypass this check.

### Server Stderr Capture

For stdio servers, the last ~50 lines of each server's stderr are drained into a private `SERVER_STDERR` map (`Arc<RwLock<HashMap<String, StderrBuffer>>>` in `src/mcp/process.rs`) by background reader threads, and surfaced in init-failure diagnostics and health checks. There is **no** public `get_server_stderr` accessor.

### Initialization Progress

To surface init progress to the UI, pass a callback to `initialize_servers_for_role_with_callback(config, Some(&callback))` (`src/mcp/mod.rs`). It receives `McpInitProgress::Starting { servers }` before initialization and `McpInitProgress::Completed { server, success, function_count }` as each server finishes.
