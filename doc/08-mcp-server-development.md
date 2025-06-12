# MCP Server Development Guide

This guide explains how to add new built-in MCP servers to Octomind. Use this when you need to create a new category of tools that should be available as a separate MCP server.

## Overview

Octomind has several built-in MCP servers:
- **developer**: Shell commands, development tools
- **filesystem**: File operations, directory listing, HTML conversion
- **agent**: Task routing to AI layers

Each server provides a specific category of tools and can be enabled/disabled independently in role configurations.

## When to Add a New MCP Server

Add a new built-in MCP server when you have:
- A logical grouping of related tools (e.g., database operations, API calls, image processing)
- Tools that should be independently configurable from existing servers
- Functionality that doesn't fit well in existing servers

## Step-by-Step Implementation

### 1. Add Server Type to Enum

**File: `src/config/mcp.rs`**

Add your server type to the `McpServerType` enum:

```rust
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum McpServerType {
    #[serde(rename = "external")]
    External,
    #[serde(rename = "developer")]
    Developer,
    #[serde(rename = "filesystem")]
    Filesystem,
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "database")]  // <- Add your new server type
    Database,
}
```

### 2. Update Server Config Helper Methods

**File: `src/config/mcp.rs`**

Add your server to the `from_name` method:

```rust
pub fn from_name(name: &str) -> Self {
    let server_type = match name {
        "developer" => McpServerType::Developer,
        "filesystem" => McpServerType::Filesystem,
        "agent" => McpServerType::Agent,
        "database" => McpServerType::Database,  // <- Add here
        _ => McpServerType::External,
    };
    // ...
}
```

Add a helper constructor method:

```rust
/// Create a database server configuration
pub fn database(name: &str, tools: Vec<String>) -> Self {
    Self {
        name: name.to_string(),
        server_type: McpServerType::Database,
        url: None,
        auth_token: None,
        command: None,
        args: Vec::new(),
        mode: McpServerMode::Http,
        timeout_seconds: 30,
        tools,
        builtin: true, // Built-in servers are always builtin
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

**File: `src/mcp/mod.rs`**

Add your server to the `get_available_functions` method:

```rust
for server in enabled_servers {
    match server.server_type {
        crate::config::McpServerType::Developer => {
            // existing code...
        }
        crate::config::McpServerType::Filesystem => {
            // existing code...
        }
        crate::config::McpServerType::Agent => {
            // existing code...
        }
        crate::config::McpServerType::Database => {  // <- Add this
            let server_functions =
                get_cached_internal_functions("database", &server.tools, || {
                    database::get_all_functions()
                });
            functions.extend(server_functions);
        }
        // ...
    }
}
```

Add to the `build_tool_server_map` function:

```rust
let server_functions = match server.server_type {
    // existing cases...
    crate::config::McpServerType::Database => {
        get_cached_internal_functions("database", &server.tools, || {
            database::get_all_functions()
        })
    }
    // ...
};
```

### 6. Update Tool Execution

**File: `src/mcp/mod.rs`**

Add execution handling in `try_execute_tool_call`:

```rust
match target_server.server_type {
    // existing cases...
    crate::config::McpServerType::Database => match call.tool_name.as_str() {
        "db_query" | "db_schema" => {
            crate::log_debug!(
                "Executing database command via database server '{}'",
                target_server.name
            );
            let mut result =
                database::execute_database_command(call, config, cancellation_token.clone()).await?;
            result.tool_id = call.tool_id.clone();
            return Ok(result);
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Tool '{}' not implemented in database server",
                call.tool_name
            ));
        }
    },
    // ...
}
```

### 7. Update Server Health Monitoring

**File: `src/mcp/server.rs`**

Add your server type to the builtin server check:

```rust
match server.server_type {
    crate::config::McpServerType::Developer
    | crate::config::McpServerType::Filesystem
    | crate::config::McpServerType::Agent
    | crate::config::McpServerType::Database => {  // <- Add here
        // Internal servers are always considered running
        // ...
    }
    // ...
}
```

### 8. Update Session Commands

**File: `src/session/chat/session/commands.rs`**

Add to server health status checks (2 locations):

```rust
let (health, restart_info) = match server.server_type {
    crate::config::McpServerType::Developer
    | crate::config::McpServerType::Filesystem
    | crate::config::McpServerType::Agent
    | crate::config::McpServerType::Database => {  // <- Add here
        // Internal servers are always running
        // ...
    }
    // ...
};
```

### 9. Update Response Processing

**File: `src/session/chat/response.rs`**

Add to server function gathering:

```rust
let server_functions = match server.server_type {
    // existing cases...
    crate::config::McpServerType::Database => {
        crate::mcp::get_cached_internal_functions("database", &server.tools, || {
            crate::mcp::database::get_all_functions()
        })
    }
    // ...
};
```

### 10. Update Config Commands

**File: `src/commands/config.rs`**

Add to server type detection (2 locations):

```rust
let effective_type = match name.as_str() {
    "developer" => McpServerType::Developer,
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
server_type = "builtin"
timeout_seconds = 30
tools = []
```

Add to role configurations where appropriate:

```toml
# Developer role - add database server
mcp = { server_refs = ["developer", "filesystem", "agent", "database"], allowed_tools = [] }
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
    ).await??;

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
