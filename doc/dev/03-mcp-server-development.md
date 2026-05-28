# MCP Server Development

Guide for adding new built-in MCP servers to Octomind.

## When to Add a New Server

Add a built-in server when you need:
- Deep integration with Octomind internals (session state, config)
- Functionality that doesn't make sense as an external process
- Tools that require access to the MCP coordinator

For external tools, prefer configuring a `stdio` or `http` server in config.

## Built-in Servers

| Server | Location | Purpose |
|--------|----------|---------|
| `core` | `src/mcp/core/` | Plan, tap |
| `runtime` | `src/mcp/runtime/` | MCP management, agent delegation, skills, scheduling, capability |
| `agent` | `src/mcp/agent/` | Agent delegation via ACP |

External:
| Server | Type | Purpose |
|--------|------|---------|
| `filesystem` (octofs) | stdio | File ops, shell, AST grep |

## Step-by-Step Guide

### 1. Create Server Module

```
src/mcp/
  your_server/
    mod.rs        # Server implementation
```

### 2. Implement `get_all_functions()`

Return a list of MCP tool definitions:

```rust
use rmcp::model::{Tool, ToolInputSchema};
use serde_json::json;

pub fn get_all_functions() -> Vec<Tool> {
    vec![
        Tool {
            name: "your_tool".into(),
            description: Some("Description shown to AI".into()),
            input_schema: ToolInputSchema {
                r#type: "object".into(),
                properties: Some(json!({
                    "param1": {
                        "type": "string",
                        "description": "Parameter description"
                    }
                })),
                required: Some(vec!["param1".into()]),
                ..Default::default()
            },
        },
    ]
}
```

### 3. Register in `src/mcp/mod.rs`

Add your server to the MCP coordinator's server registry.

### 4. Implement Tool Execution

Handle tool calls and return results:

```rust
use crate::mcp::McpToolResult;

pub async fn execute_tool(
    tool_name: &str,
    params: &serde_json::Value,
    tool_id: &str,
) -> McpToolResult {
    match tool_name {
        "your_tool" => {
            let param1 = match params.get("param1").and_then(|v| v.as_str()) {
                Some(v) => v,
                None => return McpToolResult::error(
                    "your_tool".to_string(),
                    tool_id.to_string(),
                    "Missing required parameter: param1".to_string()
                ),
            };

            // Do work...
            McpToolResult::success("your_tool".to_string(), tool_id.to_string(), "Result text".to_string())
        }
        _ => McpToolResult::error("your_tool".to_string(), tool_id.to_string(), format!("Unknown tool: {tool_name}")),
    }
}
```

### 5. Add Config Registration

Register your server as a builtin in the config:

```toml
[[mcp.servers]]
name = "your_server"
type = "builtin"
timeout_seconds = 30
tools = []
```

### 6. Add Misuse Hints (Optional)

Provide hints when the AI misuses a tool:

```rust
pub fn get_misuse_hints() -> Vec<(&'static str, &'static str)> {
    vec![
        ("your_tool", "Use param1 for X, not Y"),
    ]
}
```

## Protocol Compliance

All tools must follow MCP protocol:

- Return `McpToolResult::error()` instead of panicking
- Validate all parameters with clear error messages
- Handle missing/empty/wrong-type parameters gracefully
- Support cancellation via `CancellationToken`
- Use standard response format: `{content: [{type: "text", text: "..."}], isError: bool}`

## Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_your_tool() {
        let params = serde_json::json!({"param1": "test"});
        let result = execute_tool("your_tool", &params, "test-id").await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_missing_params() {
        let params = serde_json::json!({});
        let result = execute_tool("your_tool", &params, "test-id").await;
        assert!(result.is_error);
    }
}
```

## Reference Patterns

### Config-Dependent Functions

```rust
pub fn get_config_functions(config: &Config) -> Vec<Tool> {
    let mut tools = get_all_functions();
    if config.some_feature_enabled {
        tools.push(additional_tool());
    }
    tools
}
```

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

### Server Stderr Capture

For stdio servers, stderr is captured in `SERVER_STDERR` buffer for debugging. Access via `get_server_stderr(server_name)`.
