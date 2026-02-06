# Octomind Developer Guide

**Session-based AI development assistant with MCP tools and multi-provider support.**

## Quick Start

```bash
git clone https://github.com/muvon/octomind.git && cd octomind
export OPENROUTER_API_KEY="your_key_here"  # or OPENAI_API_KEY, ANTHROPIC_API_KEY
cargo check --message-format=short         # Fast syntax check (PREFERRED)
cargo clippy --all-features --all-targets -- -D warnings  # Fix all warnings
cargo build                                # Build when you need the binary
./target/debug/octomind session            # Start first session
```

**Essential session commands:** `/help`, `/info`, `/mcp info`, `/role`, `/model`

**Daily dev cycle:**
```bash
cargo check --message-format=short    # Verify syntax
cargo clippy --all-features --all-targets -- -D warnings  # Fix quality
cargo build                           # Build when needed
# NEVER: cargo build --release (too slow)
```

## Core Principles

- **Session-first**: Everything in interactive AI sessions with MCP tools
- **Template-based config**: All defaults in `config-templates/default.toml`, NO hardcoded values
- **MCP compliance**: Tools return `Ok(McpToolResult::error())` for failures, never `Err()`
- **Fail fast**: Use `.expect()` with clear messages, never hide errors with fallbacks
- **Proper logging**: Use `crate::log_debug!()`, never `println!()`

## Where to Look by Task

| Task | Location |
|------|----------|
| Config issues | `config-templates/default.toml`, `src/config/loading.rs`, `src/config/mod.rs` |
| Roles | `src/config/roles.rs` + template `[[roles]]` |
| MCP servers | `src/config/mcp.rs` + template `[[mcp.servers]]` |
| Layers | `src/session/layers/` + template `[[layers]]` |
| Session behavior | `src/session/chat/session/runner.rs`, `src/session/chat/session/commands/` |
| Continuation | `src/session/chat/continuation/` (modular: detection, injection, processing, file_context, constants) |
| MCP tools | `src/mcp/mod.rs` (routing), `src/mcp/*/functions.rs` (definitions), `src/mcp/*/core.rs` (implementations) |
| Providers | `src/providers/mod.rs` (octolib bridge) |
| Workflows | `src/session/workflows/`, `doc/10-workflows.md` |
| File utilities | `src/utils/file_parser.rs`, `src/utils/file_renderer.rs` |

## Critical Patterns

### MCP Tool Error Handling
```rust
// ✅ CORRECT - MCP-compliant parameter validation
let param = match call.parameters.get("param") {
    Some(Value::String(p)) if !p.trim().is_empty() => p.clone(),
    Some(_) => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "must be string".to_string())),
    None => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "missing param".to_string())),
};

// ✅ CORRECT - MCP-compliant routing
match tool::execute_command(call, token).await {
    Ok(mut result) => { result.tool_id = call.tool_id.clone(); Ok(result) }
    Err(e) => Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("failed: {}", e)))
}
```

### Fail Fast on Errors
```rust
// ❌ BAD - hides real problems
let config = load_config().unwrap_or_else(|_| default_config());

// ✅ GOOD - exposes problems immediately
let config = load_config().expect("CRITICAL: Failed to load config");
```

### Proper Logging
```rust
// ❌ BAD
println!("DEBUG: something happened");

// ✅ GOOD
crate::log_debug!("Something happened");
```

## Debugging

**Tool not working:**
1. Check routing: `src/mcp/mod.rs` → `build_tool_server_map()`
2. Check execution: `src/mcp/mod.rs` → `try_execute_tool_call()`
3. Verify `McpToolResult::error()` returns (not `Err()`)

**Configuration not loading:**
- `octomind config --validate`
- Check `OCTOMIND_*` env overrides
- `src/config/loading.rs` → `load()`

**Session commands failing:**
- `src/session/chat/session/commands/mod.rs`

**Debug commands:**
```bash
/loglevel debug                              # Enable debug logging
/mcp info                                    # Server status
/mcp list                                    # Available tools
octomind config --show                       # Current config
```

## Adding New MCP Tool

1. Define function in `src/mcp/*/functions.rs` → `get_all_functions()`
2. Implement in `src/mcp/*/core.rs` with MCP-compliant validation
3. Route in `src/mcp/mod.rs` → `try_execute_tool_call()`
4. Map in `src/mcp/tool_map.rs`
5. **CRITICAL**: Return `Ok(McpToolResult::error())` for all failures

## Project Structure (Key Files Only)

```
src/
├── config/          # Config loading, validation, types
├── session/
│   ├── chat/        # Session logic, commands, response processing
│   │   ├── continuation/  # Modular continuation system
│   │   └── session/       # Core session runner
│   ├── layers/      # Layered AI processing
│   ├── cache.rs     # 2-marker caching
│   └── cost_tracker.rs
├── mcp/
│   ├── mod.rs       # Tool routing & execution
│   ├── tool_map.rs  # Static tool→server mapping
│   ├── dev/         # shell, ast_grep
│   ├── fs/          # text_editor, list_files
│   ├── web/         # web_search, read_html
│   └── agent/       # agent_<name> tools (route to layers)
├── providers/mod.rs # octolib bridge
└── utils/           # file_parser, file_renderer
config-templates/default.toml  # ALL defaults
```

## Commands Quick Reference

| CLI Command | Description |
|-------------|-------------|
| `octomind config` | Generate default config |
| `octomind session` | Start interactive session |
| `octomind run <layer> "task"` | Execute layer directly |
| `octomind server` | Start WebSocket server |

| Session Command | Description |
|-----------------|-------------|
| `/help` | Show commands |
| `/info` | Token usage & costs |
| `/mcp info` | MCP server status |
| `/role [name]` | View/change role |
| `/model [name]` | View/change model |
| `/truncate` | Reduce context |
| `/loglevel debug` | Enable debug logging |

## Environment Variables

**AI Keys:** `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPSEEK_API_KEY`, `BRAVE_API_KEY`

**Config Overrides:** `OCTOMIND_*` (any config value, nested with `__` for sections)
```bash
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_LOG_LEVEL="debug"
export OCTOMIND_ROLES__DEVELOPER__MODEL="openai:gpt-4o"
```

## Model Format
**Always use:** `provider:model` (e.g., `openrouter:anthropic/claude-sonnet-4`)
**Never:** just `claude-sonnet-4`
