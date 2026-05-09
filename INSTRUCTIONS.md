# Octomind Developer Guide

Session-based AI development assistant in Rust. Interactive chat sessions with MCP tools; AI calls tools to read/write files, search code, run shell commands, and delegate to sub-agents (layers). Multi-provider via [octolib](https://github.com/muvon/octolib).

## Project Structure

```
src/
  acp/              # Agent Communication Protocol server
  agent/            # Agent registry, taps, inputs, dependency resolution, resolver
  branding.rs       # Branding assets
  commands/         # CLI subcommands (acp, complete, config, run, send, server, tap, untap, vars)
  config/           # Config loading, roles, MCP, layers, pipelines, hooks, agents, providers,
                    #   workflows, validation, registry, env_source; log_debug!/log_info!/log_error! macros
  directories.rs    # Path constants (data dir, config dir, sessions, logs, cache)
  embeddings/       # Local embedding engine (fastembed, Xenova/bge-small-en-v1.5, cosine similarity)
  learning/         # Cross-session lesson extraction, storage, injection
  lib.rs            # Spinner-aware println!/print!/eprintln!/eprint! macros (shadow std)
  logging/          # ACP error sink (acp_error.rs), tracing setup (tracing_setup.rs)
  main.rs           # Entry point
  mcp/              # MCP protocol implementation
    core/
      capability.rs       # Capability system (list/enable/disable/discover/auto-activate)
      dynamic.rs          # Dynamic MCP server management (add/remove at runtime)
      dynamic_agents.rs   # Dynamic agent tool registration
      functions.rs        # Core builtin tool definitions → get_all_functions()
      plan/               # Plan tool (core, storage, memory_storage, compression)
      schedule/           # Schedule tool (core, storage)
      skill.rs            # Skill management
      skill_auto.rs       # Skill auto-activation & validation hooks
      skill_tests.rs      # Skill unit tests
      plan_tests.rs       # Plan tool unit tests
    agent/                # Agent tool routing (agent_* → layer execution)
    health_monitor.rs     # Server health checks, restart tracking
    hint_accumulator.rs   # Misuse hint accumulation
    mod.rs                # Tool routing, server init, try_execute_tool_call()
    process.rs            # Server process management, health enums
    server.rs             # JSON-RPC/SSE MCP server connections
    tool_map.rs           # Global TOOL_MAP: tool name → server config
    oauth/                # OAuth 2.1 + PKCE (discovery, flow, callback_server, cimd, token_store)
    workdir.rs            # Per-thread working directory tracking
    utils.rs / shared_utils.rs  # Tool call parsing, response formatting
  proctitle.rs       # Process title management
  providers.rs       # Thin wrapper over octolib (ChatCompletionParams, schemas)
  sandbox/           # Platform sandboxing (Linux Landlock, macOS Seatbelt)
  session/           # Session management
    chat/
      animation.rs / animation_manager.rs  # Spinner & animation
      assistant_output.rs    # Assistant output formatting
      command_executor.rs    # Command execution
      commands.rs            # Slash-command constants (COMMANDS array, 26 entries)
      context_truncation.rs  # Context truncation
      conversation_compression.rs  # Context compression (task/phase/project/conversation)
      cost_tracker.rs        # Token cost tracking
      edit_mode.rs           # Edit mode handling
      file_context.rs        # File context management
      formatting.rs          # Duration/content formatting utilities
      input.rs               # User input handling
      layered_response.rs    # Layered response processing
      markdown.rs            # Markdown processing
      message_handler.rs     # Message handling
      prompt.rs              # Prompt management
      reedline_adapter.rs    # Reedline line editor
      response.rs            # Response processing orchestrator (+ run_validators hook)
      response/
        tool_execution.rs    # Tool execution orchestration
        tool_result_processor.rs  # Tool result post-processing
      syntax.rs              # Syntax highlighting
      thinking_display.rs    # Thinking/reasoning display
      tool_display.rs        # Tool output display
      tool_error_tracker.rs  # Tool error tracking
      tool_processor.rs      # Tool call processing
      session/
        api_executor.rs      # API call execution
        api_prep.rs          # API call preparation
        commands/            # 23 session command handler modules
        core.rs              # ChatSession struct, SessionInitParams builder
        display.rs           # Session display
        error_utils.rs       # Error utilities
        layer_processor.rs   # Layer processing in session context
        main_loop.rs         # Interactive & non-interactive session loops
        messages.rs          # Message management
        params.rs            # CLI parameter parsing
        prompt_setup.rs      # Prompt setup
        setup.rs             # Session setup & initialization
        utils.rs             # Session utilities
    anchor.rs          # Session anchor management
    background_jobs.rs # Async agent job tracking
    cache.rs           # CacheManager for prompt caching
    cancellation.rs    # Cancellation tokens
    chat_helper.rs     # CommandCompleter (fuzzy autocomplete for reedline)
    completion.rs      # Completion logic
    context.rs         # Session-scoped state (with_session_id task-local), init_session_services()
    dedup.rs           # Deduplication utilities
    helper_functions.rs # Context summarization helpers
    history/           # Role-based history management
    image.rs / video.rs  # Image & video attachment processing
    inbox.rs           # InboxQueue (schedule + webhook message injection)
    inject_listener.rs # Unix Domain Socket for external message injection
    layers/            # Layer trait, LayerProcessor (layer_trait.rs, processor.rs)
    logger.rs          # Session logging
    modal.rs           # Terminal modal overlay system
    model_utils.rs     # Model utilities
    output.rs          # Output abstraction (JSONL, WebSocket, Silent sinks)
    persistence.rs     # Session save/restore
    pipelines/         # Deterministic script pipeline (orchestrator, executor)
    project_context.rs # Project context management
    prompt.rs          # Session-level prompt management
    report.rs          # Session usage reporting
    smart_summarizer.rs # Smart text summarization
    token_counter.rs   # Token counting
    workflows/         # WorkflowOrchestrator, StepExecutor, PatternParser
    webhook_listener.rs # HTTP webhook → inbox injection
  state.rs           # IndexState (current_directory, graphrag_blocks)
  utils/              # file_parser, file_renderer, glob, terminal_output, time, truncation
  websocket/          # WebSocket server (protocol, server)
config-templates/
  default.toml       # Single source of truth for all config defaults (729 lines)
  agents/            # Agent template files
  map-executor.toml  # Map executor config
  map-planner.toml   # Map planner config
```

## Where to Look

| Task / Area | Start here |
|-------------|------------|
| Config defaults & all fields | `config-templates/default.toml` |
| Config loading & types | `src/config/loading.rs`, `src/config/mod.rs` |
| Roles (model + MCP per role) | `src/config/roles.rs` |
| MCP server config | `src/config/mcp.rs` |
| Provider configs (API keys) | `src/config/providers.rs` |
| Layers config | `src/config/layers.rs` |
| Pipelines config | `src/config/pipelines.rs` |
| Workflows config | `src/config/workflows.rs` |
| Hooks config | `src/config/hooks.rs` |
| Config migrations | `src/config/migrations.rs` |
| Log macros (log_debug!/log_info!/log_error!) | `src/config/mod.rs` |
| Print macros (spinner-aware println! etc.) | `src/lib.rs` |
| Logging infrastructure (ACP sink, tracing) | `src/logging/` |
| Directory paths | `src/directories.rs` |
| Session init & state | `src/session/chat/session/core.rs` |
| Session main loop | `src/session/chat/session/main_loop.rs` |
| Session setup | `src/session/chat/session/setup.rs` |
| Session commands dispatch | `src/session/chat/session/commands/mod.rs` |
| Session command constants | `src/session/chat/commands.rs` |
| Conversation compression | `src/session/chat/conversation_compression.rs` |
| Response processing & validators hook | `src/session/chat/response.rs` |
| Tool execution orchestration | `src/session/chat/response/tool_execution.rs` |
| Layers runtime | `src/session/layers/layer_trait.rs`, `processor.rs` |
| Pipelines runtime | `src/session/pipelines/orchestrator.rs`, `executor.rs` |
| Workflows | `src/session/workflows/orchestrator.rs` |
| Skills (auto-activation, validation) | `src/mcp/core/skill.rs`, `src/mcp/core/skill_auto.rs` |
| Skills config | `src/config/mod.rs` → `SkillsConfig` |
| Capabilities (list/enable/disable/discover) | `src/mcp/core/capability.rs` |
| Embeddings (cosine similarity, embed) | `src/embeddings/mod.rs` |
| Dynamic MCP servers | `src/mcp/core/dynamic.rs` |
| Dynamic agent tools | `src/mcp/core/dynamic_agents.rs` |
| MCP tool routing | `src/mcp/mod.rs` → `try_execute_tool_call()` |
| MCP tool registry | `src/mcp/tool_map.rs` |
| MCP tool definitions | `src/mcp/*/functions.rs` → `get_all_functions()` |
| MCP server init | `src/mcp/mod.rs` → `initialize_mcp_for_role()` |
| MCP health monitor | `src/mcp/health_monitor.rs` |
| MCP server connections | `src/mcp/server.rs` |
| Agent registry, manifests, capabilities | `src/agent/registry.rs` → `parse_capability_toml()` |
| Agent config/role resolution | `src/agent/resolver.rs` |
| Agent dependency resolution | `src/agent/deps.rs` |
| Placeholder substitution ({{INPUT:KEY}}) | `src/agent/inputs.rs` |
| CLI commands | `src/commands/` |
| AI provider bridge | `src/providers.rs` |
| Structured output (schema) | `src/providers.rs` → `ChatCompletionParams::with_schema()`, `src/session/mod.rs` → `chat_completion_with_provider()` |
| CLI schema flag parsing | `src/session/chat/session/params.rs`, `setup.rs` |
| Learning (extract, store, inject) | `src/learning/mod.rs`, `src/learning/extract.rs`, `src/learning/backend/`, `src/learning/inject.rs` |
| Session context & all session state | `src/session/context.rs` → `init_session_services()` |
| Inbox (schedule + webhook) | `src/session/inbox.rs` |
| Background jobs | `src/session/background_jobs.rs` |
| Sandbox | `src/sandbox/mod.rs`, `src/sandbox/linux.rs`, `src/sandbox/macos.rs` |
| ACP server | `src/acp/agent.rs`, `src/acp/commands.rs` |
| WebSocket server | `src/websocket/server.rs` |
| Image/video processing | `src/session/image.rs`, `src/session/video.rs` |
| File read/write helpers | `src/utils/file_parser.rs`, `src/utils/file_renderer.rs` |

## How Things Work

### Config is the Single Source of Truth

All defaults live in `config-templates/default.toml` (729 lines). The resolved config drives everything: model, MCP servers, layers, roles, workflows, commands, agents, compression, learning. No hardcoded values in code.

Config flow: `default.toml` → `load()` in `src/config/loading.rs` → merge with user config → `get_merged_config_for_role()` applies role overrides.

### MCP Server Activation Flow

How a server goes from config TOML to callable tool in a session:

```
Config::load()
  └─ load_and_merge_toml_from_directory(config_dir)
       ├─ regular *.toml files (config.toml, mcp.toml, roles.toml, …) — alphabetical
       └─ mcp-*.toml override files                                    — alphabetical, LAST
       merge_toml_values: tables deep-merge, [[arrays of tables]] concat + dedup by `name`
       (last entry wins → mcp-*.toml overrides anything with the same server name)
         → toml::Value → Config (serde) → config.mcp.servers = global registry
         → config.build_role_map() indexes roles by full name (including `domain:spec`)

resolve_config_and_role(tag)       [CLI `run`/`acp`/`server` + `/role`]
  ├─ plain tag  (`developer`)               → (config, "developer")
  └─ agent tag (`developer:general`)        → fetch tap manifest, resolve
                                              capabilities/inputs/env/deps,
                                              merge_agent_toml(base, manifest),
                                              inject role name = full tag
                                              → (merged_config, "developer:general")

get_merged_config_for_role(role)   [src/config/mod.rs]
  └─ get_enabled_servers_for_role
       ├─ EXPLICIT : servers named in role.mcp.server_refs
       └─ AUTO-BIND: servers whose auto_bind contains the role name (exact match,
                     including `:` — `auto_bind = ["developer:general"]`)
  └─ PATCH role_map[role] so downstream readers see a consistent view:
       • server_refs  += auto-bind names
       • allowed_tools += "<name>:*" for each auto-bind server (ONLY if allowed_tools
                          was non-empty; empty = unrestricted, nothing to patch)
  → merged.mcp.servers = only this role's active servers

initialize_mcp_for_role(role, merged_config)
  ├─ initialize_servers_for_role  — spawns stdio / opens http / registers builtins
  └─ tool_map::initialize_tool_map — builds global TOOL_MAP

TOOL_MAP (tool_name → McpServerConfig)
  ├─ config servers   (merged_config.mcp.servers)
  ├─ dynamic servers  (mcp add/enable at runtime)
  └─ dynamic agents   (agent add/enable at runtime)
```

**Load order matters.** `mcp-*.toml` files load AFTER base files (regardless of alphabetical position) so they override same-named servers in `mcp.toml`. See `is_mcp_extension_file` in `src/config/loading.rs`.

**Persisted servers** (`mcp persist`): writes `<config_dir>/mcp-<name>.toml` with `auto_bind = ["<role>"]`. Picked up automatically on next startup. No manual `server_refs` edit needed.

**`allowed_tools` gotcha**: a non-empty `allowed_tools` list silently filters tools from servers NOT listed. `get_merged_config_for_role` auto-appends `"<server>:*"` for every auto-bind server to prevent this. Empty list = no restriction.

**Role name = full tag.** Agent tags like `developer:general` become the literal role name in `role_map` and in `auto_bind` matching. `auto_bind = ["developer"]` will NOT match `"developer:general"`.

### MCP Tool Routing

1. `initialize_mcp_for_role()` builds the tool map from config-defined servers
2. `try_execute_tool_call()` looks up tool name in `TOOL_MAP` (global)
3. Routes to: builtin `core` (plan/mcp/agent/schedule/skill/capability), builtin `agent` (agent_*), or external server (stdio/http)
4. Dynamic tools (added at runtime via `mcp`/`agent` tools) are registered in `tool_map.rs` and checked for session ownership
5. All errors return `Ok(McpToolResult::error())` — never `Err()` from tool execution

### Session Entry Points (CRITICAL — keep synchronized)

All session modes share the same initialization. When adding session-scoped state, it MUST be added to ALL entry points.

| Mode | File | `init_session_services` call site |
|------|------|-----------------------------------|
| Interactive CLI | `src/session/chat/session/main_loop.rs` | line 174 |
| Non-interactive CLI | `src/session/chat/session/main_loop.rs` | line 1153 |
| ACP new_session | `src/acp/agent.rs` | line 508 |
| ACP initialize | `src/acp/agent.rs` | line 966 |
| WebSocket server | `src/websocket/server.rs` | line 613 |

**Required initialization** (inside `with_session_id` context):
```rust
crate::session::context::init_session_services(&role);
```
This single call initializes inbox, job manager, and skill pool. Do NOT call `init_inbox_for_session`, `init_job_manager`, or `init_pool` directly.

**run_activation hook** (user input — only in `main_loop.rs`):
```rust
crate::mcp::core::skill_auto::run_activation(Event::User, &input, &current_dir).await
```

**run_validators hook** (after tool execution — only in `response.rs`):
```rust
crate::mcp::core::skill_auto::run_validators(&current_content, &workdir).await
```

### Session Command Dispatch

`process_command()` in `commands/mod.rs` routes 25 slash-commands (constants in `src/session/chat/commands.rs`) to handler modules. Unknown commands return `CommandResult::TreatAsUserInput`. Each command returns `CommandOutput` (strongly-typed enum). `/done` is intercepted in `main_loop.rs` before reaching `process_command`.

### Processing Pipeline

User input → command handling → `run_activation` hook → pipelines (deterministic scripts) → workflows (AI-orchestrated steps) → layers (AI sub-agents) → tool execution → `run_validators` hook → spending threshold check → response.

### Compression System

Four compression kinds: Task, Phase, Project, Conversation. Configured via `[compression]` in config with pressure levels (token thresholds → target ratios). Decision model configured separately in `[compression.decision]`. Knowledge retention preserves critical context across compressions.

### Learning System

Cross-session adaptive learning: extracts lessons from conversations, stores them (file or MCP backend), injects relevant ones into future sessions. Configured via `[learning]` section. Backends: `file` (default, zero deps) or `mcp` (external tool with field mapping).

### Sandbox

Platform-specific filesystem write restriction. Linux: Landlock/seccomp (kernel 5.13+). macOS: Seatbelt (`sandbox-exec`). Enabled via `sandbox = true` in config or `--sandbox` CLI flag. No Windows support.

## Code Patterns

### Copyright Header

Every `.rs` file must have:
```rust
// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// ...full Apache 2.0 header...
```
New files: use current year. Modified files: update year if outdated.

### Errors — Fail Fast, Never Hide

```rust
// ✅ expose problems immediately
let config = load_config().expect("failed to load config");

// ❌ don't hide real problems
let config = load_config().unwrap_or_else(|_| default_config());
```

### Logging Macros

Defined in `src/config/mod.rs`. Mode-aware: CLI → colored terminal output; ACP/WebSocket → tracing to file.

```rust
crate::log_debug!("something happened");   // debug-level, bright blue in CLI
crate::log_info!("something happened");    // info-level, cyan in CLI
crate::log_error!("something happened");   // always visible; ACP also writes JSONL sink
// println!("DEBUG: ...");                  // ❌ breaks spinner, wrong output path
```

### Spinner-Aware Print Macros

`src/lib.rs` defines `println!`, `print!`, `eprintln!`, `eprint!` that automatically suspend the animation spinner. These shadow `std::println!` etc. at the crate root. Always use these — never `std::println!` directly.

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
Reference: `src/mcp/core/schedule/core.rs` (schedule misuse hints), `src/mcp/hint_accumulator.rs` for pattern.

## Adding a New MCP Tool

1. Define in `src/mcp/*/functions.rs` → `get_all_functions()`
2. Implement in the same module
3. Route in `src/mcp/mod.rs` → `try_execute_tool_call()` (add to `route_builtin_tool` for core tools)
4. Register in `src/mcp/tool_map.rs` (tool name → server config mapping)
5. Return `Ok(McpToolResult::error())` for all failures — never `Err()`
6. Add misuse hints if a more specific tool should be preferred
7. For dynamic tools: use `register_dynamic_agent_tool()` or `register_dynamic_server_tools()` in `tool_map.rs`

## Adding a New Session Command

1. Create module in `src/session/chat/session/commands/<name>.rs`
2. Implement handler function returning `CommandResult`
3. Add `mod <name>;` and routing in `commands/mod.rs` → `process_command()`
4. Add `CommandOutput` variant to the `CommandOutput` enum in `commands/mod.rs`
5. Add command constant to `src/session/chat/commands.rs` and the `COMMANDS` array

## Documentation Rules

- Single source of truth: each topic in exactly one file, others link to it
- Config examples must match `config-templates/default.toml` field names
- Paths must match `src/directories.rs` (data: `~/.local/share/octomind/`, config: `~/.local/share/octomind/config/config.toml`)
- MCP server type is `"stdio"` (not `"stdin"`); role format is `[[roles]]` with `name = "..."` (not `[role_name]`)
- Core MCP tools: `plan`, `mcp`, `agent`, `schedule`, `skill`, `capability`; filesystem tools from external octofs

## Debugging Starting Points

| Problem | Where to start |
|---------|----------------|
| Tool not routing | `src/mcp/mod.rs` → `try_execute_tool_call()`, `route_builtin_tool()` |
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

- **Five session entry points** must stay in sync — grep `init_session_services` when adding session-scoped state
- **Dynamic tools** have session-ownership checks — tools from other sessions are rejected
- **Spinner-aware macros** shadow `std::println!` — never use `std::println!` directly
- **Log macros** are in `src/config/mod.rs`, not `src/lib.rs` — `lib.rs` only has print macros
- **Compression decision model** is separate from main model — `[compression.decision]`
- **Learning backend** defaults to `file` — `mcp` backend requires field mapping
- **`/done` command** is intercepted in `main_loop.rs` before `process_command()` — the `DONE_COMMAND` branch in `process_command` is `unreachable!()`

## Never

- Propagate `Err()` from MCP tool execution — always wrap as `Ok(McpToolResult::error())`
- Use `std::println!`/`std::eprintln!` in library code — use spinner-aware macros from `src/lib.rs`
- Use `unwrap_or_else(|_| default_config())` patterns that hide failures
- Add session-scoped state to only one entry point — must add to all five
- Hardcode config values — derive everything from `config-templates/default.toml`
- Use `"stdin"` as MCP server type — it's `"stdio"`
- Use `[role_name]` TOML sections — roles use `[[roles]]` with `name = "..."`
