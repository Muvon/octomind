# MCP Server Development Guide

This guide explains how to add new built-in MCP servers to Octomind. Use this when you need to create a new category of tools that should be available as a separate MCP server.

## Overview

## Built-in MCP Servers

Octomind provides **three** built-in MCP servers with comprehensive development capabilities:

**Core Server** (`src/mcp/core/`):
- `plan(command=\"start|step|next|list|done|reset\", ...)` - Structured task management with progress tracking
- `mcp(action=\"list|add|enable|disable|remove\", ...)` - Dynamic MCP server management
- `agent(action=\"list|add|enable|disable|remove\", ...)` - Dynamic agent tool management

**Filesystem Server** (`src/mcp/fs/`):
- `view(path=\"...\", lines=[start, end], pattern=\"...\", content=\"...\", ...)` - Read files, view directories, and search file content
- `text_editor(command=\"create|str_replace|undo_edit\", path=\"...\", ...)` - Edit files
- `batch_edit(path=\"...\", operations=[...])` - Multiple file operations atomically
- `extract_lines(from_path=\"...\", from_range=[start, end], append_path=\"...\", append_line=N)` - Extract and move code blocks
- `shell(command=\"...\", background=false)` - Execute shell commands with output capture, foreground/background execution
- `workdir(path=\"...\", reset=false)` - Get or set working directory for parallel execution isolation
- `ast_grep(pattern=\"...\", language=\"...\", rewrite=\"...\", ...)` - Search and refactor code using AST patterns

**Agent Server** (`src/mcp/agent/`):
- `agent_*()` tools - Delegate tasks to configured ACP sub-agents (each spawns an ACP subprocess or executes in-process for dynamic agents)

## When to Add a New MCP Server

Add a new built-in MCP server when you have:
- A logical grouping of related tools (e.g., database operations, API calls, image processing)
- Tools that should be independently configurable from existing servers
- Functionality that doesn't fit well in existing servers

## MCP Server Initialization

Octomind uses a parallel initialization system for MCP servers to ensure fast startup. The initialization process includes:

1. **Parallel Connection**: All external servers (HTTP, Stdin) are connected simultaneously.
2. **Progress Tracking**: Detailed progress is shown via the `McpInitProgress` enum, displaying server names, success status, and function counts.
3. **Timeout Handling**: Each server has a configurable `timeout_seconds` to prevent startup hangs.
4. **Error Recovery**: Failed servers are logged but don't prevent the session from starting if other servers are available.

## Dynamic MCP Servers and Agents

In addition to config-defined servers, Octomind supports dynamic management of MCP servers and agents at runtime.

### Dynamic MCP Servers
- **Registration**: Add a server configuration to the dynamic manager.
- **Activation**: Connect and fetch tools from a registered server.
- **Control**: Temporarily deactivate a server's tools.
- **Lifecycle**: Dynamic servers persist for the duration of the session.

### Dynamic Agents
- **Registration**: Define specialized AI agents with their own system prompts and tools.
- **Tool Integration**: Each dynamic agent becomes an MCP tool prefixed with `agent_`.
- **Execution**: Agents can run in-process or as separate ACP subprocesses.
## External HTTP Servers with OAuth 2.1 + PKCE

Octomind supports OAuth 2.1 + PKCE authentication for external HTTP MCP servers. This allows secure authentication without storing credentials.

### OAuth Configuration

Add OAuth configuration to your HTTP MCP server:

```toml
[[mcp.servers]]
name = "github_mcp"
type = "http"
url = "https://api.github.com/mcp"
timeout_seconds = 30
tools = []

# OAuth 2.1 + PKCE configuration
[mcp.servers.oauth]
client_id = "your-oauth-client-id"
client_secret = "your-oauth-client-secret"
authorization_url = "https://github.com/login/oauth/authorize"
token_url = "https://github.com/login/oauth/access_token"
callback_url = "http://localhost:34567/oauth/callback"
scopes = ["repo", "read:org"]
```

### OAuth Flow

1. **Initiation**: When Octomind connects to the server, it checks for OAuth configuration
2. **Authorization**: User is directed to the authorization URL in their browser
3. **Token Exchange**: After user authorization, Octomind exchanges the authorization code for an access token
4. **Automatic Usage**: Subsequent requests use the OAuth token automatically
5. **Token Refresh**: Tokens are automatically refreshed when expired

### Implementation Details

OAuth support is implemented in `src/config/oauth_config.rs` with:
- PKCE (Proof Key for Code Exchange) for enhanced security
- Automatic token refresh
- Secure token storage
- Support for multiple OAuth providers

### Benefits

- **Secure**: No need to store credentials in configuration
- **User-Controlled**: Users authorize access through their browser
- **Automatic**: Tokens are managed automatically
- **Standard**: Uses OAuth 2.1 standard with PKCE

## Tool Misuse Hints

When a tool can be misused in place of a better dedicated tool, append a hint to the output. This guides the model toward the right tool without blocking execution.

**Rules:**
- Gate on `crate::mcp::tool_map::get_server_for_tool("tool_name").is_some()` — never hint for disabled tools
- Hint text starts with `⚠️`, names the preferred tool in backticks, explains why
- Append to output, never block — the command still runs and returns its result
- Keep hints to one line

**Pattern:**
```rust
let hint = if crate::mcp::tool_map::get_server_for_tool("better_tool").is_some() {
    "\n\n⚠️ Prefer `better_tool` for this use case — reason why it's better."
} else {
    ""
};
// Append hint to result content string
```

**Existing hints (reference implementations):**
- `src/mcp/fs/shell.rs` — `SHELL_MISUSE_HINTS` table: warns on `cat/grep/find/sed` when dedicated tools are enabled
- `src/mcp/fs/text_editing.rs` — `str_replace` hints `line_replace` when match spans multiple lines

## Step-by-Step Implementation

1. Create server module structure (`src/mcp/<server_name>/`)
2. Add your server type to the `McpServerType` enum in `src/config/mcp.rs`
3. Update server config helpers in the same file
4. Register tool routing and error handling in `src/mcp/mod.rs` (never return Err, always MCP error)
5. Add config-driven registration and allowed_tools patterns
6. Validate parameters using MCP-compliant patterns
7. Never add fallback or default tool logic for missing config
8. Add misuse hints if a more specific tool should be preferred (see above)
9. See [src/mcp/*/functions.rs] for examples

### 2. Update Server Config Helper Methods

**File: `src/config/mcp.rs`**

Add your server to the `from_name` method:

```rust
pub fn from_name(name: &str) -> Self {
    let connection_type = match name {
        \"core\" => McpConnectionType::Builtin,
        \"filesystem\" => McpConnectionType::Builtin,
        \"agent\" => McpConnectionType::Builtin,
        \"database\" => McpConnectionType::Builtin,  // <- Add here
        _ => McpConnectionType::Http,
    };
    // ...
}
```

Add a helper constructor method:

```rust
/// Create a database server configuration
pub fn database(name: &str, tools: Vec<String>) -> Self {
    Self::Builtin {
        name: name.to_string(),
        timeout_seconds: 30,
        tools,
    }
}
```

### 3. Create Server Module

**Create directory: `src/mcp/database/`**

**File: `src/mcp/database/mod.rs`**

```rust
// Copyright 2025 Muvon Un Limited
// Licensed under the Apache License, Version 2.0

// Database MCP provider - handles database operations

pub mod functions;

// Re-export main functionality
pub use functions::{execute_database_command, get_all_functions};
```

**File: `src/mcp/database/functions.rs`**

```rust
// Copyright 2025 Muvon Un Limited
// Licensed under the Apache License, Version 2.0

use crate::mcp::{McpFunction, McpToolCall, McpToolResult};
use anyhow::Result;
use serde_json::json;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

// Get all available database functions
pub fn get_all_functions() -> Vec<McpFunction> {
    vec![
        get_query_function(),
        get_schema_function(),
        // Add more functions as needed
    ]
}

// Define your tool functions
fn get_query_function() -> McpFunction {
    McpFunction {
        name: "db_query".to_string(),
        description: "Execute SQL query against configured database".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "SQL query to execute"
                },
                "database": {
                    "type": "string",
                    "description": "Database name (optional)"
                }
            },
            "required": ["sql"]
        }),
    }
}

fn get_schema_function() -> McpFunction {
    McpFunction {
        name: "db_schema".to_string(),
        description: "Get database schema information".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "table": {
                    "type": "string",
                    "description": "Specific table name (optional)"
                }
            }
        }),
    }
}

// Execute database tool calls
pub async fn execute_database_command(
    call: &McpToolCall,
    _config: &crate::config::Config,
    _cancellation_token: Option<Arc<AtomicBool>>,
) -> Result<McpToolResult> {
    match call.tool_name.as_str() {
        "db_query" => execute_query(call).await,
        "db_schema" => execute_schema(call).await,
        _ => Err(anyhow::anyhow!(
            "Tool '{}' not implemented in database server",
            call.tool_name
        )),
    }
}

async fn execute_query(call: &McpToolCall) -> Result<McpToolResult> {
    let sql = call
        .parameters
        .get("sql")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("SQL parameter required"))?;

    // Implement your database query logic here
    let result = format!("Executed SQL: {}", sql);

    Ok(McpToolResult::success(
        call.tool_name.clone(),
        call.tool_id.clone(),
        result,
    ))
}

async fn execute_schema(call: &McpToolCall) -> Result<McpToolResult> {
    // Implement your schema retrieval logic here
    let result = "Database schema information";

    Ok(McpToolResult::success(
        call.tool_name.clone(),
        call.tool_id.clone(),
        result,
    ))
}
```

### 4. Add Module to MCP Root

**File: `src/mcp/mod.rs`**

Add module declaration:

```rust
pub mod agent;
pub mod database;  // <- Add your module
pub mod dev;
pub mod fs;
// ...
```

### 5. Update MCP Function Discovery

**File: `src/mcp/tool_map.rs`**
Add your server to the `build_tool_server_map_impl` method:

```rust
match server.name() {
    \"core\" => {
        // ...
    }
    \"database\" => crate::mcp::get_filtered_server_functions(
        \"database\",
        server.tools(),
        crate::mcp::database::get_all_functions,
    ),
    // ...
}
```
### 6. Add Server Type Enum (if needed)

If your server needs a dedicated enum variant (rare for builtin servers):

```rust
// In src/config/mcp.rs
pub enum McpServerType {
    Core,
    Filesystem,
    Agent,
    Database,  // <- Add this
    Http,
}
```

### 7. Update Tool Execution

**File: `src/mcp/mod.rs`**

Add execution handling in `try_execute_tool_call`:

```rust
match target_server.connection_type {
    crate::config::McpConnectionType::Builtin => {
        // Handle builtin servers by name
        match target_server.name.as_str() {
            "database" => match call.tool_name.as_str() {
                "db_query" | "db_schema" => {
                    crate::log_debug!(
                        "Executing database command via database server '{}'",
                        target_server.name
                    );
                    let mut result = match database::execute_database_command(call, config, cancellation_token.clone()).await {
                        Ok(res) => res,
                        Err(e) => {
                            return Ok(McpToolResult::error(
                                call.tool_name.clone(),
                                call.tool_id.clone(),
                                format!("Database execution failed: {}", e),
                            ));
                        }
                    };
                    result.tool_id = call.tool_id.clone();
                    return Ok(result);
                }
                _ => {
                    return Ok(McpToolResult::error(
                        call.tool_name.clone(),
                        call.tool_id.clone(),
                        format!("Tool '{}' not implemented in database server", call.tool_name),
                    ));
                }
            },
            // ... other servers
        }
    }
    // ... other connection types
}
```

### 8. Update Server Health Monitoring

**File: `src/mcp/server.rs`**

Add your server type to the builtin server check:

```rust
match server.connection_type {
    crate::config::McpConnectionType::Builtin => {
        // All builtin servers are always available
    }
    // ...
}
```

### 9. Update Session Commands

**File: `src/session/chat/session/commands.rs`**

Add to server health status checks (2 locations):

```rust
let (health, restart_info) = match server.connection_type {
    crate::config::McpConnectionType::Builtin => {
        // All builtin servers are always healthy
    }
    // ...
};
```

### 10. Update Response Processing

**File: `src/session/chat/response.rs`**

Add to server function gathering:

```rust
let server_functions = match server.connection_type {
    crate::config::McpConnectionType::Builtin => {
        // Handle builtin servers by name
        match server.name.as_str() {
            "database" => {
                crate::mcp::get_cached_internal_functions("database", &server.tools, || {
                    crate::mcp::database::get_all_functions()
                })
            }
            // ...
        }
    }
    // ...
};
```

### 11. Update Config Commands

**File: `src/commands/config.rs`**

Add to server type detection (2 locations):

```rust
let effective_type = match name.as_str() {
    "core" => McpServerType::Core,
    "filesystem" => McpServerType::Filesystem,
    "agent" => McpServerType::Agent,
    "database" => McpServerType::Database,  // <- Add here
    _ => McpServerType::External,
};

match effective_type {
    // existing cases...
    McpServerType::Database => {
        println!("  - {} (built-in database tools) - available", name)
    }
    // ...
}
```

And in the display function:

```rust
match effective_type {
    // existing cases...
    McpServerType::Database => {
        println!("      🗄️ {} (built-in database tools)", name);
    }
    // ...
}
```

### 11. Add to Default Configuration

**File: `config-templates/default.toml`**

Add your server to the MCP servers section:

```toml
[[mcp.servers]]
name = "database"
type = "builtin"
timeout_seconds = 30
tools = []
```

Add to role configurations where appropriate:

```toml
# Developer role - add database server
mcp = { server_refs = ["core", "filesystem", "agent", "database"], allowed_tools = [] }
```

### 12. Update Documentation

**File: `doc/06-advanced.md`**

Add documentation for your new server and its tools.

## Testing Your Implementation

1. **Build and check for errors:**
   ```bash
   cargo check
   ```

2. **Test configuration loading:**
   ```bash
   cargo run -- config --validate
   ```

3. **Test in a session:**
   ```bash
   cargo run -- session
   # Try using your new tools
   ```

## Best Practices

1. **Naming**: Use clear, descriptive names for server types and tools
2. **Error Handling**: Provide clear error messages for invalid parameters
3. **Documentation**: Include comprehensive parameter descriptions
4. **Tool Grouping**: Keep related tools together in the same server
5. **Configuration**: Make tools configurable through the `tools` array
6. **Testing**: Test with various parameter combinations
7. **Logging**: Use `crate::log_debug!` for debugging information

## Common Patterns

### Config-Dependent Functions
If your tools need configuration data:

```rust
pub fn get_database_function(config: &crate::config::Config) -> McpFunction {
    // Access config.database_config or similar
    // Return function with dynamic parameters based on config
}
```

### Async Operations
Most MCP operations are async. Use `tokio` for async operations:

```rust
async fn execute_async_operation(call: &McpToolCall) -> Result<McpToolResult> {
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        perform_operation()
    ).await?;

    Ok(McpToolResult::success(
        call.tool_name.clone(),
        call.tool_id.clone(),
        result,
    ))
}
```

### Error Handling
Always provide meaningful error messages:

```rust
let param = call
    .parameters
    .get("required_param")
    .and_then(|v| v.as_str())
    .ok_or_else(|| anyhow::anyhow!(
        "Tool '{}' requires 'required_param' parameter",
        call.tool_name
    ))?;
```

This guide provides a complete template for adding new built-in MCP servers to Octomind. Follow these steps systematically to ensure your new server integrates properly with all system components.
