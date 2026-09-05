# Contributing to Octomind

Thank you for your interest in contributing. This document covers everything you need to ship correct, maintainable code.

---

## Table of Contents

- [Development Setup](#development-setup)
- [Code Style](#code-style)
- [Architecture Overview](#architecture-overview)
- [Adding Features](#adding-features)
  - [New MCP Tool](#new-mcp-tool)
  - [New Session Command](#new-session-command)
  - [New Config Field](#new-config-field)
  - [New Session-Scoped State](#new-session-scoped-state)
- [Rust Patterns](#rust-patterns)
- [Error Handling](#error-handling)
- [Logging](#logging)
- [Testing](#testing)
- [Pre-Commit Checks](#pre-commit-checks)
- [Commit Messages](#commit-messages)
- [Validation Checklist](#validation-checklist)

---

## Development Setup

```bash
# Clone and build
git clone https://github.com/muvon/octomind
cd octomind
cargo build

# Install pre-commit hooks (required, per clone)
pip install pre-commit
pre-commit install          # or: make pre-commit-install

# Run checks manually (same flags as the hooks and CI)
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo check --all-targets --all-features
cargo test

# One-shot: fmt + clippy + test
make dev
```

**Minimum Rust version:** 1.95 (enforced by `rust-version` in `Cargo.toml`).

See [Building from Source](doc/dev/01-building-from-source.md) for prerequisites, Make targets, and cross-compilation.

---

## Code Style

### Formatting

- **Tabs, not spaces** — `rustfmt.toml` sets `hard_tabs = true`
- **LF line endings**, UTF-8, trailing newline on every file (`.editorconfig`)
- **120-character line limit**
- Run `cargo fmt --all` before committing; the pre-commit hook enforces this

### Copyright header

Every new `.rs` file **must** begin with:

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

### Print macros

Use the crate-level macros (`println!`, `eprintln!`, `print!`, `eprint!`) defined in `src/lib.rs`. They suspend the animation spinner automatically.

```rust
// ✅ correct
println!("output");
eprintln!("error");

// ❌ breaks the spinner and wrong output path
std::println!("output");
std::eprintln!("error");
```

---

## Architecture Overview

```
Config::load()  →  get_merged_config_for_role(role)  →  initialize_mcp_for_role()
                                                              └─ TOOL_MAP built

User input  →  /command?  →  guardrails/pipe  →  workflows  →  layers
                                                             └─  tool execution loop  →  response
```

Key files:

| What | Where |
|------|-------|
| Tool routing | `src/mcp/mod.rs` → `try_execute_tool_call()` / `route_builtin_tool()` |
| Tool map (name → server) | `src/mcp/tool_map.rs` |
| Builtin `core` tools | `src/mcp/core/functions.rs` |
| Builtin `orchestration` tools | `src/mcp/orchestration/mod.rs` |
| Builtin `runtime` tools | `src/mcp/runtime/mod.rs` |
| Builtin `agent_*` tools | `src/mcp/agent/functions.rs` |
| Session main loop | `src/session/chat/session/main_loop.rs` |
| Session commands | `src/session/chat/session/commands/` |
| Session-scoped services | `src/session/context.rs` → `init_session_services()` |
| Config types | `src/config/` |
| Config defaults | `config-templates/default.toml` |
| Directory constants | `src/directories.rs` |

Builtin servers (each has its own match arm in `route_builtin_tool()` and `tool_map`):

| Server | Tools |
|--------|-------|
| `core` | `recall` (conditional) |
| `orchestration` | `tap`, `schedule`, `monitor` |
| `runtime` | `mcp`, `agent`, `skill`, `capability` |
| `agent` | generated `agent_<name>` delegation tools |
| `local` | scripts auto-discovered from `<workdir>/.agents/tools/` |

See [Architecture](doc/dev/02-architecture.md) for module-by-module detail.

### Config merge rules

- All `*.toml` files in the config directory are merged alphabetically
- Files matching `mcp-*.toml` are loaded **after** all base files — they always win for same-named servers
- Arrays are concatenated and deduped by `name`; tables are deep-merged
- `auto_bind` is **exact-match** — `"developer"` does not match `"developer:general"`
- `allowed_tools` non-empty silently drops any unlisted tool — `get_merged_config_for_role` auto-appends `"<server>:*"` for auto-bind servers

---

## Adding Features

### New MCP Tool

1. **Define** the tool schema in the server's `get_all_functions()` — `src/mcp/core/functions.rs`, `src/mcp/orchestration/mod.rs`, or `src/mcp/runtime/mod.rs`
2. **Implement** the handler in the same module
3. **Route** in `src/mcp/mod.rs` → `route_builtin_tool()` — add a match arm under the server name
4. **Register** in `src/mcp/tool_map.rs` — map the tool name to its server config

Rules:
- All failures return `Ok(McpToolResult::error(...))` — never `Err()`
- Validate parameters explicitly; return a descriptive error for missing/wrong-type params
- If a more specific tool exists for a use-case, append a hint (but only when that tool is actually enabled — see `src/mcp/hint_accumulator.rs`)
- Dynamic tools use `register_dynamic_agent_tool()` / `register_dynamic_server_tools()` in `tool_map.rs`; tools registered by one session are rejected from another

```rust
// ✅ parameter extraction pattern
let param = match call.parameters.get("key") {
    Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
    Some(_) => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "must be a non-empty string".into())),
    None    => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "required parameter".into())),
};

// ✅ wrapping internal errors
match tool::execute(call).await {
    Ok(mut r) => { r.tool_id = call.tool_id.clone(); Ok(r) }
    Err(e)    => Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), format!("{e}")))
}
```

### New Session Command (`/name`)

1. Create `src/session/chat/session/commands/<name>.rs` — implement the handler returning `CommandResult`
2. Add `mod <name>;` in `commands/mod.rs` and a routing arm in `process_command()`
3. If the command has a new result shape, add a `CommandOutput` variant to the enum in `commands/mod.rs`
4. Add the constant to `src/session/chat/commands.rs` and the `COMMANDS` array (it is a fixed-size array — bump the length)

Note: the CLI main loop and the ACP prompt path intercept `/done` before dispatch, but the ACP `octomind/command` ext-method and WebSocket command messages reach `process_command()` directly. The `DONE_COMMAND` arm there is real and transport-agnostic — keep it working.

### New Config Field

1. Add the field with its default value to `config-templates/default.toml` **first**
2. Add the corresponding field to the matching Rust type in `src/config/`
3. Never hardcode config values — all defaults belong in the template

### New Session-Scoped State

There are **four** session entry points that must all be updated together:

| Mode | Location |
|------|----------|
| Interactive / non-interactive CLI | `src/session/chat/session/main_loop.rs` → `init_session_runtime()` |
| ACP `new_session` | `src/acp/agent.rs` |
| ACP `initialize` | `src/acp/agent.rs` |
| WebSocket | `src/websocket/server.rs` |

All session-scoped services are initialized through a single call inside the `with_session_id` context:

```rust
crate::session::context::init_session_services(&role);
```

Never call `init_inbox_for_session`, `init_job_manager`, or similar directly. If you add new session-scoped state, add it inside `init_session_services` and verify every entry point still calls it (`rg init_session_services src`).

---

## Rust Patterns

### Ownership

- Default to owned types in struct fields (`String`, `Vec<T>`, `PathBuf`) — borrowed fields force lifetime parameters that infect callers
- Borrow in function signatures: `&str`, `&[T]`, `&Path`; return owned values
- Reach for `Cow<'a, T>` only when benchmarks show real allocation pressure

### Shared state

- `std::sync::Mutex` held across `.await` is a deadlock — use `tokio::sync::Mutex` or an actor with a channel
- Interior mutability (`RefCell`, `Mutex`) signals shared-state design — prefer ownership transfer or message passing
- Session-keyed state lives in `src/session/context.rs` and is looked up via the task-local `SessionId` — never in process globals

### Abstractions

- Define a trait when there are two real implementations, not in anticipation
- Static dispatch (`fn f<T: Trait>`) is the default; `dyn Trait` only for type erasure
- Newtype pattern (`struct UserId(u64)`) for domain types — prevents mixing primitives

### General

- No wrapper methods (inline 1–3 line delegates instead)
- No `unwrap_or_else(|_| default)` that swallows real errors
- No speculative abstractions — YAGNI applies strictly
- Comments explain **why**, not what — remove dead code rather than comment it out
- Named constants, no magic numbers

---

## Error Handling

```rust
// ✅ fail fast — surfaces the real problem
let config = load_config().context("failed to load config")?;

// ❌ hides real problems
let config = load_config().unwrap_or_else(|_| default_config());
```

- The crate uses `anyhow::Result` throughout; add `.context(...)` at the point where the caller loses the original meaning (file path, server name, session name)
- `thiserror` enums only where callers must match on variants (`src/session/cancellation.rs`, `src/mcp/oauth/token_store.rs`) — don't introduce one for a single call site
- Validate at real boundaries (user input, external APIs, MCP responses), not at internal seams you control
- When we own the schema, required fields fail loudly — no silent fallbacks or graceful degradation
- MCP tool execution: **always** `Ok(McpToolResult::error(...))`, never `Err()`

---

## Logging

Log macros are defined in `src/config/mod.rs`:

```rust
crate::log_debug!("detail");   // bright blue in CLI; tracing in ACP/WS
crate::log_info!("status");    // cyan in CLI
crate::log_error!("failure");  // always visible; ACP also writes JSONL sink
```

- Use these macros exclusively — raw `println!` bypasses the spinner and wrong output path
- Pass values as positional args (`log_debug!("x={}", x)`), not inline captures (`{x}`) — clippy runs with `-D warnings` and cannot see through the macro
- No trailing comma after the last argument — the macros take `$($arg:expr),*`
- Log **decisions and state transitions**, not mechanical step-by-step tracing
- Do not add logging to investigate a bug you can fix directly

---

## Testing

- Unit test bodies live in sibling `<module>_tests.rs` files; the production file only declares the module:

  ```rust
  #[cfg(test)]
  #[path = "feature_tests.rs"]
  mod tests;
  ```

  Never add inline `#[cfg(test)] mod tests { ... }` bodies to production `.rs` files. Use a descriptive sibling name (`foo_inline_tests.rs`, `foo_coverage_tests.rs`) when one source file needs several test modules.
- Integration tests in `tests/` exercise the binary and protocols end-to-end (ACP, WebSocket, PTY, MCP stub server); shell smoke tests live in `tests/smoke/`
- In-crate tests that need a real LLM round trip use the fake-provider harness in `src/session/chat/test_support.rs`
- Do not add tests speculatively — write them for real invariants
- `make coverage` runs `cargo-llvm-cov` with `_tests.rs` files excluded so percentages describe product code

---

## Pre-Commit Checks

The pre-commit hooks run automatically on `git commit`:

| Hook | Command |
|------|---------|
| Format | `cargo fmt --all` |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` |
| Compile | `cargo check --all-targets --all-features` |
| Trailing whitespace | `trailing-whitespace` |
| File endings | `end-of-file-fixer` |
| Merge conflicts | `check-merge-conflict` |
| TOML / YAML syntax | `check-toml`, `check-yaml` |
| Large files | `check-added-large-files` (max 1000 KB) |

Install once with `pre-commit install`. All hooks must pass before a commit lands. `make pre-commit` runs every hook over all files.

CI (`.github/workflows/ci.yml`) runs the test suite across the toolchain matrix and publishes coverage; `dependencies.yml` runs `cargo audit`.

---

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short description>

[optional body]
```

Types: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `build`, `style`

Scopes match top-level modules or subsystems: `mcp`, `session`, `supervisor`, `compression`, `config`, `acp`, `websocket`, `agent`, `learning`, `chat`, `commands`, `schedule`, `sandbox`, `deps`, `release`

Append `!` after the scope for breaking changes (`refactor(config)!: ...`). `CHANGELOG.md` is generated at release time from these messages — the subject line is what users read.

Examples:
```
feat(mcp): add file_watch tool to core server
fix(session): prevent race condition in spinner shutdown
feat(usage)!: support pricing v2 allowances
docs: add CONTRIBUTING.md
refactor(config): consolidate merge logic into load()
```

---

## Validation Checklist

Before opening a pull request:

- [ ] Apache 2.0 copyright header on every new `.rs` file
- [ ] Unit test bodies in sibling `*_tests.rs` files, not production files
- [ ] No `std::println!` / `std::eprintln!` — use crate macros
- [ ] No `unwrap_or_else(|_| ...)` that swallows real errors
- [ ] MCP tool failures return `Ok(McpToolResult::error(...))`, not `Err(...)`
- [ ] New config fields added to `config-templates/default.toml` first, then the Rust type
- [ ] Session-scoped state added inside `init_session_services`, every entry point still calls it
- [ ] `mcp-*.toml` override behavior considered if adding config loading logic
- [ ] `auto_bind` strings are exact-match — full role tag in both places
- [ ] No hardcoded config values
- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo test` passes
- [ ] Every changed line traces directly to the request — no opportunistic cleanups

---

## Never

| Don't | Because |
|-------|---------|
| Return `Err()` from MCP tool execution | Callers expect `Ok(McpToolResult)` — always |
| Use `std::println!` / `std::eprintln!` | Breaks the terminal spinner |
| Use `unwrap_or_else(\|_\| default)` | Hides real failures silently |
| Put test bodies inline in production files | Sibling `*_tests.rs` keeps product code readable and coverage honest |
| Add session state outside `init_session_services` | Causes subtle mode-specific bugs across CLI / ACP / WebSocket |
| Use `"stdin"` as MCP server type | Correct value is `"stdio"` |
| Use `[role_name]` TOML sections for roles | Always `[[roles]]` with `name = "..."` |
| Omit the copyright header | License compliance requirement |
| Hardcode config values | All defaults belong in `config-templates/default.toml` |
| Add unrequested features | YAGNI — scope creep slows review and introduces regressions |
| Comment out dead code | Delete it; git history preserves it |
