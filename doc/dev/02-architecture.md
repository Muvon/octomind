# Architecture

Contributor overview mapping Octomind's CLI, configuration, session runtime, MCP routing, supervision, and persistence to source modules.

## Source Layout

```text
src/
  main.rs                         CLI parsing and subcommand dispatch
  commands/                       run, login, server, acp, tap, send, workflow
  config/                         TOML loading, merge, validation, migrations, roles
  agent/                          tap registry, manifests, capabilities, dependencies
  acp/                            ACP stdio agent and extension commands
  websocket/                      WebSocket protocol and server
  workflow/                       external workflow schema, validation, execution
  mcp/
    mod.rs                        initialization and builtin/external tool routing
    tool_map.rs                   process-global tool-to-server map
    process.rs                    external process state and notification bridges
    server.rs                     stdio and HTTP MCP clients
    core/                         recall, plans, local project tools
    orchestration/                tap, schedule, monitor
    runtime/                      mcp, agent, skill, capability management
    agent/                        generated agent_<name> execution tools
    oauth/                        OAuth discovery, PKCE, callback, token storage
  session/
    context.rs                    session-keyed service registries
    persistence.rs                session replay and listing
    logger.rs                     compressed JSONL event log
    inbox.rs                      injected-message queue and source labels
    inject_listener.rs            octomind send IPC endpoint
    webhook_listener.rs           HTTP POST → script → inbox
    output.rs                     silent, JSONL, and WebSocket sinks
    chat/
      response.rs                 response and tool-call processing
      conversation_compression/   compression gate, summary, archive, knowledge
      session/                     setup, loops, commands, API preparation/execution
  supervisor/
    gate.rs                       completion gate
    plan.rs                       external plan controller
    condense.rs                   oversized tool-result condenser
    learning/                     extraction, retrieval, retention, evolution
  sandbox/                        Linux Landlock and macOS Seatbelt policies
  logging/                        CLI/ACP/WebSocket tracing and ACP error sink
  providers.rs                    octolib adapter and model-purpose tagging
  directories.rs                 data, config, session, log, and runtime paths
```

Tests generally live in sibling `*_tests.rs` files and are attached with an explicit `#[path = "..."]` module declaration.

## Entry Points and Session Setup

`src/main.rs` parses the clap subcommand and loads `Config` before dispatching to `src/commands/`. Bare `octomind` uses the default `run` arguments.

| Mode | Command handler | Session setup |
|------|-----------------|---------------|
| Interactive or piped CLI | `src/commands/run.rs` | `src/session/chat/session/main_loop.rs` |
| WebSocket | `src/commands/server.rs` | `src/websocket/server.rs` |
| ACP stdio | `src/commands/acp.rs` | `src/acp/agent.rs` |
| External workflow | `src/commands/workflow.rs` | child `octomind run --format jsonl` processes driven by `src/workflow/proc.rs` |

Every session entry point initializes session-keyed services through `session::context::init_session_services`, then restores plan and schedule state for the selected session. The CLI `run` path additionally owns the `octomind send` IPC listener and configured webhook listeners. ACP and WebSocket use their own transports and spawn inbox monitors for asynchronous schedule and job events.

## Configuration and Roles

`Config::load` in `src/config/loading.rs` reads the selected `config.toml`, then merges sibling TOML files. Ordinary files are processed alphabetically; `mcp-*.toml` extension files are applied last. Tables deep-merge, arrays concatenate, and same-name table entries are deduplicated with the later entry winning. The result is deserialized, migrated when needed, validated, and given a role lookup map.

`Config::get_merged_config_for_role` selects servers referenced by the role or matched through exact-string `auto_bind`. When a role uses a non-empty `allowed_tools` list, tools outside its patterns are filtered. Interactive CLI roles receive a narrow overlay for `schedule` and `monitor`; piped, ACP, and WebSocket paths retain the ordinary role merge.

## Model Purposes

There are exactly three model purposes, represented by `ModelPurpose` in `src/providers.rs`:

| Purpose | Configuration owner | Typical callers |
|---------|---------------------|-----------------|
| Main | `[model]`, with role/runtime name overrides | normal session and workflow-step requests |
| Supervisor | `[supervisor.model]` | gate, plan, learning, condense, and other supervisor work |
| Compression | `[compression.model]` | conversation compression |

The complete `[model]` profile is the inheritance baseline. Role, `[supervisor.model]`, and `[compression.model]` tables are partial overrides. The shipped default for every purpose is `octohub:auto`, authenticated through `octomind login`.

## MCP Activation and Tool Routing

The default config declares four builtin servers:

| Server | Tool surface |
|--------|--------------|
| `core` | conditional `recall`; plans are supervisor-internal |
| `orchestration` | `tap`, `schedule`, `monitor` |
| `runtime` | `mcp`, `agent`, `skill`, `capability` management |
| `agent` | generated `agent_<name>` execution tools |

`initialize_servers_for_role_with_callback` starts configured stdio and HTTP servers and reports progress. `tool_map::initialize_tool_map` then maps each visible tool name to its server configuration. A call flows through `execute_tool_call` and `try_execute_tool_call`; builtin calls reach `route_builtin_tool`, while external calls are forwarded through `mcp::server::execute_tool_call`.

Dynamic MCP servers and dynamic agents update the tool map at runtime. Their registries are session-keyed when a session context is active, and execution checks reject a dynamic tool owned by another session. Project-local executable tools under `<workdir>/.agents/tools/` use the synthetic `local` server and are revalidated against the current workdir.

## Session Context and Asynchronous Input

`src/session/context.rs` keys the inbox, job manager, tap-run state, skills, schedules, dynamic agents, dynamic MCP servers, and other runtime services by session ID. This keeps concurrent ACP and WebSocket sessions from sharing their logical queues even though some underlying process registries and the global tool map are process-wide.

All non-user input enters `src/session/inbox.rs` as an `InboxMessage` with a typed source such as schedule, background agent, tap run, skill, inject, webhook, guardrail hook, or validator. CLI daemon, ACP, and WebSocket loops drain the queue and run the same AI response pipeline for each injected turn.

## Output Surfaces

`src/session/output.rs` defines three sinks over the shared `websocket::ServerMessage` schema:

- `SilentSink` discards structured events because CLI rendering happens separately.
- `JsonlSink` serializes one server message per stdout line.
- `WebSocketSink` forwards server messages through a channel.

ACP translates the same internal events into ACP `session/update` notifications and extension metadata rather than serializing WebSocket JSON on stdout.

## Persistence and Compression

Each session uses an append-only zstd-compressed JSONL log resolved by `src/session/logger.rs`. `src/session/persistence.rs` replays summaries, messages, command records, compression/restoration points, retained knowledge, plan snapshots, and schedule snapshots to reconstruct the current state.

The compression orchestrator is `check_and_compress_conversation` in `src/session/chat/conversation_compression/`. Automatic calls respect the adaptive fire line and cooldowns; `/done` uses the same engine with `CompressionTrigger::Done`. A successful fold writes a `COMPRESSION_POINT`, stores the post-compression state, retains bounded critical knowledge, and keeps a lossless archive for details omitted from the active summary.

## Error Boundaries

- Command and setup layers use `anyhow::Result` with contextual errors.
- MCP parameter and tool failures return `Ok(McpToolResult::error(...))` so the model receives a recoverable tool error.
- Central routing/cancellation may return a hard `Err`, which the response pipeline surfaces at the transport boundary.
- ACP reserves stdout and stderr for protocol traffic; tracing and structured protocol errors go to files under the Octomind logs directory.

## Key Dependencies

- `octolib`: provider implementations, model metadata, and local embeddings
- `rmcp`: MCP clients and protocol types
- `agent-client-protocol`: ACP types and stdio connection loop
- `tokio`: asynchronous runtime, tasks, processes, networking, and channels
- `clap`: CLI parsing
- `serde`, `serde_json`, and `toml`: configuration and protocol serialization
- `hyper`: webhook HTTP server
- `tokio-tungstenite`: WebSocket transport
- `reedline`: interactive terminal input
