# ACP Protocol

The Agent Client Protocol (ACP) enables Octomind to run as a sub-agent over stdio, communicating via JSON-RPC. This is used for editor integration and agent-to-agent delegation.

## Overview

ACP provides:
- JSON-RPC over stdio communication
- Tool execution with streaming results
- Slash command support
- MCP server injection from the host
- Session lifecycle management

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

The agent reads JSON-RPC messages from stdin and writes responses to stdout. Stderr is reserved for logging (written to `~/.local/share/octomind/logs/acp-debug.log`).

## Protocol Flow

1. **Host starts** `octomind acp <role>` as a subprocess
2. **Initialization handshake**: host sends capabilities, agent responds with available features
3. **Authentication**: host authenticates (no-op by default)
4. **Session creation**: host calls `new_session` or `load_session` (resume by ID)
5. **Message exchange**: host sends user messages, agent streams responses
6. **Tool execution**: agent announces tool calls, streams results
7. **Cancellation**: host can cancel in-progress prompts via `cancel`
8. **Shutdown**: host closes stdin or sends shutdown message

## Agent Capabilities

The agent advertises these capabilities during initialization:

- **Session management**: `new_session`, `load_session` (resume by session ID)
- **Prompt**: image support, embedded context
- **MCP**: HTTP transport support (SSE is not supported)
- **Cancellation**: cancel in-progress prompts
- **Commands**: extension commands via `octomind/command` namespace

## Use Cases

### Editor Integration

Editors (Neovim, Zed, JetBrains) use ACP to embed Octomind as an AI assistant. See [Editor Integration](../usage/12-editor-integration.md).

### Agent Delegation

Configured agents (`[[agents]]`) spawn ACP subprocesses to handle tasks:

```toml
[[agents]]
name = "context_gatherer"
description = "Gather codebase context"
command = "octomind acp context_gatherer"
workdir = "."
```

When the AI calls `agent_context_gatherer(task="...")`, Octomind:
1. Spawns `octomind acp context_gatherer` as a subprocess
2. Sends the task via JSON-RPC
3. Collects the agent's response (all `agent_message_chunk` text)
4. Returns the result as a tool output

### Custom ACP Servers

The `command` field in `[[agents]]` can point to any ACP-compatible binary, not just Octomind. This enables integration with custom tools and services.

## Background Inbox Monitor

ACP sessions automatically spawn a background task that monitors the session's inbox for incoming messages from schedules, webhooks, injections, and background agents. When a message arrives:

1. The monitor acquires the session
2. Processes the message through the full AI pipeline (tool calls, streaming, etc.)
3. Streams the response back to the ACP client
4. Returns the session to the pool

This uses `tokio::sync::Notify` for efficient event-driven wake-ups — no polling. The monitor exits when the session is destroyed.

## Session ID in MCP Capabilities

MCP servers receive a `session_id` field during the initialize handshake. This is sent under `capabilities.experimental.session`:

```json
{
  "capabilities": {
    "experimental": {
      "session": {
        "role": "developer",
        "spec": "...",
        "project": "my-project",
        "session_id": "abc123..."
      }
    }
  }
}
```

This allows MCP servers to identify and track specific sessions, enabling session-scoped state and per-session behavior.

## Error Handling

- Protocol errors are logged to `~/.local/share/octomind/logs/acp-errors.jsonl`
- Structured JSONL format for programmatic error analysis
- Stdout/stderr are separated to prevent protocol corruption

## MCP Server Injection

Hosts can inject additional MCP servers during the ACP initialization handshake. The injected servers become available to the agent's session alongside its configured servers.

This enables editors to provide project-specific tools (e.g., language servers, project databases) to the AI session.
