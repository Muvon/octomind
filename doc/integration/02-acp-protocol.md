# ACP Protocol

ACP lets Octomind run as a JSON-RPC sub-agent over stdio for editor integration and agent-to-agent delegation.

ACP provides:

- JSON-RPC over stdio communication
- Tool execution with streaming results
- Slash command support (advertised set) plus a programmatic [extension-command](#extension-commands) method
- MCP server injection from the host (Stdio/HTTP)
- Session lifecycle management
- Out-of-band [cost/token usage](#cost-and-token-usage-side-channel) reporting via `_meta`

## Starting an ACP Agent

```bash
octomind acp --name editor-session
```

| Flag | Description |
|------|-------------|
| `TAG` | Agent tag or role name. Omit for `default` (shipped value `assistant:concierge`). |
| `--name`, `-n` | Preferred session name for the next `new_session` |
| `--resume`, `-r` | Resume a specific session by name on `new_session` |
| `--resume-recent` | Resume the most recent session for the CWD on `new_session` |
| `--model`, `-m` | Override the model for all sessions |
| `--sandbox` | Restrict filesystem writes to CWD |
| `--hook` | Parsed and carried into ACP session options, but the ACP path does not currently start webhook listeners |

The agent reads JSON-RPC messages from stdin and writes responses to stdout. The initialized ACP runtime sends
diagnostics to files under the data directory:

| File | Contents |
|------|----------|
| `acp-debug.log` | Tracing output for the ACP session (controlled by `RUST_LOG` / config `log_level`) |
| `acp-errors.jsonl` | Structured JSONL error sink for programmatic protocol-error analysis |
| `acp-init-errors.log` | Fallback for failures that happen *before* logging is up (tracing / error-sink initialization). Written directly with no formatting. |

The data directory is `OCTOMIND_DATA_DIR`, or `~/.local/share/octomind` on macOS/Linux and `%LOCALAPPDATA%/octomind` on
Windows; the files above live in its `logs/` directory. Keep stdout exclusively for JSON-RPC. Capture stderr separately
because CLI/bootstrap failures can write there before ACP logging starts.

```bash
RUST_LOG=debug octomind acp --name editor-session 2>acp-startup.log
```

> ACP output reuses the same internal `ServerMessage` pipeline as the WebSocket server — `ToolCall`/`ToolCallUpdate`
> translation mirrors the WebSocket message types. See [WebSocket Server](01-websocket-server.md) for the shared message
> shapes.

For example, resume a named session and override its model:

```bash
octomind acp assistant --resume editor-session --model octohub:auto
```

Or resume the most recent session for the directory supplied by the host:

```bash
octomind acp --resume-recent --sandbox
```

## MCP Server Injection

Hosts can inject additional MCP servers when creating a session (`session/new` or `session/load`). The injected servers
become available to that session alongside its configured servers, letting editors provide project-specific tools (e.g.
language servers, project databases) to the AI.

Injection semantics:

- **Transports**: `Stdio` and `HTTP` servers are accepted (timeout hard-coded to 30s, empty per-server tool filter; role
  tool filters still apply). `SSE` and unknown transports are skipped silently with a log line.

- **Config snapshot**: injection does not mutate the base config. MCP processes/tool registration remain shared within
  the agent process; this is not a separate credential or process boundary.

- **Deduped by name**: a server whose name is already present is not re-added.
- **Ignored fields**: injected Stdio `env` and HTTP `headers` are not copied into Octomind's server config. Arrange
  credentials in the agent's environment or configured MCP server instead.

For an existing HTTP MCP server listening locally, the creation payload is:

```json
{"jsonrpc":"2.0","id":9,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[{"type":"http","name":"project-tools","url":"http://127.0.0.1:9000/mcp","headers":[]}]}}
```

## Protocol Flow

Each step is labelled with who acts — **(host)** = the editor/parent client, **(agent)** = Octomind.

1. **(host)** Starts `octomind acp [TAG]` as a subprocess.
2. **(host → agent)** `initialize`: host sends its capabilities; agent responds with `ProtocolVersion::LATEST`, its
  capabilities, agent identity, and an `octomind.dev` extension marker (see [Agent Capabilities](#agent-capabilities)).

3. **(host → agent)** `authenticate`: optional and a no-op. Provider credentials still come from the agent
  environment/config.

4. **(host → agent)** Session creation: `session/new` starts a fresh session; `session/load` resumes a specific session
  id from disk. See [session creation details](02-acp-protocol.md) for the behavioral difference.

5. **(host ↔ agent)** Message exchange: host sends `session/prompt`; agent streams responses as `session/update`
  notifications.

6. **(agent → host)** Tool execution: agent announces tool calls as `ToolCall` updates and streams `ToolCallUpdate`
  results.

7. **(host → agent)** Cancellation: host sends `session/cancel`; the in-flight prompt returns `StopReason::Cancelled`.
8. **(host)** Shutdown: close stdin after sending your final request. The agent waits for finite background work and
  resulting inbox turns before closing stdout and returning. Keep reading stdout until EOF. There is no dedicated
  shutdown RPC.

### Example message exchange

Use this Python client from your project directory. It starts the agent, waits for each response, prints updates, and
uses the returned session ID. The wire format is one JSON object per line; do not close stdin before the pending
responses arrive.

```python
import json
import os
import subprocess

agent = subprocess.Popen(
    ["octomind", "acp", "--name", "editor-session"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1,
)

def request(number, method, params):
    agent.stdin.write(json.dumps({
        "jsonrpc": "2.0", "id": number, "method": method, "params": params
    }) + "\n")
    agent.stdin.flush()
    for line in agent.stdout:
        message = json.loads(line)
        if message.get("id") == number:
            if "error" in message:
                raise RuntimeError(message["error"])
            return message["result"]
        print(json.dumps(message))
    raise RuntimeError("ACP closed before replying")

try:
    request(1, "initialize", {"protocolVersion": 1, "clientCapabilities": {}})
    session = request(2, "session/new", {"cwd": os.getcwd(), "mcpServers": []})
    session_id = session["sessionId"]
    print(request(3, "session/prompt", {
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": "Explain src/main.rs"}],
    }))
finally:
    agent.stdin.close()
    for line in agent.stdout:
        print(line, end="")
    agent.wait()
```

For subsequent payload examples, replace `editor-session` with the `sessionId` returned by your host's creation request.
To resume that saved session, send:

```json
{"jsonrpc":"2.0","id":4,"method":"session/load","params":{"sessionId":"editor-session","cwd":"/tmp","mcpServers":[]}}
```

Set `cwd` to the absolute project directory you want the session to use.

### Cost and token usage side-channel

The standard ACP `UsageUpdate` variant is not used. Instead, token and cost usage is delivered out-of-band as a
`SessionInfoUpdate` notification carrying an `octomind.usage` object in the notification `_meta`:

```json
{"jsonrpc":"2.0","method":"session/update",
 "params":{"sessionId":"editor-session",
   "update":{"sessionUpdate":"session_info_update"},
   "_meta":{"octomind.usage":{
     "session_tokens": 12000,
     "session_cost": 0.0,
     "input_tokens": 9000,
     "output_tokens": 3000,
     "cache_read_tokens": 0,
     "cache_write_tokens": 0,
     "reasoning_tokens": 0
   }}}}
```

After a normal AI prompt the agent also emits notification metadata:

```json
{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"editor-session","update":{"sessionUpdate":"session_info_update"},"_meta":{"octomind.verified":false,"octomind.pending_work":false}}}
```

`octomind.verified` is true only when supervisor verification tracking is enabled and the detector reports no
outstanding verification need. It is not proof that every task requirement passed. `octomind.pending_work` indicates
outstanding async work and also appears in the successful prompt result's `_meta`; `end_turn` alone does not mean
background work has finished.

Clients that preserve ACP `_meta` can read this extension; clients that discard `_meta` still receive the normal session
updates but not this usage payload.

Grounded behavior lifecycle changes are delivered as `SessionInfoUpdate` notifications with an `octomind.evolution`
`_meta` object, preserving the same action/id/name/kind/state/scope fields without rendering control-plane text as an
assistant message.

### Session creation: `new_session` vs `load_session`

Both calls run the session in `websocket` output mode, merge any client-injected MCP servers (see [MCP Server
Injection](#mcp-server-injection)), and spawn a [background inbox monitor](#background-inbox-monitor). They differ in
how they pick the session:

- **`session/new`** creates a fresh session and, on the **first** call, consumes the one-shot CLI overrides `--name` /
  `--resume` / `--resume-recent`. After that first call those overrides revert to defaults; subsequent `session/new`
  calls ignore them.

- **`session/load`** always resumes the specific session id supplied by the client, read from disk. It does not touch
  the one-shot overrides.

The `--model` value applies to **every** session created or loaded for the agent's lifetime. Although clap accepts
repeatable `--hook` values and ACP stores them in each session's arguments, ACP does not call the `run`-mode listener
bootstrap, so those values do not start webhook listeners.

### Prompt Content

`session/prompt` content blocks are mapped as follows:

- **Text** blocks are joined with newlines into the prompt.
- **Image** blocks are attached as inline base64 image attachments (using the block's `mimeType`).
- **Resource** blocks carrying a blob resource with a `video/*` MIME type are attached as video (ACP has no native video
  block). Audio and resource-link blocks are ignored.

To attach an actual local image or video, indent this code inside the Python client's `try` block, before `finally`
(choose a model supporting the modality):

```python
import base64
from pathlib import Path

image_data = base64.b64encode(Path("screenshot.png").read_bytes()).decode("ascii")
print(request(6, "session/prompt", {
    "sessionId": session_id,
    "prompt": [
        {"type": "text", "text": "Describe this screenshot"},
        {"type": "image", "mimeType": "image/png", "data": image_data},
    ],
}))
video_data = base64.b64encode(Path("clip.mp4").read_bytes()).decode("ascii")
print(request(7, "session/prompt", {
    "sessionId": session_id,
    "prompt": [
        {"type": "text", "text": "Summarize this clip"},
        {"type": "resource", "resource": {
            "uri": "file:///tmp/clip.mp4", "mimeType": "video/mp4", "blob": video_data,
        }},
    ],
}))
```

If the prompt has no text, image, or video content, the agent immediately returns `StopReason::EndTurn`.

## Use Cases

### Editor Integration

For editor launch configurations, see [Editor Integration](../usage/12-editor-integration.md).

> Compatibility note: usage and extension data are delivered through ACP `_meta`. Clients that forward `_meta` see them;
> clients that strip `_meta` still work but do not surface those fields.

### Agent Delegation

Configured agents (`[[agents]]`) spawn ACP subprocesses to handle tasks:

```toml
[[agents]]
name = "context_gatherer"
description = "Gather codebase context"
command = "octomind acp context_gatherer"
workdir = "."
```

For an `agent_context_gatherer` tool call with these arguments, Octomind acts as the **ACP client**:

```json
{"task":"Summarize src/main.rs and its command dispatch"}
```

1. Spawns `octomind acp context_gatherer` as a subprocess.
2. Sends `initialize` (with `protocolVersion: 1`) — it does **not** call `authenticate`.
3. Sends `session/new` with an empty `mcpServers` list.
4. Sends `session/prompt` carrying the task as a single text block.
5. Accumulates every `agent_message_chunk` text into the result, and forwards intermediate `session/update` events
  (thinking, tool calls, tool results) up to the parent's notification sink so the user sees the sub-agent's progress
  live.

6. Waits for outstanding async work when `octomind.pending_work` is true, then returns accumulated text (surfacing any
  `session/prompt` error instead of an empty string).

Initialization and session creation each have a 30-second handshake timeout. Custom ACP binaries must reply to both
requests before they can receive the task.

## Background Inbox Monitor

ACP sessions automatically spawn a background task that monitors schedules and the session inbox for internally enqueued
messages such as scheduled work, background agents, tap runs, skills, monitors, detached jobs, and guardrail feedback.
ACP does not start the `octomind send` or webhook listeners used by `octomind run`. When a message arrives:

1. The monitor acquires the session (via a per-session exclusion lock, so it never races with a concurrent user prompt).
2. Surfaces the injected message to the client as a `UserMessageChunk` prefixed with its source label, e.g. `[schedule
  abc12345] run the test suite`.

3. Processes the message through the full AI pipeline (tool calls, streaming, etc.).
4. Streams the response back to the ACP client.
5. Returns the session to the pool.

The prompt path also surfaces queued inbox work as source-labelled user chunks before the new user prompt.

The monitor is event-driven, not polling: each loop it flushes due/idle schedule entries into the inbox, then waits on a
`tokio::select!` over either the next schedule timer (`next_schedule_sleep`) or an inbox notification. It exits when the
session is destroyed.

## Extension Commands

Beyond the slash commands sent as prompts, a host can invoke session commands programmatically through the
`octomind/command` ACP extension method.

> The ACP library strips the leading `_` from a method name before routing, so the agent matches the method name without
> the underscore prefix.

On the wire, include the leading underscore and a leading slash in the command (unlike WebSocket commands):

```json
{"jsonrpc":"2.0","id":8,"method":"_octomind/command","params":{"session_id":"editor-session","command":"/mcp","args":["list"]}}
```

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

For example, a handled command can return:

```json
{"jsonrpc":"2.0","id":8,"result":{"success":true,"output":null,"error":null}}
```

The command result maps to the response as:

| Command result | Response |
|----------------|----------|
| Handled | `success: true`, `output: null`, `error: null` |
| Handled with output | `success: true`, `output` = the command's JSON |
| Exit | `success: true`, `output` = `{ "action": "exit" }` |
| Treated as user input (unknown command) | `success: false`, `error` = `"Unknown command: <command>"` |

`/done` through the extension method compresses only: trailing `args` do not become a follow-up prompt. Use `/done` in
`session/prompt` when you want trailing instructions processed. Session-context-dependent commands such as `/status`
should also use `session/prompt`: the extension dispatcher does not establish the task-local session context required by
those handlers.

```json
{"jsonrpc":"2.0","id":10,"method":"session/prompt","params":{"sessionId":"editor-session","prompt":[{"type":"text","text":"/status agents"}]}}
```

## Cancellation

A `session/cancel` notification triggers `SessionCancellation::shutdown()` for the targeted session, signalling the
in-flight operation to stop. A successful in-flight API call then returns `StopReason::Cancelled` rather than
`StopReason::EndTurn`. Because the agent runs single-threaded inside a `LocalSet`, cancellation only takes effect at the
prompt's next await point.

Send cancellation without a JSON-RPC request ID while a prompt is running:

```json
{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"editor-session"}}
```

Prompts and extension commands for the same session wait on the same exclusion lock. Cancellation signals the active
operation; API failures may still return a JSON-RPC error rather than a successful cancelled result.

## Common questions

| Symptom | What to check |
|---------|---------------|
| Command appears in the editor but fails | The advertised `session`, `workflow`, and `agents` entries have no handler. |
| Host waits forever after `/exit` | `/exit` reports intent; close the subprocess stdin to end ACP. |
| Hook receives no requests | ACP parses `--hook` but starts no HTTP listener; use the run daemon. |
| Extension method not found | Send `_octomind/command` on the wire and `/info` or another slash-prefixed command. |
| No usage, verification, or evolution data | Preserve `params._meta` on notifications and `result._meta` on results. |
| No startup response | Inspect the three log files above and captured bootstrap stderr. |

## Agent Capabilities

The `initialize` response advertises:

- **Protocol version**: `ProtocolVersion::LATEST` (the newest version the bundled `agent_client_protocol` crate
  supports).

- **Agent identity**: `agentInfo` = `{ name: "octomind", version: <crate version> }`.
- **Session management**: `loadSession: true` — both `session/new` and `session/load` (resume by session id) are
  supported.

- **Prompt**: `image: true` (inline base64 images) and `embeddedContext: true` (embedded resources, used to carry video
  — see [Prompt Content](#prompt-content)).

- **MCP**: `http: true` — HTTP transport is advertised so clients offer HTTP MCP servers. SSE is **not** supported and
  such servers are skipped silently.

- **Cancellation**: in-progress prompts can be cancelled.
- **Extension commands**: `_meta["octomind.dev"] = { commands: true }` signals support for the `octomind/command`
  extension method (see [Extension Commands](#extension-commands)).

## Advertised slash commands

After a session is created the agent sends an `AvailableCommandsUpdate` listing the slash commands the client may offer.
Names are sent **without** the leading `/` (the client prepends it for display). This is the ACP-advertised set and is
distinct from the full interactive CLI command set:

| Command | Input hint | Description |
|---------|-----------|-------------|
| `help` | — | Show available commands |
| `role` | `<role_name>` | View or change current role |
| `model` | `<provider:model>` | View or change current AI model |
| `done` | — | Force-compress the conversation context and, when learning is enabled, start lesson extraction in the background |
| `info` | — | Display token and cost breakdown for this session |
| `clear` | — | Clear the screen |
| `copy` | — | Copy last response to clipboard |
| `context` | `[all\|assistant\|user\|tool\|large]` | Display session context |
| `list` | `[page]` | List all available sessions |
| `session` | `[session_name]` | Advertised for client compatibility, but not implemented by the session command dispatcher |
| `run` | `<command_name>` | Execute a command layer |
| `workflow` | `<workflow_name> [input]` | Advertised for client compatibility, but not implemented; run workflows via `octomind workflow <name-or-file>` instead |
| `mcp` | `[info\|list\|full\|health\|dump\|validate]` | MCP server management |
| `plan` | — | Display the current supervisor-owned plan |
| `prompt` | `[template_name]` | Manage prompt templates |
| `image` | `<path>` | Attach image to next message |
| `video` | `<path>` | Attach video to next message |
| `loglevel` | `[none\|info\|debug]` | Set logging level |
| `report` | — | Generate detailed usage report for this session |
| `skill` | `[name\|pattern\|page]` | List, filter, or toggle skills |
| `effort` | `[low\|medium\|high]` | View or change reasoning effort level |
| `schedule` | `[list\|add\|remove\|edit] [<id>] [when=...] [message=...] [every=...]` | Schedule a message to be injected at a future time |
| `agents` | `[session]` | Advertised but unsupported by shared dispatch; use `/status` for async work |
| `usage` | — | Show spend and quotas for the signed-in Octomind account |
| `login` | — | Start the Octomind account sign-in flow |
| `exit` | — | Report exit requested; the host still owns subprocess shutdown |

Slash commands are sent as ordinary `session/prompt` text per the ACP spec. The agent intercepts any prompt beginning
with `/` *before* the AI pipeline, runs it via the session command handler, and streams the result back as an
`agent_message_chunk`. `/done` (optionally with trailing instructions, e.g. `/done now write tests`) is intercepted
first: it compresses the conversation, reports a status chunk, and — if trailing instructions are present — falls
through to process them as a normal prompt.

For example, request MCP information through the prompt channel:

```json
{"jsonrpc":"2.0","id":5,"method":"session/prompt","params":{"sessionId":"editor-session","prompt":[{"type":"text","text":"/mcp list"}]}}
```

Use the [Session Commands](../reference/02-session-commands.md) reference for argument examples. The advertised list is
not a dispatch guarantee: `session`, `workflow`, and `agents` are unsupported. `/done` compresses and may start
learning; despite its advertised description, this handler does not auto-commit. Remote `clear`/`copy` commands run on
the agent host and do not clear your editor or copy to its clipboard.

## Session context passed to downstream MCP servers

This is **not** part of the ACP handshake with the host. It is client information Octomind sends downstream to MCP
servers. Modern MCP requests carry client information per request; the legacy fallback carries it in `initialize`. In
both cases, `capabilities.experimental.session` has this shape:

```json
{
  "capabilities": {
    "experimental": {
      "session": {
        "role": "developer",
        "spec": "general",
        "project": "my-project",
        "session_id": "editor-session",
        "workdir": "/tmp",
        "git": false
      }
    }
  }
}
```

The full object is `role` (the role domain), `spec`, `project`, `session_id`, `workdir`, and `git` (whether the workdir
is inside a Git repository).

## See also

- [ACP implementation](../../src/acp/agent.rs), [extension dispatch](../../src/acp/commands.rs), and [CLI
  flags](../../src/commands/acp.rs).

- [Shared command handlers](../../src/session/chat/session/commands/mod.rs) and [ACP subprocess
  client](../../src/mcp/agent/functions.rs).

- [WebSocket Server](01-websocket-server.md) — JSON frame transport and shared event shapes.
- [Daemon Mode and Webhook Hooks](03-daemon-and-hooks.md) — external injection with `run`.
- [Editor Integration](../usage/12-editor-integration.md) — editor setup.
- [Session Commands](../reference/02-session-commands.md) — command arguments and examples.
