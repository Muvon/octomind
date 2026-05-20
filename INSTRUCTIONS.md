# Octomind — AI Development Assistant (Rust)

Session-based AI assistant where the model calls MCP tools (read/write files, search, shell, delegate) to do real work. Sessions run interactively (CLI), non-interactively (`--format`), or as daemons (ACP/WebSocket). Config is the single source of truth — all runtime behavior (model, tools, roles, compression, learning) derives from TOML. Multi-provider via `octolib`. Rust 1.95+, tokio async, `clap` CLI.

## Project Structure

```
src/
  main.rs              # Entry: CLI parsing → Config::load() → subcommand dispatch
  lib.rs               # Spinner-aware print macros (shadow std::println! etc.)
  config/              # Config types, loading, migrations, log macros
  mcp/                 # Tool routing, server lifecycle, all builtin tools
    core/              # plan, tap, capability, skill, local_tool, dynamic servers
    runtime/           # mcp, agent, skill, schedule, capability tools
    agent/             # agent_* tool routing → layer/subprocess
  session/
    chat/session/      # ChatSession: init, main loop, command dispatch, API calls
    chat/              # Response processing, tool execution, compression, display
    context.rs         # Session-scoped state (task-local SessionId propagation)
    layers/            # AI sub-agent execution
    pipelines/         # Deterministic script pipelines
    workflows/         # AI-orchestrated multi-step workflows
    learning/          # Cross-session lesson extraction/injection
  acp/                 # ACP stdio server (agent-to-agent protocol)
  websocket/           # WebSocket server for remote sessions
  agent/               # Tap registry, manifest resolution, dependency resolution
  commands/            # CLI subcommand handlers
config-templates/
  default.toml         # ALL config fields with defaults — single source of truth
  agents/              # Agent template files
```

## Where to Look First

| Task | Start here |
|------|------------|
| Add a new MCP tool | `src/mcp/core/functions.rs` (core) or `src/mcp/runtime/mod.rs` (runtime) → then route in `src/mcp/mod.rs` |
| Add a session command (`/foo`) | `src/session/chat/session/commands/` → `mod.rs` → `src/session/chat/commands.rs` |
| Change a config field/default | `config-templates/default.toml` first, then matching type in `src/config/` |
| Trace a config load failure | `src/config/loading.rs` → `load()` |
| Understand MCP server activation | `src/config/mod.rs` → `get_merged_config_for_role()` |
| Debug tool not found/routing | `src/mcp/tool_map.rs` → `get_server_for_tool()`, then `src/mcp/mod.rs` → `try_execute_tool_call()` |
| Session init / state management | `src/session/context.rs` → `init_session_services()` |
| Session main loop | `src/session/chat/session/main_loop.rs` |
| Response / tool execution flow | `src/session/chat/response.rs` → `src/session/chat/response/tool_execution.rs` |
| Skill auto-activation | `src/mcp/core/skill_auto.rs` |
| Layer / pipeline / workflow | `src/session/layers/`, `src/session/pipelines/`, `src/session/workflows/` |
| Learning system | `src/learning/` |
| ACP server | `src/acp/agent.rs` |
| Sandbox | `src/sandbox/mod.rs` |
| Directory path constants | `src/directories.rs` |

## Architecture: The Flows That Matter

### Config → Role → Tools (activation chain)

```
Config::load()
  └─ merge all *.toml in config_dir (alphabetical)
     then mcp-*.toml files AFTER base files (override same-named servers)
     arrays: concat + dedup by `name`; tables: deep-merge

get_merged_config_for_role(role)            [src/config/mod.rs]
  └─ collects servers: explicit server_refs UNION auto_bind matches
     auto_bind matches on EXACT string — "developer" ≠ "developer:general"
  └─ result: merged config with only this role's servers visible

initialize_mcp_for_role(role, merged_config)
  └─ spawns stdio / opens http / registers builtins
  └─ builds TOOL_MAP: tool_name → McpServerConfig

try_execute_tool_call(call)                 [src/mcp/mod.rs]
  └─ TOOL_MAP lookup → routes to: core | runtime | agent | local | external
```

**Key rules:**
- `mcp-*.toml` always loads after `mcp.toml` regardless of filename sort order — use this for overrides
- `mcp persist` writes `<config_dir>/mcp-<name>.toml` with `auto_bind = ["<role>"]` — picked up on next start
- `allowed_tools` non-empty → silently filters tools not listed; `get_merged_config_for_role` auto-appends `"<server>:*"` for auto-bind servers to prevent accidental filtering

### Session Lifecycle (CRITICAL INVARIANT)

Five entry points all share the same initialization contract. When adding session-scoped state, ALL five must be updated:

| Mode | Entry point |
|------|-------------|
| Interactive + non-interactive CLI | `src/session/chat/session/main_loop.rs` → `init_session_runtime()` |
| ACP new_session | `src/acp/agent.rs` ~line 568 |
| ACP initialize | `src/acp/agent.rs` ~line 1166 |
| WebSocket | `src/websocket/server.rs` ~line 625 |

Required inside `with_session_id` context:
```rust
crate::session::context::init_session_services(&role);
// Initializes inbox, job manager, skill pool, schedule storage in one call.
// Never call init_inbox_for_session / init_job_manager / etc. directly.
```

### Processing Pipeline

```
User input
  → /command? → CommandResult (or TreatAsUserInput)
  → run_activation hook (main_loop.rs only)  [skill auto-activation on user input]
  → pipelines (deterministic scripts)
  → workflows (AI-orchestrated steps)
  → layers (AI sub-agents)
  → tool execution loop
  → run_validators hook (response.rs only)   [skill validators after tool use]
  → spending check
  → response output
```

`/done` is intercepted in `main_loop.rs` before reaching `process_command()` — the `DONE_COMMAND` branch in `process_command` is `unreachable!()`.

## Code Patterns

### Copyright header — every `.rs` file

```rust
// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
```

### Errors: fail fast, never hide

```rust
// ✅
let config = load_config().expect("failed to load config");

// ❌ hides real problems
let config = load_config().unwrap_or_else(|_| default_config());
```

### MCP tools: errors are values, never panics

```rust
// ✅ parameter validation
let param = match call.parameters.get("key") {
    Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
    Some(_) => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "must be string".into())),
    None    => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "required".into())),
};

// ✅ routing — wrap Err, never propagate
match tool::execute(call).await {
    Ok(mut r) => { r.tool_id = call.tool_id.clone(); Ok(r) }
    Err(e)    => Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("{e}")))
}
```

### Logging macros (defined in `src/config/mod.rs`)

```rust
crate::log_debug!("detail");   // bright blue in CLI; tracing in ACP/WS
crate::log_info!("status");    // cyan in CLI
crate::log_error!("failure");  // always visible; ACP also writes JSONL sink

// ❌ breaks spinner AND wrong output path
println!("DEBUG: ...");
std::println!("...");
```

### Print macros (`src/lib.rs` — shadow std)

`println!`, `print!`, `eprintln!`, `eprint!` in this crate automatically suspend the animation spinner. Always use these. Never call `std::println!` directly.

### MCP misuse hints — guide, never block

When a dedicated tool would be better, append a hint to the result — but only if that tool is actually enabled:
```rust
let hint = if crate::mcp::tool_map::get_server_for_tool("better_tool").is_some() {
    "\n\n💡 Prefer `better_tool` here — reason."
} else { "" };
```
See `src/mcp/hint_accumulator.rs` and `src/mcp/core/schedule/core.rs` for examples.

### Formatting: hard tabs

`rustfmt.toml` sets `hard_tabs = true`. Run `cargo fmt --all` before every commit.

## Adding a New MCP Tool

1. **Define** in `src/mcp/core/functions.rs` → `get_all_functions()` (core server) **or** `src/mcp/runtime/mod.rs` → `get_all_functions()` (runtime server)
2. **Implement** in the same file/module
3. **Route** in `src/mcp/mod.rs` → `route_builtin_tool()` — add a match arm for `"core"` or `"runtime"`
4. **Register** in `src/mcp/tool_map.rs` — add tool name → server config mapping
5. All failures → `Ok(McpToolResult::error(...))` — never `Err()`
6. Add misuse hints where a more specific tool exists
7. Dynamic tools: use `register_dynamic_agent_tool()` or `register_dynamic_server_tools()` in `tool_map.rs`
8. Local project tools: drop executable scripts into `<workdir>/.agents/tools/` (auto-discovered by `src/mcp/core/local_tool.rs`)

## Adding a New Session Command (`/name`)

1. Create `src/session/chat/session/commands/<name>.rs`, implement handler returning `CommandResult`
2. Add `mod <name>;` and routing arm in `commands/mod.rs` → `process_command()`
3. Add `CommandOutput` variant to the enum in `commands/mod.rs` if the command has a new result shape
4. Add constant to `src/session/chat/commands.rs` and the `COMMANDS` array

## Development Workflow

```bash
cargo build                                                  # dev build
cargo build --release                                        # release build
cargo fmt --all                                              # format (must pass CI)
cargo clippy --all-targets --all-features -- -D warnings    # lint (must pass CI)
cargo test --release                                         # tests
make dev                                                     # fmt + clippy + test
make pre-commit                                              # full pre-commit suite
```

CI checks (all must pass): `cargo check` → `fmt --check` → `clippy -D warnings` → `cargo test`.

## Validation Checklist

Before any commit:
- [ ] `cargo fmt --all` passes (hard tabs — zero diff)
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings
- [ ] `cargo test --release` — all pass
- [ ] Apache 2.0 copyright header on every new `.rs` file
- [ ] No `std::println!` / `std::eprintln!` — use crate macros
- [ ] No `unwrap_or_else(|_| ...)` that swallows real errors
- [ ] MCP tool failures return `Ok(McpToolResult::error(...))` not `Err(...)`
- [ ] Session-scoped state added to all five entry points (grep `init_session_services`)
- [ ] New config fields added to `config-templates/default.toml` first, then matching Rust type

## Gotchas

- **`mcp-*.toml` load order** — loads AFTER all base `*.toml` files regardless of sort order. `mcp-foo.toml` always wins over `mcp.toml` for same-named servers. This is the intended override mechanism.
- **`auto_bind` is exact-match** — `"developer"` will NOT match role `"developer:general"`. Use the full tag in both places.
- **`allowed_tools` non-empty silently filters** — any server not in the list has its tools dropped. `get_merged_config_for_role` auto-appends `"<server>:*"` for auto-bind servers, but watch for this when constructing configs manually.
- **Five session entry points must stay in sync** — grep `init_session_services` before adding session-scoped state.
- **`/done` bypasses `process_command`** — intercepted in `main_loop.rs`; the `DONE_COMMAND` arm in `process_command` is `unreachable!()`.
- **Log macros live in `src/config/mod.rs`**, not `src/lib.rs`. `lib.rs` only has the print macros.
- **Core vs runtime builtins** — `core` server: `plan`, `tap`. `runtime` server: `mcp`, `agent`, `skill`, `schedule`, `capability`. Separate match arms in `route_builtin_tool()`.
- **Dynamic tool session ownership** — tools registered by one session are rejected from another. Intentional isolation.
- **Compression decision model** is separate from the main model — configured at `[compression.decision]` in config, not `model`.

## Never

- Return `Err()` from MCP tool execution — always `Ok(McpToolResult::error(...))`
- Use `std::println!` / `std::eprintln!` anywhere in crate code — breaks the spinner
- Use `unwrap_or_else(|_| default)` patterns that swallow real errors
- Add session-scoped state to only some entry points — all five or none
- Hardcode config values — all defaults belong in `config-templates/default.toml`
- Use `"stdin"` as MCP server type — correct value is `"stdio"`
- Use `[role_name]` TOML sections for roles — always `[[roles]]` with `name = "..."`
- Omit the Apache 2.0 copyright header from a new `.rs` file
