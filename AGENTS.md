# Octomind — AGENTS.md

Open-source AI coding agent and agent runtime: one Rust binary, any model (multi-provider via `octolib`), MCP-native. Sessions run interactively (CLI), non-interactively (`--format`), or as daemons (ACP stdio / WebSocket). TOML config is the single source of truth — model, tools, roles, compression, supervisor and learning all derive from it. Rust 1.95+ (MSRV enforced), tokio async, `clap` CLI.

## Commands
- Setup: `cargo build` · Hooks: `pip install pre-commit && pre-commit install` (fmt + clippy + check run on every commit)
- Dev: `cargo run` · Build: `cargo build` / `cargo build --release` (`make build`)
- One-shot gate: `make dev` (fmt + clippy + test)
- Fmt: `cargo fmt --all` · Check-only: `make fmt-check`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Test: `cargo test` (debug build — what CI runs; `make test` = `cargo test --release`, slower first run)
- Coverage: `make coverage` — `cargo llvm-cov` with `*_tests.rs` excluded so percentages describe product code
- Cross-compile/dist: `make build-all` / `make dist` (uses `cross`, see `Cross.toml`) · Release packaging: `build.sh`
- Bench (token-efficiency vs committed baseline): protocol in `bench/README.md`
- Build/test environment prereqs are per-OS — see **Platforms** below

## Where to look
| Task | Start here |
|------|------------|
| Add an MCP tool | schema in that server's `get_all_functions()` — `src/mcp/core/functions.rs`, `src/mcp/orchestration/mod.rs`, `src/mcp/runtime/mod.rs`, `src/mcp/agent/functions.rs` → implement in same module → match arm in `src/mcp/mod.rs` `route_builtin_tool()` → register in `src/mcp/tool_map.rs` |
| Add a session command (`/foo`) | `src/session/chat/session/commands/<name>.rs` returning `CommandResult` → `mod` + routing arm in `commands/mod.rs` `process_command()` → new `CommandOutput` variant if the result shape is new → constant + entry in the fixed-size `COMMANDS` array in `src/session/chat/commands.rs` (bump the length) |
| Change a config field/default | `config-templates/default.toml` FIRST, then the matching type in `src/config/` |
| Config load / merge / role resolution | `src/config/loading.rs` (`load()`), `src/config/merge.rs` (`get_merged_config_for_role()`) |
| Tool not found / routing bugs | `src/mcp/tool_map.rs` `get_server_for_tool()` → `src/mcp/mod.rs` `try_execute_tool_call()` |
| Session-scoped state | `src/session/context.rs` `init_session_services()` — task-local `SessionId` via `with_session_id` |
| Session main loop | `src/session/chat/session/main_loop.rs` (`init_session_runtime()`) |
| Response / tool execution | `src/session/chat/response.rs`, `response/tool_execution.rs`, `response/tool_result_processor.rs` |
| Skill auto-activation / validators | `src/mcp/runtime/skill_auto.rs` (`run_activation`, `run_validators`) |
| Compression | `src/session/chat/conversation_compression/` (`decision.rs`, `apply.rs`, `schema.rs`, `attention/`) |
| Supervisor (gate, plan, detect, condense, resolve, authorizer, stats) | `src/supervisor/` |
| Learning (extract / inject / retention / file backend / evolution) | `src/supervisor/learning/` |
| Workflows | `src/workflow/` (`schema.rs`, `run.rs`, `validate.rs`) + templates `config-templates/workflow*.toml` |
| Taps (shareable project agents) | `src/agent/` (registry, taps, resolver, deps) |
| Layers / guardrails / pipes | `src/session/layers/`, `src/session/guardrails.rs`, `src/session/pipe.rs` |
| ACP / WebSocket servers | `src/acp/agent.rs`, `src/websocket/server.rs` |
| Sandbox / embeddings / telemetry | `src/sandbox/`, `src/embeddings/`, `src/telemetry.rs` |
| CLI subcommands | `src/commands/` (run, tap, untap, workflow, config, send, server, acp, …) |
| Path / directory constants | `src/directories.rs` |

## Architecture: flows that matter

**Config → role → tools.** `Config::load()` merges every `*.toml` in the config dir alphabetically (arrays concat + dedup by `name`; tables deep-merge), then `mcp-*.toml` files AFTER base files — they always win for same-named servers (intended override path; `mcp persist` writes `<config_dir>/mcp-<name>.toml` with `auto_bind`). `get_merged_config_for_role(role)` collects explicit `server_refs` UNION exact-match `auto_bind` hits → `initialize_mcp_for_role()` spawns stdio/http servers, registers builtins, builds `TOOL_MAP` (tool name → server) → `try_execute_tool_call()` dispatches.

**Session lifecycle (critical invariant).** Four entry points share one init contract; session-scoped state goes inside `init_session_services()` and ALL four must keep calling it (`rg init_session_services`): CLI + non-interactive (`main_loop.rs` `init_session_runtime()`), ACP `new_session` and ACP `initialize` (`src/acp/agent.rs`), WebSocket (`src/websocket/server.rs`) — always inside `with_session_id`. Never call `init_inbox_for_session` / `init_job_manager` / similar directly.

**Processing pipeline.** User input → `/command` (`CommandResult` or `TreatAsUserInput`) → `run_activation` hook (main_loop only; skill auto-activation) → guardrails/pipe → workflows → layers → tool execution loop → `run_validators` hook (response.rs only) → spending check → output. `/done` is intercepted by the CLI main loop and the ACP prompt path, but the ACP `octomind/command` ext-method and WebSocket messages reach `process_command()` directly — its `DONE_COMMAND` arm is live code, not a leftover; keep it working.

## Conventions
- Apache 2.0 header (verbatim, below) on every new `.rs` file
- Tabs not spaces (`rustfmt.toml`), LF, 120-col limit; `cargo fmt --all` before every commit
- Unit test bodies in sibling `<name>_tests.rs` files; the production file only declares `#[cfg(test)] #[path = "<name>_tests.rs"] mod tests;` — never inline `mod tests {}` bodies
- Logging via `crate::log_debug!` / `log_info!` / `log_error!` / `log_conditional!` (defined in `src/config/mod.rs`): positional args only (`log_debug!("x={}", x)` — clippy cannot see through inline captures), no trailing comma. Log decisions and state transitions, not step-by-step tracing
- Printing via the crate macros (`println!` etc. shadow std in `src/lib.rs` and suspend the spinner) — never `std::println!`
- MCP tool failures are values: `Ok(McpToolResult::error(...))`, never `Err()`; validate params explicitly, wrap internal errors:
  ```rust
  match call.parameters.get("key") {
      Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
      _ => return Ok(McpToolResult::error(call.tool_name.clone(), call.tool_id.clone(), "key: non-empty string required".into())),
  }
  ```
- Fail fast: `.expect()` / `.context()?`; `anyhow` throughout, `thiserror` only where callers match variants
- No `std::sync::Mutex` across `.await` (deadlock) — `tokio::sync::Mutex` or an actor; session-keyed state only via the task-local `SessionId`, never process globals
- No wrapper methods, no speculative abstractions, no magic numbers; comments explain why; delete dead code instead of commenting it out
- All config defaults in `config-templates/default.toml` — never hardcode values
- Conventional Commits `type(scope): subject` (`!` = breaking); scopes = top-level modules (`mcp`, `session`, `supervisor`, `config`, `workflow`, `acp`, …); CHANGELOG is generated from subjects at release
- Misuse hints guide, never block: append a 💡 hint only when the better tool is actually enabled — gate on `crate::mcp::tool_map::get_server_for_tool("tool").is_some()` (see `src/mcp/hint_accumulator.rs`)
- Dynamic tools: `register_dynamic_agent_tool()` / `register_dynamic_server_tools()` in `tool_map.rs`; local project tools: executable scripts in `<workdir>/.agents/tools/` (auto-discovered by `src/mcp/core/local_tool.rs`)

### Copyright header (every `.rs` file)
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

## Done
- `cargo fmt --all` clean · `cargo clippy --all-targets --all-features -- -D warnings` exits 0 · `cargo check --all-targets --all-features` exits 0 · `cargo test` passes
- New `.rs` files carry the Apache header; test bodies in sibling `*_tests.rs`
- Session-scoped changes: `rg init_session_services` confirms all four entry points covered
- Config changes: `config-templates/default.toml` updated first
- Every changed line traces to the request — no opportunistic cleanups

## Platforms
One codebase for Linux, macOS and Windows — CI runs the full test suite on all three (stable on each OS + beta/nightly on Ubuntu; nightly is allowed to fail). Platform-sensitive facts:
- Everywhere: `protoc` required to build; `rg` + `ast-grep` on PATH for tests (tests spawn them as subprocesses)
- ONNX Runtime: Linux tests need the static lib via `ORT_LIB_LOCATION` (see the download step in `.github/workflows/ci.yml`); Windows/macOS auto-download prebuilts when it is unset
- Windows MSVC quirks: `RUSTFLAGS=-C target-feature=-crt-static` plus `CXXFLAGS_x86_64_pc_windows_msvc=-MD` — a mixed static/dynamic CRT causes LNK2038 / unresolved `__imp_*` link errors
- The first embedding test downloads the HF model `muvon/octomind-embed` (~130 MB) into the octolib cache: `~/.cache/octolib/huggingface` (Linux), `~/Library/Caches/octolib/huggingface` (macOS), `~/AppData/Local/octolib/huggingface` (Windows)
- OS-specific code lives in cfg-gated modules — follow the `src/sandbox/{linux,macos}.rs` pattern (`landlock` is Linux-only; `libc`/`proctitle` are Unix-only; sandbox backends exist for Linux and macOS only). Don't sprinkle one-off `#[cfg]` where a module split exists
- A test that cannot pass on every OS must be explicitly gated with the reason stated in code — silent per-OS divergence hides regressions
- musl/static release builds compile ORT from source in Alpine (see ci.yml `musl-build` + `Cross.toml`); a plain glibc `cargo build --release` binary is what runs in the Debian bench containers

## Boundaries (telemetry, accounting, learning)
Separate systems — wiring a metric into one does NOT make it appear in the others:
- `src/supervisor/stats.rs` — process-local accumulator for `/info`/debug snapshots; not persisted, not anonymous telemetry; counters aggregate across all concurrent daemon/ACP/WebSocket sessions (no per-session split)
- `SessionInfo` (`src/session/mod.rs`) — the persisted per-session record; add there (with `#[serde(default)]` + resume coverage) only when a metric must survive restart
- `src/telemetry.rs::Event` — the exact anonymous wire schema; an event is reported remotely only if it is defined there and populated by `record_session`. Audit each lifecycle path (CLI/piped/daemon exit, ACP disconnect, workflow, WebSocket — WS emits no session row today)
- Adaptive controllers may stay session-keyed and ephemeral (e.g. condenser state, cleared by `cleanup_session`) — don't persist just to observe
- Learning records: `memory_type = "learning"` = quote-first user rules (verbatim real-user quote + separate verifier); `"experience"` = learner-formed from valuable trajectories (REAL USER/TOOL citations, `verified|failed|unknown`, grounding verifier, one bounded repair, then fail closed). Storage: `learning/{project}/{role}/` — project-scoped, role-filtered
- File records are the sole supervisor-learning authority; external memory MCP tools are specialists, never learning stores. `related` = stable file IDs, `evidence` = `session://…/message/…`, retrieval expands links one hop
- Recall = one runtime-only Active Memory Pack per genuine user turn (token-bounded, materialized per provider request, dropped under headroom pressure); outcome credit only for pack IDs the specialist materially used
- Retention: two-watermark hot/cold lifecycle, per-type token budgets; similarity selects merge candidates but never authorizes a merge; merges need a grounding verifier and move sources to `.archive/` only after the replacement is stored; cold recall is lexical paging via `.archive/catalog.jsonl`; materially used cold records promote to hot
- Evolution (`[supervisor.learning.evolution]`): structured candidate → native-parser → verifier → shadow (control arm) → bounded trial (treatment arm, promoted only when it beats its shadow control beyond `noise_margin` at acceptable API-call cost) → active (pruned when the gain vanishes); learning text never becomes executable policy directly

## Gotchas
- `mcp-*.toml` loads AFTER all base `*.toml` regardless of sort order — the intended override mechanism
- `auto_bind` is exact-match: `"developer"` ≠ `"developer:general"` — use the full tag in both places
- A non-empty `allowed_tools` silently drops unlisted tools; `get_merged_config_for_role` auto-appends `"<server>:*"` for auto-bind servers — beware when constructing configs manually
- In TOML config files, scalar keys must come BEFORE nested table headers in a section — a scalar placed after `[x.y]` is parsed as a field of that table and silently ignored
- The compression decision/summary model is `[compression.model]`, separate from `model` (the legacy `[compression.decision]` spelling is migrated away)
- Log macros live in `src/config/mod.rs`; `src/lib.rs` only shadows the print macros
- Builtin servers (each its own arm in `route_builtin_tool()` + `tool_map`): `core` (incl. conditional `recall`), `orchestration` (`tap`, `schedule`, `monitor`), `runtime` (`mcp`, `agent`, `skill`, `capability`), `agent` (`agent_*`), `local` (`.agents/tools/` scripts)
- Dynamic tools are session-owned — a tool registered in one session is rejected when called from another (intentional isolation)

## Never
- Return `Err()` from MCP tool execution — always `Ok(McpToolResult::error(...))`
- Use `std::println!` / `std::eprintln!` — breaks the spinner and the output path
- Swallow errors with `unwrap_or_else(|_| default)`
- Add session-scoped state outside `init_session_services` or to only some of the four entry points
- Put inline `#[cfg(test)] mod tests {}` bodies in production files
- Hardcode config values — defaults belong in `config-templates/default.toml`
- Use `"stdin"` as the MCP server type — the value is `"stdio"`
- Use `[role]` tables — roles are `[[roles]]` with `name = "..."`
- Omit the Apache 2.0 header from a new `.rs` file
- Hold `std::sync::Mutex` across `.await`
- Add unrequested features or opportunistic cleanups

## References
- `CONTRIBUTING.md` — full dev guide (setup, Rust patterns, commit scopes, pre-commit detail)
- `doc/README.md` — doc index · `doc/dev/02-architecture.md`, `doc/dev/03-mcp-server-development.md` for depth
- `doc/reference/03-config-reference.md`, `doc/reference/04-environment-variables.md` — every field and env var
- `bench/README.md` — token-efficiency benchmark protocol and baseline advancing
