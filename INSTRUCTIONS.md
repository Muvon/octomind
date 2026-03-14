# Octomind Developer Guide

Octomind is a session-based AI development assistant written in Rust. Users run interactive chat sessions with MCP tools attached; the AI can call those tools to read/write files, search code, run shell commands, browse the web, and delegate to sub-agents (layers). Multiple AI providers are supported via [octolib](https://github.com/muvon/octolib).

## Architecture Overview

```
CLI / WebSocket
      │
      ▼
 ChatSession          ← src/session/chat/session/  (core.rs, main_loop.rs)
      │
      ├── Roles       ← src/config/roles.rs         (model, system prompt, MCP servers per role)
      ├── Layers      ← src/session/layers/          (chained AI sub-agents, run after each response)
      ├── Workflows   ← src/session/workflows/       (multi-step orchestrated task runners)
      │
      └── MCP servers ← src/mcp/
            ├── core/     plan, ask
            ├── fs/       view, text_editor, batch_edit, extract_lines, shell, workdir, ast_grep
            └── agent/    agent_* tools → route tasks to configured layers
```

**Config is the single source of truth.** All defaults live in `config-templates/default.toml`. The resolved config drives everything: which model, which MCP servers, which layers, which role. No hardcoded values anywhere in code.

## Where to Look

| Area | Entry point |
|------|-------------|
| Config defaults | `config-templates/default.toml` |
| Config loading & types | `src/config/loading.rs`, `src/config/mod.rs` |
| Roles (model + MCP per role) | `src/config/roles.rs` |
| MCP server config | `src/config/mcp.rs` |
| Layers config | `src/config/layers.rs` |
| Session init & state | `src/session/chat/session/core.rs` |
| Session main loop | `src/session/chat/session/main_loop.rs` |
| Session commands (`/role`, `/model` …) | `src/session/chat/session/commands/` |
| Conversation compression | `src/session/chat/conversation_compression.rs` |
| Layers runtime | `src/session/layers/layer_trait.rs`, `processor.rs` |
| Workflows | `src/session/workflows/orchestrator.rs` |
| MCP tool routing | `src/mcp/mod.rs` → `try_execute_tool_call()` |
| MCP tool registry | `src/mcp/tool_map.rs` |
| MCP tool definitions | `src/mcp/*/functions.rs` → `get_all_functions()` |
| AI provider bridge | `src/providers.rs` (thin wrapper over octolib) |
| Structured output (schema) | `src/providers.rs` → `ChatCompletionParams::with_schema()`, `src/session/mod.rs` → `chat_completion_with_provider()` |
| CLI schema flag parsing | `src/session/chat/session/params.rs`, `src/session/chat/session/setup.rs` |
| File read/write helpers | `src/utils/file_parser.rs`, `src/utils/file_renderer.rs` |

## Code Quality Rules

**Build & lint — run after every change:**
```bash
cargo check --message-format=short                            # fast syntax check
cargo clippy --all-features --all-targets -- -D warnings      # must pass clean
cargo build                                                   # debug only — never --release
```

**Errors — fail fast, never hide:**
```rust
// ✅ expose problems immediately
let config = load_config().expect("failed to load config");

// ❌ hides real problems
let config = load_config().unwrap_or_else(|_| default_config());
```

**Logging — never println in library code:**
```rust
crate::log_debug!("something happened");   // ✅
println!("DEBUG: ...");                    // ❌ — breaks spinner, wrong output path
```

**MCP tools — errors are values, not panics:**
```rust
// Parameter validation
let param = match call.parameters.get("param") {
    Some(Value::String(p)) if !p.trim().is_empty() => p.clone(),
    Some(_) => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "must be string".to_string())),
    None    => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "missing param".to_string())),
};

// Routing — wrap Err, never propagate
match tool::execute(call).await {
    Ok(mut r) => { r.tool_id = call.tool_id.clone(); Ok(r) }
    Err(e)    => Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("failed: {e}")))
}
```

**MCP misuse hints — guide the model toward better tools:**
When a tool is used where a dedicated tool would be better, append a hint to the output. Never block execution. Only emit if the recommended tool is actually enabled.
```rust
let hint = if crate::mcp::tool_map::get_server_for_tool("better_tool").is_some() {
    "\n\n⚠️ Prefer `better_tool` here — reason."
} else { "" };
```
Reference: `src/mcp/fs/shell.rs` (`SHELL_MISUSE_HINTS`), `src/mcp/fs/text_editing.rs` (str_replace → line_replace hint).

## Adding a New MCP Tool

1. Define in `src/mcp/*/functions.rs` → `get_all_functions()`
2. Implement in the same module
3. Route in `src/mcp/mod.rs` → `try_execute_tool_call()`
4. Register in `src/mcp/tool_map.rs`
5. Return `Ok(McpToolResult::error())` for all failures — never `Err()`
6. Add misuse hints if a more specific tool should be preferred

## Debugging Starting Points

| Problem | Where to start |
|---------|----------------|
| Tool not routing | `src/mcp/mod.rs` → `build_tool_server_map()`, `try_execute_tool_call()` |
| Tool not found | `src/mcp/tool_map.rs` |
| Config not loading | `src/config/loading.rs` → `load()` |
| Session command broken | `src/session/chat/session/commands/mod.rs` |
| Layer not running | `src/session/layers/processor.rs` |
| Compression not working | `src/session/chat/conversation_compression.rs` |
| Structured output not working | `src/session/mod.rs` → `chat_completion_with_provider()` (provider capability check), `src/providers.rs` → `to_octolib_params()` (schema application) |
