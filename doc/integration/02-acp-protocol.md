# ACP Protocol

The Agent Client Protocol (ACP) enables Octomind to run as a sub-agent over stdio, communicating via JSON-RPC. This is used for editor integration and agent-to-agent delegation.

## Overview

ACP provides:
- JSON-RPC over stdio communication
- Tool execution with streaming results
- Slash command support (advertised set) plus a programmatic [extension-command](#extension-commands) method
- MCP server injection from the host (Stdio/HTTP)
- Session lifecycle management
- Out-of-band [cost/token usage](#cost--token-usage-side-channel) reporting via `_meta`

## Starting an ACP Agent

```bash
octomind acp [TAG] [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `TAG` | Agent tag (e.g. `developer:general`) or role name. Omit for config default. |
| `--name`, `-n` | Preferred session name for the next `new_session` |
| `--resume`, `-r` | Resume a specific session by name on `new_session` |
| `--resume-recent` | Resume the most recent session for the CWD on `new_session` |
| `--model`, `-m` | Override the model for all sessions |
| `--sandbox` | Restrict filesystem writes to CWD |
| `--hook` | Activate a webhook hook by name (repeatable) |

The agent reads JSON-RPC messages from stdin and writes responses to stdout. Stdout and stderr are reserved for the protocol, so all diagnostics go to files in `~/.local/share/octomind/logs/`:

| File | Contents |
|------|----------|
| `acp-debug.log` | Tracing output for the ACP session (controlled by `RUST_LOG` / config `log_level`) |
| `acp-errors.jsonl` | Structured JSONL error sink for programmatic protocol-error analysis |
| `acp-init-errors.log` | Fallback for failures that happen *before* logging is up (tracing / error-sink initialization). Written directly with no formatting. |

> ACP output reuses the same internal `ServerMessage` pipeline as the WebSocket server — `ToolCall`/`ToolCallUpdate` translation mirrors the WebSocket message types. See [WebSocket Server](01-websocket-server.md) for the shared message shapes.

## Protocol Flow

Each step is labelled with who acts — **(host)** = the editor/parent client, **(agent)** = Octomind.

1. **(host)** Starts `octomind acp [TAG]` as a subprocess.
2. **(host → agent)** `initialize`: host sends its capabilities; agent responds with `ProtocolVersion::LATEST`, its capabilities, agent identity, and an `octomind.dev` extension marker (see [Agent Capabilities](#agent-capabilities)).
3. **(host → agent)** `authenticate`: a no-op — the agent always returns success and never requires credentials.
4. **(host → agent)** Session creation: `session/new` starts a fresh session; `session/load` resumes a specific session id from disk. See [Session Creation](#session-creation-new_session-vs-load_session) for the behavioral difference.
5. **(host ↔ agent)** Message exchange: host sends `session/prompt`; agent streams responses as `session/update` notifications.
6. **(agent → host)** Tool execution: agent announces tool calls as `ToolCall` updates and streams `ToolCallUpdate` results.
7. **(host → agent)** Cancellation: host sends `session/cancel`; the in-flight prompt returns `StopReason::Cancelled`.
8. **(host)** Shutdown: the agent runs until stdin closes (client disconnect), at which point the JSON-RPC I/O loop ends and `run()` returns. There is **no** dedicated shutdown RPC.

### Example message exchange

A minimal session looks like this on the wire (one JSON object per line, newline-delimited):

```jsonc
// host → agent
{"jsonrpc":"2.0","id":1,"method":"initialize",
 "params":{"protocolVersion":1,"clientCapabilities":{}}}

// agent → host (capabilities + identity, see below)
{"jsonrpc":"2.0","id":1,"result":{
  "protocolVersion":1,
  "agentCapabilities":{"loadSession":true,
    "mcpCapabilities":{"http":true},
    "promptCapabilities":{"image":true,"embeddedContext":true},
    "_meta":{"octomind.dev":{"commands":true}}},
  "agentInfo":{"name":"octomind","version":"0.29.0"}}}

// host → agent
{"jsonrpc":"2.0","id":2,"method":"session/new",
 "params":{"cwd":"/path/to/project","mcpServers":[]}}

// agent → host
{"jsonrpc":"2.0","id":2,"result":{"sessionId":"<session-id>"}}

// host → agent
{"jsonrpc":"2.0","id":3,"method":"session/prompt",
 "params":{"sessionId":"<session-id>",
   "prompt":[{"type":"text","text":"Explain main.rs"}]}}

// agent → host (streamed; many notifications) then the final result
{"jsonrpc":"2.0","method":"session/update",
 "params":{"sessionId":"<session-id>",
   "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"..."}}}}
{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}
```

### Cost / token usage side-channel

The standard ACP `UsageUpdate` variant is not used. Instead, token and cost usage is delivered out-of-band as a `SessionInfoUpdate` notification carrying an `octomind.usage` object in the notification `_meta`:

```jsonc
{"jsonrpc":"2.0","method":"session/update",
 "params":{"sessionId":"<session-id>",
   "update":{"sessionUpdate":"session_info_update"},
   "_meta":{"octomind.usage":{
     "session_tokens": 12000,
     "session_cost": 0.0123,
     "input_tokens": 9000,
     "output_tokens": 3000,
     "cache_read_tokens": 0,
     "cache_write_tokens": 0,
     "reasoning_tokens": 0
   }}}}
```

`_meta` is the spec-blessed extensibility channel, so this works on any ACP 0.10.x client that forwards `_meta` through unchanged.

## Agent Capabilities

The `initialize` response advertises:

- **Protocol version**: `ProtocolVersion::LATEST` (the newest version the bundled `agent_client_protocol` crate supports).
- **Agent identity**: `agentInfo` = `{ name: "octomind", version: <crate version> }`.
- **Session management**: `loadSession: true` — both `session/new` and `session/load` (resume by session id) are supported.
- **Prompt**: `image: true` (inline base64 images) and `embeddedContext: true` (embedded resources, used to carry video — see [Prompt Content](#prompt-content)).
- **MCP**: `http: true` — HTTP transport is advertised so clients offer HTTP MCP servers. SSE is **not** supported and such servers are skipped silently.
- **Cancellation**: in-progress prompts can be cancelled.
- **Extension commands**: `_meta["octomind.dev"] = { commands: true }` signals support for the `octomind/command` extension method (see [Extension Commands](#extension-commands)).

### Session creation: `new_session` vs `load_session`

Both calls run the session in `websocket` output mode, merge any client-injected MCP servers (see [MCP Server Injection](#mcp-server-injection)), and spawn a [background inbox monitor](#background-inbox-monitor). They differ in how they pick the session:

- **`session/new`** creates a fresh session and, on the **first** call, consumes the one-shot CLI overrides `--name` / `--resume` / `--resume-recent`. After that first call those overrides revert to defaults; subsequent `session/new` calls ignore them.
- **`session/load`** always resumes the specific session id supplied by the client, read from disk. It does not touch the one-shot overrides.

The `--model` and `--hook` flags, by contrast, apply to **every** session created or loaded for the agent's lifetime.

### Advertised slash commands

After a session is created the agent sends an `AvailableCommandsUpdate` listing the slash commands the client may offer. Names are sent **without** the leading `/` (the client prepends it for display). This is the ACP-advertised set and is distinct from the full interactive CLI command set:

| Command | Input hint | Description |
|---------|-----------|-------------|
| `help` | — | Show available commands |
| `role` | `<role_name>` | View or change current role |
| `model` | `<provider:model>` | View or change current AI model |
| `done` | — | Force-compress the conversation context; if learning is enabled, extract lessons in the background (no memory write, no auto-commit) |
| `info` | — | Display token and cost breakdown for this session |
| `clear` | — | Clear the screen |
| `copy` | — | Copy last response to clipboard |
| `context` | `[all\|assistant\|user\|tool\|large]` | Display session context |
| `list` | `[page]` | List all available sessions |
| `session` | `[session_name]` | Switch to or create a session |
| `run` | `<command_name>` | Execute a command layer |
| `workflow` | `<workflow_name> [input]` | **Legacy / no-op** — still advertised over ACP, but `/workflow` was removed; run workflows via the `octomind workflow <file.toml>` CLI instead |
| `mcp` | `[info\|list\|full\|health\|dump\|validate]` | MCP server management |
| `plan` | — | Display current plan stored in MCP plan tool |
| `prompt` | `[template_name]` | Manage prompt templates |
| `image` | `<path>` | Attach image to next message |
| `video` | `<path>` | Attach video to next message |
| `loglevel` | `[none\|info\|debug]` | Set logging level |
| `report` | — | Generate detailed usage report for this session |
| `skill` | `[name\|pattern\|page]` | List, filter, or toggle skills |
| `effort` | `[low\|medium\|high\|xhigh\|max]` | View or change reasoning effort level |
| `schedule` | `[list\|add\|remove\|edit] [<id>] [when=...] [message=...] [every=...]` | Schedule a message to be injected at a future time |
| `exit` | — | Exit the session |

Slash commands are sent as ordinary `session/prompt` text per the ACP spec. The agent intercepts any prompt beginning with `/` *before* the AI pipeline, runs it via the session command handler, and streams the result back as an `agent_message_chunk`. `/done` (optionally with trailing instructions, e.g. `/done now write tests`) is intercepted first: it compresses the conversation, reports a status chunk, and — if trailing instructions are present — falls through to process them as a normal prompt.

### Prompt Content

`session/prompt` content blocks are mapped as follows:

- **Text** blocks are joined with newlines into the prompt.
- **Image** blocks are attached as inline base64 image attachments (using the block's `mimeType`).
- **Resource** blocks carrying a blob resource with a `video/*` MIME type are attached as video (ACP has no native video block). Audio and resource-link blocks are ignored.

If the prompt has no text, image, or video content, the agent immediately returns `StopReason::EndTurn`.

## Use Cases

### Editor Integration

Editors (Neovim, Zed, JetBrains) use ACP to embed Octomind as an AI assistant. See [Editor Integration](../usage/12-editor-integration.md).

> Compatibility note: usage and extension data are delivered through ACP `_meta`, so any ACP 0.10.x client that forwards `_meta` through unchanged will see them; clients that strip `_meta` still work but won't surface cost/usage.

### Agent Delegation

Configured agents (`[[agents]]`) spawn ACP subprocesses to handle tasks:

```toml
[[agents]]
name = "context_gatherer"
description = "Gather codebase context"
command = "octomind acp context_gatherer"
workdir = "."
```

When the AI calls `agent_context_gatherer(task="...")`, Octomind acts as the **ACP client**:
1. Spawns `octomind acp context_gatherer` as a subprocess.
2. Sends `initialize` (with `protocolVersion: "0.1.0"`) — it does **not** call `authenticate`.
3. Sends `session/new` with an empty `mcpServers` list.
4. Sends `session/prompt` carrying the task as a single text block.
5. Accumulates every `agent_message_chunk` text into the result, and forwards intermediate `session/update` events (thinking, tool calls, tool results) up to the parent's notification sink so the user sees the sub-agent's progress live.
6. Returns the accumulated text as the tool output (surfacing any `session/prompt` error instead of an empty string).

### Custom ACP Servers

The `command` field in `[[agents]]` can point to any ACP-compatible binary, not just Octomind. This enables integration with custom tools and services.

## Background Inbox Monitor

ACP sessions automatically spawn a background task that monitors the session's schedules and inbox for incoming messages from schedules, webhooks, injections, and background agents. When a message arrives:

1. The monitor acquires the session (via a per-session exclusion lock, so it never races with a concurrent user prompt).
2. Surfaces the injected message to the client as a `UserMessageChunk` prefixed with its source label, e.g. `[Scheduled] run the test suite`.
3. Processes the message through the full AI pipeline (tool calls, streaming, etc.).
4. Streams the response back to the ACP client.
5. Returns the session to the pool.

The same source-label surfacing happens in the `prompt()` path: before processing a user's prompt, the agent drains any inbox messages that arrived earlier and streams each as a `[<source>] ...` user chunk.

The monitor is event-driven, not polling: each loop it flushes due/idle schedule entries into the inbox, then waits on a `tokio::select!` over either the next schedule timer (`next_schedule_sleep`) or an inbox notification. It exits when the session is destroyed.

## Session context passed to downstream MCP servers

This is **not** part of the ACP handshake with the host. It is the payload Octomind sends *downstream* to the stdio MCP servers it spawns: when Octomind initializes a stdio MCP server, the MCP `initialize` request (MCP `protocolVersion` `2025-03-26`) carries the current session context under `params.capabilities.experimental.session`:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "clientInfo": { "name": "octomind", "version": "0.29.0" },
    "protocolVersion": "2025-03-26",
    "capabilities": {
      "experimental": {
        "session": {
          "role": "developer",
          "spec": "...",
          "project": "my-project",
          "session_id": "abc123...",
          "workdir": "/path/to/project"
        }
      }
    }
  }
}
```

This lets MCP servers identify and track specific sessions, enabling session-scoped state and per-session behavior. The full object is `role` (the role domain), `spec`, `project`, `session_id`, and `workdir`.

## Extension Commands

Beyond the slash commands sent as prompts, a host can invoke session commands programmatically through the `octomind/command` ACP extension method.

> The ACP library strips the leading `_` from a method name before routing, so the agent matches the method name without the underscore prefix.

**Request** (`CommandRequest`):

| Field | Type | Notes |
|-------|------|-------|
| `session_id` | string | Session to run the command in |
| `command` | string | Command to execute, e.g. `/info` |
| `args` | string[] | Optional arguments (defaults to `[]`) |

**Response** (`CommandResponse`):

| Field | Type | Notes |
|-------|------|-------|
| `success` | bool | Whether the command executed successfully |
| `output` | JSON \| null | Structured command output, when any |
| `error` | string \| null | Error message when `success` is false |

The command result maps to the response as:

| Command result | Response |
|----------------|----------|
| Handled | `success: true`, no `output` |
| Handled with output | `success: true`, `output` = the command's JSON |
| Exit | `success: true`, `output` = `{ "action": "exit" }` |
| Treated as user input (unknown command) | `success: false`, `error` = `"Unknown command: <command>"` |

## Error Handling

- ACP diagnostics are split across three log files in `~/.local/share/octomind/logs/` — see [Starting an ACP Agent](#starting-an-acp-agent) for the full table (`acp-debug.log`, `acp-errors.jsonl`, `acp-init-errors.log`).
- Protocol errors land in `acp-errors.jsonl` as structured JSONL for programmatic analysis.
- Stdout and stderr are reserved for the JSON-RPC protocol, so they are never used for logging — this prevents protocol corruption.

## Cancellation

A `session/cancel` notification triggers `SessionCancellation::shutdown()` for the targeted session, signalling the in-flight operation to stop. The corresponding `session/prompt` then returns `StopReason::Cancelled` rather than `StopReason::EndTurn`. Because the agent runs single-threaded inside a `LocalSet`, cancellation only takes effect at the prompt's next await point.

## MCP Server Injection

Hosts can inject additional MCP servers when creating a session (`session/new` or `session/load`). The injected servers become available to that session alongside its configured servers, letting editors provide project-specific tools (e.g. language servers, project databases) to the AI.

Injection semantics:

- **Transports**: `Stdio` and `HTTP` servers are accepted (timeout hard-coded to 30s, no tool filter — all tools exposed). `SSE` and unknown transports are skipped silently with a log line.
- **Session-scoped**: injection is applied to a per-session config snapshot; the base config is never mutated.
- **Deduped by name**: a server whose name is already present is not re-added.
