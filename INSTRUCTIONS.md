# Octomind Developer Guide

Session-based AI development assistant in Rust. Interactive chat sessions with MCP tools; AI calls tools to read/write files, search code, run shell commands, and delegate to sub-agents (layers). Multi-provider via [octolib](https://github.com/muvon/octolib).

## Project Structure

```
src/
  acp/              # Agent Communication Protocol server
  agent/            # Agent registry, taps, inputs, dependency resolution
  config/           # Config loading, roles, MCP, layers, pipelines, hooks, migrations
  directories.rs    # Path constants (data dir, config dir, sessions, logs, cache)
  learning/         # Cross-session lesson extraction, storage, injection
  logging.rs        # Logging infrastructure
  mcp/              # MCP protocol implementation
    core/           # Builtin tools: plan, mcp, agent, schedule, skill
      dynamic.rs   # Dynamic MCP server management (add/remove at runtime)
      dynamic_agents.rs  # Dynamic agent tool registration
      skill_auto.rs     # Skill auto-activation & validation hooks
    agent/          # Agent tool routing (agent_* → layer execution)
    health_monitor.rs   # Server health checks, restart tracking
    hint_accumulator.rs # Misuse hint accumulation
    mod.rs          # Tool routing, server init, try_execute_tool_call()
    process.rs     # Server process management, health enums
    server.rs       # JSON-RPC/SSE MCP server connections
    tool_map.rs     # Global TOOL_MAP: tool name → server config
    utils.rs        # Tool call parsing, response formatting
    workdir.rs      # Per-thread working directory tracking
  providers.rs      # Thin wrapper over octolib (ChatCompletionParams, schemas)
  sandbox/          # Platform sandboxing (Linux Landlock, macOS Seatbelt)
  session/          # Session management
    background_jobs.rs  # Async agent job tracking
    cache.rs            # CacheManager for prompt caching
    cancellation.rs     # Cancellation tokens
    chat/               # Chat session logic
      conversation_compression.rs  # Context compression (task/phase/project/conversation)
      session/
        core.rs          # ChatSession struct, SessionInitParams builder
        main_loop.rs    # Interactive & non-interactive session loops
        commands/       # 25 session commands (clear, context, copy, display, done, exit, help, image, info, list, loglevel, mcp, model, plan, prompt, report, role, run, session, skill, summarize, truncate, utils, video, workflow)
        params.rs       # CLI parameter parsing
        setup.rs        # Session setup & initialization
    context.rs         # Session-scoped context (init_session_services)
    helper_functions.rs # Context summarization helpers
    history.rs         # Role-based history management
    inbox.rs           # InboxQueue (schedule + webhook message injection)
    inject_listener.rs # Unix Domain Socket for external message injection
    layers/            # Layer trait, LayerProcessor, LayerDefinition
    modal.rs           # Terminal modal overlay system
    output.rs          # Output abstraction (JSONL, WebSocket, Silent sinks)
    pipelines/         # Deterministic script pipeline (orchestrator, executor)
    report.rs          # Session usage reporting
    smart_summarizer.rs # Smart text summarization
    workflows/         # WorkflowOrchestrator, StepExecutor, PatternParser
    webhook_listener.rs # HTTP webhook → inbox injection
  state.rs           # IndexState (current_directory, graphrag_blocks)
  utils/              # file_parser, file_renderer, glob, terminal_output, time, truncation
  websocket/          # WebSocket server (handle_session_message)
cli/
  src/
    commands/        # CLI subcommands (acp, config, run, send, server, tap, untap, vars)
config-templates/
  default.toml       # Single source of truth for all config defaults (782 lines)
doc/
  usage/             # End-user docs (01-12)
  integration/       # Integration docs (01-04)
  dev/               # Contributor docs (01-03)
  troubleshooting/   # Troubleshooting & migration (01-02)
  reference/         # CLI, commands, config, env vars (01-04)
```

## Where to Look

| Task / Area | Start here |
|-------------|------------|
| Config defaults & all fields | `config-templates/default.toml` |
| Config loading & types | `src/config/loading.rs`, `src/config/mod.rs` |
| Roles (model + MCP per role) | `src/config/roles.rs` |
| MCP server config | `src/config/mcp.rs` |
| Layers config | `src/config/layers.rs` |
| Pipelines config | `src/config/pipelines.rs` |
| Hooks config | `src/config/hooks.rs` |
| Config migrations | `src/config/migrations.rs` |
| Directory paths | `src/directories.rs` |
| Session init & state | `src/session/chat/session/core.rs` |
| Session main loop | `src/session/chat/session/main_loop.rs` |
| Session setup | `src/session/chat/session/setup.rs` |
| Session commands | `src/session/chat/session/commands/mod.rs` |
| Conversation compression | `src/session/chat/conversation_compression.rs` |
| Layers runtime | `src/session/layers/layer_trait.rs`, `processor.rs` |
| Pipelines runtime | `src/session/pipelines/orchestrator.rs`, `executor.rs` |
| Workflows | `src/session/workflows/orchestrator.rs` |
| Skills (auto-activation, validation) | `src/mcp/core/skill.rs`, `src/mcp/core/skill_auto.rs` |
| Skills config | `src/config/mod.rs` → `SkillsConfig` |
| Dynamic MCP servers | `src/mcp/core/dynamic.rs` |
| Dynamic agent tools | `src/mcp/core/dynamic_agents.rs` |
| MCP tool routing | `src/mcp/mod.rs` → `try_execute_tool_call()` |
| MCP tool registry | `src/mcp/tool_map.rs` |
| MCP tool definitions | `src/mcp/*/functions.rs` → `get_all_functions()` |
| MCP server init | `src/mcp/mod.rs` → `initialize_mcp_for_role()` |
| MCP health monitor | `src/mcp/health_monitor.rs` |
| MCP server connections | `src/mcp/server.rs` |
| Agent registry & capabilities | `src/agent/registry.rs` → `parse_capability_toml()` |
| Tap registry & discovery | `src/agent/taps.rs` |
| Agent inputs & placeholders | `src/agent/inputs.rs` ({{INPUT:KEY}}, {{ENV:KEY}}) |
| Agent dependency resolution | `src/agent/deps.rs` |
| AI provider bridge | `src/providers.rs` |
| Structured output (schema) | `src/providers.rs` → `ChatCompletionParams::with_schema()`, `src/session/mod.rs` → `chat_completion_with_provider()` |
| CLI schema flag parsing | `src/session/chat/session/params.rs`, `setup.rs` |
| Learning (extract, store, inject) | `src/learning/mod.rs`, `extract/`, `backends/`, `inject.rs` |
| Inbox (schedule + webhook) | `src/session/inbox.rs` |
| Background jobs | `src/session/background_jobs.rs` |
| Session context init | `src/session/context.rs` → `init_session_services()` |
| Sandbox | `src/sandbox/mod.rs` (Linux: seccomp, macOS: sandbox-exec) |
| ACP server | `src/acp/agent.rs`, `src/acp/commands.rs` |
| WebSocket server | `src/websocket/server.rs` |
| File read/write helpers | `src/utils/file_parser.rs`, `src/utils/file_renderer.rs` |
| CLI commands | `cli/src/commands/` |

## How Things Work

### Config is the Single Source of Truth

All defaults live in `config-templates/default.toml` (782 lines). The resolved config drives everything: model, MCP servers, layers, roles, workflows, commands, agents, compression, learning. No hardcoded values in code.

Config flow: `default.toml` → `load()` in `src/config/loading.rs` → merge with user config → `get_merged_config_for_role()` applies role overrides.

### MCP Tool Routing

1. `initialize_mcp_for_role()` builds the tool map from config-defined servers
2. `try_execute_tool_call()` looks up tool name in `TOOL_MAP` (global)
3. Routes to: builtin `core` (plan/mcp/agent/schedule/skill), builtin `agent` (agent_*), or external server (stdio/http)
4. Dynamic tools (added at runtime via `mcp`/`agent` tools) are registered in `tool_map.rs` and checked for session ownership
5. All errors return `Ok(McpToolResult::error())` — never `Err()` from tool execution

### Session Entry Points (CRITICAL — keep synchronized)

All session modes share the same initialization. When adding session-scoped state, it MUST be added to ALL entry points.

| Mode | File | Function |
|------|------|----------|
| Interactive CLI | `src/session/chat/session/main_loop.rs:100` | `run_interactive_session()` |
| Non-interactive CLI | `src/session/chat/session/main_loop.rs:959` | `run_interactive_session_with_input()` |
| ACP new_session | `src/acp/agent.rs:469` | `AcpAgent` — first `with_session_id` block |
| ACP initialize | `src/acp/agent.rs:917` | `AcpAgent` — second `with_session_id` block |
| WebSocket server | `src/websocket/server.rs:609` | `handle_session_message()` |

**Required initialization** (inside `with_session_id` context):
```rust
crate::session::context::init_session_services(&role);
```
This single call initializes inbox, job manager, and skill pool. Do NOT call `init_inbox_for_session`, `init_job_manager`, or `init_pool` directly.

**run_activation hook** (user input — only in main_loop.rs):
```rust
crate::mcp::core::skill_auto::run_activation(Event::User, &input, &current_dir).await
```

**run_validators hook** (after tool execution — only in tool_result_processor.rs):
```rust
crate::mcp::core::skill_auto::run_validators(Event::Turn, &content, &workdir).await
```

**If you add a new session entry point or session-scoped state, grep for `init_inbox_for_session` to find all locations that need updating.**

### Session Command Dispatch

`process_command()` in `commands/mod.rs` routes 25 slash-commands to handler modules. Unknown commands return `CommandResult::TreatAsUserInput`. Each command returns `CommandOutput` (strongly-typed enum with variants: Help, Info, Model, Role, Loglevel, Copy, Clear, Plan, Truncate, Summarize, etc.).

### Processing Pipeline

User input → command handling → `run_activation` hook → pipelines (deterministic scripts) → workflows (AI-orchestrated steps) → layers (AI sub-agents) → tool execution → `run_validators` hook → spending threshold check → response.

### Compression System

Four compression kinds: Task, Phase, Project, Conversation. Configured via `[compression]` in config with pressure levels (token thresholds → target ratios). Decision model configured separately in `[compression.decision]`. Knowledge retention preserves critical context across compressions.

### Learning System

Cross-session adaptive learning: extracts lessons from conversations, stores them (file or MCP backend), injects relevant ones into future sessions. Configured via `[learning]` section. Backends: `file` (default, zero deps) or `mcp` (external tool with field mapping).

### Sandbox

Platform-specific filesystem write restriction. Linux: Landlock/seccomp (kernel 5.13+). macOS: Seatbelt (`sandbox-exec`). Enabled via `sandbox = true` in config or `--sandbox` CLI flag.

## Code Quality Rules

### Copyright Header

Every `.rs` file must have:
```rust
// Copyright <YEAR> Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// ...full Apache 2.0 header...
```
- New files: use current year
- Modified files: update year if outdated
- Check: `rg -l "Copyright 2025" --type rust` should return nothing for files modified in the current year

### Build & Lint

```bash
cargo check --message-format=short                            # fast syntax check
cargo clippy --all-features --all-targets -- -D warnings      # must pass clean
cargo build                                                   # debug only -- never --release
```

### Errors — Fail Fast, Never Hide

```rust
// ✅ expose problems immediately
let config = load_config().expect("failed to load config");

// ❌ don't hide real problems
// let config = load_config().unwrap_or_else(|_| default_config());
```

### Logging — Never println in Library Code

```rust
crate::log_debug!("something happened");   // ✅ correct
// println!("DEBUG: ...");                  // ❌ breaks spinner, wrong output path
```

### MCP Tools — Errors Are Values, Not Panics

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

### MCP Misuse Hints — Guide the Model Toward Better Tools

When a tool is used where a dedicated tool would be better, append a hint. Never block execution. Only emit if the recommended tool is actually enabled.
```rust
let hint = if crate::mcp::tool_map::get_server_for_tool("better_tool").is_some() {
    "\n\n Prefer `better_tool` here -- reason."
} else { "" };
```
Reference: `src/mcp/core/schedule.rs` (schedule misuse hints), `src/mcp/hint_accumulator.rs` for pattern.

### Spinner-Aware Print Macros

`src/lib.rs` defines `println!`, `print!`, `eprintln!`, `eprint!` macros that automatically suspend the animation spinner before printing. These shadow `std::println!` etc. at the crate root. Always use these — never `std::println!` directly.

## Adding a New MCP Tool

1. Define in `src/mcp/*/functions.rs` → `get_all_functions()`
2. Implement in the same module
3. Route in `src/mcp/mod.rs` → `try_execute_tool_call()` (add to `route_builtin_tool` for core tools)
4. Register in `src/mcp/tool_map.rs` (tool name → server config mapping)
5. Return `Ok(McpToolResult::error())` for all failures — never `Err()`
6. Add misuse hints if a more specific tool should be preferred
7. For dynamic tools (added at runtime): use `register_dynamic_agent_tool()` or `register_dynamic_server_tools()` in `tool_map.rs`

## Adding a New Session Command

1. Create module in `src/session/chat/session/commands/<name>.rs`
2. Implement handler function returning `CommandResult`
3. Add `mod <name>;` and `pub use` in `commands/mod.rs`
4. Add `CommandOutput` variant to the `CommandOutput` enum in `commands/mod.rs`
5. Add routing in `process_command()` function

## Documentation Rules

- Single source of truth: each topic in exactly one file, others link to it
- Config examples must match `config-templates/default.toml` field names
- Paths must match `src/directories.rs` (data: `~/.local/share/octomind/`, config: `~/.local/share/octomind/config/config.toml`)
- MCP server type is `"stdio"` (not `"stdin"`); role format is `[[roles]]` with `name = "..."` (not `[role_name]`)
- Core MCP tools: `plan`, `mcp`, `agent`, `schedule`, `skill`; filesystem tools from external octofs

## Validation & Quality

```bash
cargo check --message-format=short                            # must pass
cargo clippy --all-features --all-targets -- -D warnings      # must pass clean
cargo build                                                   # must succeed (debug)
rg -l "Copyright 2025" --type rust                            # should return nothing (stale years)
```

- All `Err()` from tool execution wrapped as `Ok(McpToolResult::error())`
- No `std::println!` in library code — use `crate::log_debug!` or spinner-aware macros
- No `unwrap_or_else(|_| default)` patterns that hide failures
- New session-scoped state initialized in ALL five entry points
- Dynamic tools check session ownership before execution
- Config field names match `config-templates/default.toml`

## Debugging Starting Points

| Problem | Where to start |
|---------|----------------|
| Tool not routing | `src/mcp/mod.rs` → `build_tool_server_map()`, `try_execute_tool_call()` |
| Tool not found | `src/mcp/tool_map.rs` → `get_server_for_tool()` |
| Dynamic tool not appearing | `src/mcp/core/dynamic.rs`, `src/mcp/core/dynamic_agents.rs` |
| Config not loading | `src/config/loading.rs` → `load()` |
| Config migration issues | `src/config/migrations.rs` → `check_and_upgrade_config()` |
| Session command broken | `src/session/chat/session/commands/mod.rs` → `process_command()` |
| Layer not running | `src/session/layers/processor.rs` |
| Compression not working | `src/session/chat/conversation_compression.rs` |
| Structured output not working | `src/session/mod.rs` → `chat_completion_with_provider()`, `src/providers.rs` → `to_octolib_params()` |
| MCP server not starting | `src/mcp/server.rs`, `src/mcp/health_monitor.rs` |
| Skill auto-activation issues | `src/mcp/core/skill_auto.rs` |
| Learning not injecting | `src/learning/inject.rs` → `retrieve_and_format()` |
| ACP connection issues | `src/acp/agent.rs` → `authenticate()`, `new_session()` |
| Agent dependency resolution | `src/agent/deps.rs` → `resolve_deps()` |
| Placeholder substitution | `src/agent/inputs.rs` ({{INPUT:KEY}}, {{ENV:KEY}}) |

## Gotchas

- **Five session entry points** must stay in sync — grep `init_inbox_for_session` when adding state
- **Dynamic tools** have session-ownership checks — tools from other sessions are rejected
- **Spinner-aware macros** shadow `std::println!` — never use `std::println!` directly
- **Compression decision model** is separate from main model — `[compression.decision]`
- **Learning backend** defaults to `file` — `mcp` backend requires field mapping
- **Sandbox** is platform-specific — Linux: Landlock, macOS: Seatbelt; no Windows support

## Never

- Propagate `Err()` from MCP tool execution — always wrap as `Ok(McpToolResult::error())`
- Use `std::println!`/`std::eprintln!` in library code — use spinner-aware macros from `src/lib.rs`
- Use `unwrap_or_else(|_| default_config())` patterns that hide failures
- Add session-scoped state to only one entry point — must add to all five
- Hardcode config values — derive everything from `config-templates/default.toml`
- Use `"stdin"` as MCP server type — it's `"stdio"`
- Use `[role_name]` TOML sections — roles use `[[roles]]` with `name = "..."`
