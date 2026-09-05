# Architecture

Use this contributor guide to trace a request from CLI configuration through sessions, MCP tools, and supervision. It
maps the runtime boundaries you need to preserve when changing code.

## Entry Points and Session Setup

`src/main.rs` parses the clap subcommand and loads `Config` before dispatching to `src/commands/`. Bare `octomind` uses
the default `run` arguments.

| Mode | Command handler | Session setup |
|------|-----------------|---------------|
| Interactive or piped CLI | `src/commands/run.rs` | `src/session/chat/session/main_loop.rs` |
| WebSocket | `src/commands/server.rs` | `src/websocket/server.rs` |
| ACP stdio | `src/commands/acp.rs` | `src/acp/agent.rs` |
| External workflow | `src/commands/workflow.rs` | child `octomind run --format jsonl` processes driven by `src/workflow/proc.rs` |

Every session entry point initializes session-keyed services through `session::context::init_session_services`, then
restores plan and schedule state for the selected session. The CLI `run` path additionally owns the `octomind send` IPC
listener and configured webhook listeners. ACP and WebSocket use their own transports and spawn inbox monitors for
asynchronous schedule and job events.

Start a session or select a transport with these entry points (run each separately):

```bash
octomind login
octomind run assistant:concierge --name architecture-notes
printf '%s\n' 'Explain the session initialization contract.' | octomind run --format jsonl
octomind acp assistant:concierge
octomind server --host 127.0.0.1 --port 8080
```

`run` accepts a role/tag positional argument, not a message argument. Piped prompts come from stdin; `--format plain`
and `--format jsonl` explicitly select non-interactive output. ACP expects a client speaking its stdio protocol; the
WebSocket command listens for clients rather than reading a terminal prompt. For workflow definitions, see
[Workflows](../usage/09-workflows.md).

## Configuration and Roles

`Config::load` in `src/config/loading.rs` chooses the config path, upgrades that file when needed, then merges all TOML
files in its directory. `config.toml` loads first, ordinary siblings alphabetically next, and `mcp-*.toml` files last.
Tables deep-merge; arrays of tables concatenate and keep the last whole entry for each `name`; scalar arrays replace
earlier arrays. Deserialization, role-map construction, and validation follow. `OCTOMIND_CONFIG_PATH` selects a file and
its sibling directory, not an isolated single-file load.

`Config::get_merged_config_for_role` in `src/config/merge.rs` selects servers referenced by the role or matched through
exact-string `auto_bind`. When a role uses a non-empty `allowed_tools` list, tools outside its patterns are filtered.
Interactive CLI roles receive a narrow overlay for `schedule` and `monitor`; piped, ACP, and WebSocket paths retain the
ordinary role merge.

For example, add this role to a sibling `roles.toml` in your config directory. The full role definition is needed when
replacing a same-name entry; `developer` and `developer:general` are different exact-match names.

```toml
[[roles]]
name = "architecture_reader"
system = "Explain the supplied architecture text. State what the text does not establish."
welcome = "Paste the source or question to inspect."

[roles.model]
max_tokens = 8192

[roles.mcp]
server_refs = []
allowed_tools = []
```

```bash
octomind config --validate
octomind run architecture_reader
```

### Model Purposes

There are exactly three model purposes, represented by `ModelPurpose` in `src/providers.rs`:

| Purpose | Configuration owner | Typical callers |
|---------|---------------------|-----------------|
| Main | `[model]`, with role profile and runtime name overrides | normal session and workflow-step requests |
| Supervisor | `[supervisor.model]` | gate, plan, learning, condense, and other supervisor work |
| Compression | `[compression.model]` | conversation compression |

The complete `[model]` profile is the inheritance baseline. Role, `[supervisor.model]`, and `[compression.model]` tables
are partial overrides. The shipped default for every purpose is `octohub:auto`, authenticated through `octomind login`
as shown above. `src/config/model.rs` resolves missing override fields against the main profile; the explicit supervisor
and compression defaults come from `config-templates/default.toml`.

## MCP Activation and Tool Routing

The default config declares four builtin servers:

| Server | Tool surface |
|--------|--------------|
| `core` | `recall` when attention or attention governance is enabled; plans are supervisor-internal |
| `orchestration` | `tap`, `schedule`, `monitor` |
| `runtime` | `mcp`, `agent`, `skill`, `capability` management |
| `agent` | generated `agent_<name>` execution tools |

`initialize_servers_for_role_with_callback` starts configured stdio and HTTP servers and reports progress.
`tool_map::initialize_tool_map` then maps each visible tool name to its server configuration. A call flows through
`execute_tool_call` and `try_execute_tool_call`; builtin calls reach `route_builtin_tool`, while external calls are
forwarded through `mcp::server::execute_tool_call`.

Dynamic MCP servers and dynamic agents update the tool map at runtime. Their registries are session-keyed when a session
context is active, and execution checks reject a dynamic tool owned by another session. Project-local executable tools
under `<workdir>/.agents/tools/` use the synthetic `local` server and are revalidated against the current workdir.

## Session Context and Asynchronous Input

`src/session/context.rs` keys the inbox, job manager, tap-run state, skills, schedules, dynamic agents, dynamic MCP
servers, and other runtime services by session ID. This keeps concurrent ACP and WebSocket sessions from sharing their
logical queues even though some underlying process registries and the global tool map are process-wide.

Asynchronous external input enters `src/session/inbox.rs` as an `InboxMessage` with a typed source such as schedule,
monitor, background agent/job, tap run, skill, inject, webhook, guardrail hook, or validator. Supervisor continuation
notes have separate paths; they are not all inbox messages. CLI daemon, ACP, and WebSocket loops drain the queue and run
the same AI response pipeline for each injected turn.

For a CLI daemon, leave the first command running and send from another terminal:

```bash
printf '%s\n' 'Wait for the next request.' | octomind run --name architecture-demo --daemon --format jsonl
```

```bash
octomind send --name architecture-demo 'Explain which session services are initialized.'
```

Unix IPC sockets live under `$XDG_RUNTIME_DIR/octomind` or the system temporary directory with a UID suffix; Windows
uses named pipes. These are host-local, even if persistent data is shared (`src/directories.rs`,
`src/commands/send.rs`).

### Output Surfaces

`src/session/output.rs` defines three sinks over the shared `websocket::ServerMessage` schema:

- `SilentSink` discards structured events because CLI rendering happens separately.
- `JsonlSink` serializes one server message per stdout line.
- `WebSocketSink` forwards server messages through a channel.

ACP translates the same internal events into ACP `session/update` notifications and extension metadata rather than
serializing WebSocket JSON on stdout.

## Persistence and Compression

Each session uses an append-only zstd-compressed JSONL log resolved by `src/session/logger.rs`.
`src/session/persistence.rs` replays summaries, messages, command records, compression/restoration points, and retained
knowledge. Plan and schedule modules separately restore their snapshots from the same log.

The compression orchestrator is `check_and_compress_conversation` in `src/session/chat/conversation_compression/`.
Automatic calls respect the adaptive fire line and cooldowns; `/done` uses the same engine with
`CompressionTrigger::Done`. A successful fold writes a `COMPRESSION_POINT`, stores the post-compression state, retains
bounded critical knowledge, and normally keeps a lossless archive for details omitted from the active summary. In
`conversation_compression/apply.rs`, optional PACT compression aborts if archive verification fails; forced compression
can continue without exact recall for that cycle after a storage failure.

In the interactive `architecture-notes` session above, finish the task and request a fold, then exit:

```text
/done
/exit
```

```bash
octomind run --resume architecture-notes
```

`/done` has both CLI/ACP prompt interception and a shared command handler for ACP extension and WebSocket command
messages. See [Compression](../usage/08-compression.md) for the user-facing behavior.

### Learning and observability

Learning lives in `src/supervisor/learning/`. Detached extraction writes file records, and recall constructs one bounded
Active Memory Pack per genuine user turn. `src/session/chat/session/api_executor.rs` materializes it for requests and
removes it afterward; exposure alone earns no outcome credit. Evolution is disabled by default. See [Learning
Benchmarks](05-learning-benchmark.md) for evaluation scope, and [Learning](../usage/13-learning.md) for storage and
inspection.

`src/supervisor/stats.rs` holds process-global, non-persisted counters shown in `/info`. These can mix concurrent
sessions. Persisted session accounting lives in `SessionInfo`; anonymous telemetry has its own explicit schema in
`src/telemetry.rs`. Do not infer one sink's contents from another.

```text
/info
/learning
```

## Troubleshooting and Error Boundaries

- Command and setup layers use `anyhow::Result` with contextual errors.
- MCP parameter and tool failures return `Ok(McpToolResult::error(...))` so the model receives a recoverable tool error.
- Central routing/cancellation may return a hard `Err`, which the response pipeline surfaces at the transport boundary.
- ACP reserves stdout and stderr for protocol traffic; tracing and structured protocol errors go to files under the
  Octomind logs directory.

When a tool is missing, inspect the role's `server_refs`, exact `auto_bind` tag, and `allowed_tools`, then the mapping
in `src/mcp/tool_map.rs`. Inside a session, start with:

```text
/mcp
/loglevel debug
```

When adding session state, find every initializer and cleanup path before editing:

```bash
rg -n 'init_session_services|cleanup_session' src/session src/acp src/websocket
```

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

Tests generally live in sibling `*_tests.rs` files and are attached with an explicit `#[path = "..."]` module
declaration.

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

## See also

- [Building from Source](01-building-from-source.md)
- [MCP Server Development](03-mcp-server-development.md)
- [Configuration Reference](../reference/03-config-reference.md)
- [ACP Integration](../integration/02-acp-protocol.md)
- [WebSocket Server](../integration/01-websocket-server.md)
