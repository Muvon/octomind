# Octomind Developer Guide

Octomind is a session-based AI development assistant written in Rust. Users run interactive chat sessions with MCP tools attached; the AI can call those tools to read/write files, search code, run shell commands, and delegate to sub-agents (layers). Multiple AI providers are supported via [octolib](https://github.com/muvon/octolib).

## Documentation

Documentation lives in `doc/` organized by audience:

```
doc/
  README.md                    # Index and navigation
  usage/01-..12-*.md           # End-user docs (install, config, sessions, tools, etc.)
  integration/01-..04-*.md     # Integration docs (WebSocket, ACP, daemon/hooks, taps)
  dev/01-..03-*.md             # Contributor docs (build, architecture, MCP dev)
  troubleshooting/01-..02-*.md # Troubleshooting and migration
  reference/01-..04-*.md       # CLI, session commands, config fields, env vars
```

Numbering is ordering within each directory (01, 02, ...), not global.

**Key rules for docs:**
- Single source of truth: each topic documented in exactly one file, others link to it
- All config examples must match `config-templates/default.toml` field names
- All paths must match `src/directories.rs` (data: `~/.local/share/octomind/`, config: `~/.local/share/octomind/config/config.toml`)
- MCP server type is `"stdio"` (not `"stdin"`)
- Role format is `[[roles]]` with `name = "..."` (not `[role_name]` sections)
- Core MCP tools are only: `plan`, `mcp`, `agent`, `schedule`, `skill`
- Filesystem tools (view, text_editor, etc.) come from external octofs stdio server

## Architecture Overview

```
CLI / WebSocket
      |
      v
 ChatSession          <- src/session/chat/session/  (core.rs, main_loop.rs)
      |
      +-- Roles       <- src/config/roles.rs         (model, system prompt, MCP servers per role)
      +-- Layers      <- src/session/layers/          (chained AI sub-agents, run after each response)
      +-- Pipelines   <- src/session/pipelines/       (deterministic script steps, run before workflows)
      +-- Workflows   <- src/session/workflows/       (multi-step orchestrated task runners)
      |
      +-- MCP servers <- src/mcp/
            +-- core/     plan, mcp, agent, schedule, skill
            +-- (filesystem tools provided by external octofs MCP server)
            +-- agent/    agent_* tools -> route tasks to configured layers
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
| Directory paths | `src/directories.rs` |
| Session init & state | `src/session/chat/session/core.rs` |
| Session main loop | `src/session/chat/session/main_loop.rs` |
| Session commands (`/role`, `/model` ...) | `src/session/chat/session/commands/` |
| Conversation compression | `src/session/chat/conversation_compression.rs` |
| Layers runtime | `src/session/layers/layer_trait.rs`, `processor.rs` |
| Pipelines config | `src/config/pipelines.rs` |
| Pipelines runtime | `src/session/pipelines/orchestrator.rs`, `executor.rs` |
| Workflows | `src/session/workflows/orchestrator.rs` |
| Skills (auto-activation, validation) | `src/mcp/core/skill.rs`, `src/mcp/core/skill_auto.rs` |
| Skills config | `src/config/mod.rs` -> `SkillsConfig` |
| Capability resolution (runtime) | `src/agent/registry.rs` -> `parse_capability_toml()` |
| Tap registry & discovery | `src/agent/taps.rs` |
| MCP tool routing | `src/mcp/mod.rs` -> `try_execute_tool_call()` |
| MCP tool registry | `src/mcp/tool_map.rs` |
| MCP tool definitions | `src/mcp/*/functions.rs` -> `get_all_functions()` |
| AI provider bridge | `src/providers.rs` (thin wrapper over octolib) |
| Structured output (schema) | `src/providers.rs` -> `ChatCompletionParams::with_schema()`, `src/session/mod.rs` -> `chat_completion_with_provider()` |
| CLI schema flag parsing | `src/session/chat/session/params.rs`, `src/session/chat/session/setup.rs` |
| File read/write helpers | `src/utils/file_parser.rs`, `src/utils/file_renderer.rs` |

## Session Entry Points (CRITICAL — keep synchronized)

All session modes share the same initialization sequence. When adding new session-scoped
state (like skill pools, inbox, job managers), it MUST be added to ALL entry points.

| Mode | File | Function |
|------|------|----------|
| Interactive CLI | `src/session/chat/session/main_loop.rs:100` | `run_interactive_session()` |
| Non-interactive CLI | `src/session/chat/session/main_loop.rs:959` | `run_interactive_session_with_input()` |
| ACP new_session | `src/acp/agent.rs:469` | `AcpAgent` — first `with_session_id` block |
| ACP initialize | `src/acp/agent.rs:917` | `AcpAgent` — second `with_session_id` block |
| WebSocket server | `src/websocket/server.rs:609` | `handle_session_message()` |

**Required initialization sequence** (inside `with_session_id` context):
```
1. crate::session::inbox::init_inbox_for_session()
2. crate::mcp::agent::functions::init_job_manager()
3. crate::mcp::core::skill_auto::init_pool(&role)
4. crate::mcp::core::skill_auto::load_env_skills().await
```

**run_activation hook** (user input processing — only in main_loop.rs):
```
crate::mcp::core::skill_auto::run_activation(Event::User, &input, &current_dir).await
```
Called before layers/pipeline processing, after command handling.

**run_validators hook** (after tool execution — only in tool_result_processor.rs):
```
crate::mcp::core::skill_auto::run_validators(Event::Turn, &content, &workdir).await
```
Called after tool results are collected, before spending threshold check.

**If you add a new session entry point or session-scoped state, grep for
`init_inbox_for_session` to find all locations that need updating.**

## Code Quality Rules

**Copyright header -- every `.rs` file must have it:**
```rust
// Copyright <YEAR> Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// ...full Apache 2.0 header...
```
- New files: use the current year
- Modified files: update the year to the current year if it's outdated
- Check: `rg -l "Copyright 2025" --type rust` should return nothing for files modified in the current year

**Build & lint -- run after every change:**
```bash
cargo check --message-format=short                            # fast syntax check
cargo clippy --all-features --all-targets -- -D warnings      # must pass clean
cargo build                                                   # debug only -- never --release
```

**Errors -- fail fast, never hide:**
```rust
// expose problems immediately
let config = load_config().expect("failed to load config");

// don't hide real problems
// let config = load_config().unwrap_or_else(|_| default_config());
```

**Logging -- never println in library code:**
```rust
crate::log_debug!("something happened");   // correct
// println!("DEBUG: ...");                  // breaks spinner, wrong output path
```

**MCP tools -- errors are values, not panics:**
```rust
// Parameter validation
let param = match call.parameters.get("param") {
    Some(Value::String(p)) if !p.trim().is_empty() => p.clone(),
    Some(_) => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "must be string".to_string())),
    None    => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "missing param".to_string())),
};

// Routing -- wrap Err, never propagate
match tool::execute(call).await {
    Ok(mut r) => { r.tool_id = call.tool_id.clone(); Ok(r) }
    Err(e)    => Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("failed: {e}")))
}
```

**MCP misuse hints -- guide the model toward better tools:**
When a tool is used where a dedicated tool would be better, append a hint to the output. Never block execution. Only emit if the recommended tool is actually enabled.
```rust
let hint = if crate::mcp::tool_map::get_server_for_tool("better_tool").is_some() {
    "\n\n Prefer `better_tool` here -- reason."
} else { "" };
```
Reference: `src/mcp/core/schedule.rs` (schedule misuse hints), `src/mcp/hint_accumulator.rs` for pattern.

## Adding a New MCP Tool

1. Define in `src/mcp/*/functions.rs` -> `get_all_functions()`
2. Implement in the same module
3. Route in `src/mcp/mod.rs` -> `try_execute_tool_call()`
4. Register in `src/mcp/tool_map.rs`
5. Return `Ok(McpToolResult::error())` for all failures -- never `Err()`
6. Add misuse hints if a more specific tool should be preferred

## Debugging Starting Points

| Problem | Where to start |
|---------|----------------|
| Tool not routing | `src/mcp/mod.rs` -> `build_tool_server_map()`, `try_execute_tool_call()` |
| Tool not found | `src/mcp/tool_map.rs` |
| Config not loading | `src/config/loading.rs` -> `load()` |
| Session command broken | `src/session/chat/session/commands/mod.rs` |
| Layer not running | `src/session/layers/processor.rs` |
| Compression not working | `src/session/chat/conversation_compression.rs` |
| Structured output not working | `src/session/mod.rs` -> `chat_completion_with_provider()` (provider capability check), `src/providers.rs` -> `to_octolib_params()` (schema application) |
