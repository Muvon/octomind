# WebSocket Server for Remote AI Agent Sessions

Use Octomind's WebSocket server to create remote AI sessions from web clients, bots, and automation tools. This guide
covers server setup, JSON frames, client examples, and troubleshooting.

## Quick Start

```bash
# Start server
octomind server --host 127.0.0.1 --port 8080

# Connect with websocat
websocat ws://127.0.0.1:8080
```

On connect, the server sends a welcome `status` frame. Create or resume your session, wait for its session status, then
send a prompt. In `websocat`, enter these JSON objects as separate lines:

```json
{"type":"session","request_id":"create-1","session_id":"my-session"}
```

After `Session created: my-session` or `Session resumed: my-session`:

```json
{"type":"message","request_id":"message-1","session_id":"my-session","content":"Explain src/main.rs"}
```

A `session` frame initializes session services and the background inbox monitor. Existing sessions can also be loaded
directly by `message` or `command`; these frames never create a missing session.

## Starting the Server

```bash
octomind server --host 127.0.0.1 --port 8080 --sandbox
```

| Flag | Default | Description |
|------|---------|-------------|
| `TAG` | `default` (`assistant:concierge`) | Agent tag (e.g. `developer:general`) or role name (e.g. `assistant`) |
| `--host` | `127.0.0.1` | Bind address |
| `--port`, `-p` | `8080` | Port. `0` picks a free port; the startup line `WebSocket server started on ws://<addr>` prints the bound address |
| `--sandbox` | config `sandbox` (`false`) | Restrict all filesystem writes to the current working directory |
| `--allow-origin` | none | Browser origin permitted to connect. Repeatable |

| Environment | Description |
|-------------|-------------|
| `OCTOMIND_SERVER_TOKEN` | Shared secret every handshake must present as `Authorization: Bearer <token>`; unset = no authentication. Environment only, so it never shows in `ps` |
| `OCTOMIND_MEDIA_ROOT` | Where `message` attachments are read from (default `/home/octo/.octomind/media`) |

## Authentication

An embedder that spawns the server for itself (a desktop app, an editor) should set `OCTOMIND_SERVER_TOKEN` to a
per-run random secret and send it on every connection:

```bash
OCTOMIND_SERVER_TOKEN="$(openssl rand -hex 16)" octomind server --port 0
```

```
GET / HTTP/1.1
Authorization: Bearer <OCTOMIND_SERVER_TOKEN>
```

A handshake without the right token is refused with `HTTP 401` before the welcome frame. The `Origin` check below runs
first, so a browser page is refused with `403` whatever it sends.

## Browser origins

The handshake explicitly checks browser origins because loopback binding alone does not authenticate browser clients. An
accepted connection can use the configured toolset and read session responses.

The server therefore refuses any handshake that carries an `Origin` header not listed in `--allow-origin`, with `HTTP
403` before the welcome frame:

```bash
octomind server --allow-origin http://localhost:3000 --allow-origin https://dashboard.example.com
```

Origins are matched exactly, as sent by the browser (scheme, host, and port; no trailing slash). Native clients —
`websocat`, the Python example below, and clients configured without an Origin header — send no `Origin` header and
connect without configuration.

## Single principal per process

The server has no notion of a user. One process serves one identity: config, MCP server processes, and OAuth tokens are
process-global and shared by every session, so all tool calls from every session go out with the same credentials.
`session_id` is a name, not a capability — any connection may resume any session.

To serve multiple users, run one process per user and give each process a separate `OCTOMIND_DATA_DIR`. That separates
its config, OAuth state, sessions, logs, and caches on every supported platform. Media uses a separate root, so set
`OCTOMIND_MEDIA_ROOT` as well when sharing a host. Use separate OS identities or containers when you need filesystem
isolation; the runtime socket directory is keyed to the OS user, not `OCTOMIND_DATA_DIR`.

```bash
OCTOMIND_DATA_DIR="$HOME/.octomind-alice" OCTOMIND_MEDIA_ROOT="$HOME/.octomind-alice/media" \
  octomind server --port 8081
```

## Logging

Logs are written to `logs/websocket-debug.log` under `OCTOMIND_DATA_DIR`. Without that override, the data root is
`~/.local/share/octomind` on macOS/Linux and `%LOCALAPPDATA%/octomind` on Windows. The file is opened at startup. Config
`log_level` defaults to `"info"`; `RUST_LOG` overrides the tracing filter. The tracing filter currently maps
unrecognized values, including config `"none"`, to `info`; use `RUST_LOG=off` to disable tracing.

```bash
RUST_LOG=debug octomind server
```

To configure debug logging persistently, set this top-level key in `config/config.toml` under the data root:

```toml
log_level = "debug"
```

## Protocol

Communication uses JSON messages over WebSocket.

### Client to Server

**Session creation** (auto-named):

```json
{
  "type": "session",
  "request_id": "req-003"
}
```

**Session creation** (named or resume):

```json
{
  "type": "session",
  "request_id": "req-004",
  "session_id": "my-session",
  "cwd": "/home/me/projects/app",
  "role": "developer:rust"
}
```

`session_id` is optional. Omit it to create an auto-named session. If you provide a name, the server resumes the on-disk
session at `sessions/<session_id>.jsonl.zst` under the data directory if it exists, otherwise it creates a new session
with that name. The `status` reply distinguishes the two: `"Session created: <id>"` vs `"Session resumed: <id>"` (a
`session` message never makes an AI call).

`cwd` is optional: an existing absolute directory the session runs in. Everything octomind resolves from the working
directory follows it — `AGENTS.md`, `.agents/` (guardrails, skills, tools), and the project id MCP servers scope their
state by. It is not stored with the session, so send it on every bind. One server can host sessions in many
directories at once; with `--sandbox`, writes stay confined to the server's own launch directory (plus the usual data
and temp paths), so keep session directories under it.

`role` is optional: an agent tag or role name a session **created** by this bind runs under, instead of the server's
role. The tag is resolved (and its MCP servers started) for this session only, and pinned to it — a later resume keeps
it. A resumed session keeps the role it already runs under whatever the bind says; switch it with the `role` command.
A role that fails to resolve returns an `error` and creates nothing.

**Cancel** — stop the session's running turn:

```json
{"type": "cancel", "request_id": "req-005", "session_id": "my-session"}
```

The server reads `cancel` frames while a turn runs, on any connection, and acknowledges them at once. The turn stops as
it would on Ctrl+C and ends with its usual `cost` frame, which carries `"cancelled": true` (no `error` frame). It also
stops a turn the session started by itself (a schedule, a delegated result). With no turn running, the ack is the only
reply.

**Close** — unload a session from server memory:

```json
{"type": "close", "request_id": "req-006", "session_id": "my-session"}
```

A long-running server keeps every bound session in memory. `close` drops the session and all of its session-scoped
state (including its schedules and background work) and replies with a `status` whose `data` is
`{"command_type": "close", "was_loaded": true|false}`. The session log stays on disk, so a later `session` bind resumes
it. Refused with a busy `error` while the session runs a turn.

`message` and `command` frames require an established session. Sending one for a `session_id` that is neither in memory
nor on disk returns:

```json
{
  "type": "error",
  "message": "Session not found: my-session. Send a \"session\" message first to create or resume a session."
}
```

The server never auto-creates a session from a `message`/`command` frame.

**Message** — send user input:

```json
{
  "type": "message",
  "request_id": "req-001",
  "session_id": "my-session",
  "content": "Explain the auth module"
}
```

**Command message** — execute session command:

```json
{
  "type": "command",
  "request_id": "req-002",
  "session_id": "my-session",
  "command": "mcp",
  "args": ["list"]
}
```

`request_id` is optional on every client frame. When present, the server echoes it in the immediate `ack` frame and in
validation errors, so clients can correlate accepted/rejected inputs without relying only on ordering.

**Message with attachments** — media uploaded out-of-band and referenced by opaque ID:

```json
{
  "type": "message",
  "session_id": "my-session",
  "content": "What is wrong with this screenshot?",
  "attachments": [
    {"id": "AbCdEf0123456789GhIjKlMn", "kind": "image", "media_type": "image/png", "name": "screenshot.png", "size": 1234}
  ]
}
```

`attachments` is optional. `content` may be empty when at least one attachment is present. `kind` is `image`, `video`,
or `audio`. `id` is exactly 24 ASCII alphanumeric characters and is never interpreted as a path: the server locates the
file in the media root (`OCTOMIND_MEDIA_ROOT`, default `/home/octo/.octomind/media`) whose name starts with `<id>.`. The
writer must store the file as `<id>.<ext>` — the extension is required, both because format detection needs it and so
the file stays browsable in a Files UI — and there must be exactly one such file; zero matches or multiple matches
produce an error. The file must be a regular file (symlinks are rejected). Before any file is opened, the server checks
that the session's model supports the requested modality (vision for `image`, video for `video`) and rejects the whole
message with an `error` frame otherwise. `audio` attachments are validated for readability only and are not forwarded to
the model yet.

To prepare the image example, place an actual PNG named `screenshot.png` in your current directory, then start with a
media root you control (the JSON `size` above is illustrative metadata; the server reads the actual file):

```bash
mkdir -p "$HOME/.octomind-media"
cp screenshot.png "$HOME/.octomind-media/AbCdEf0123456789GhIjKlMn.png"
OCTOMIND_MEDIA_ROOT="$HOME/.octomind-media" octomind server
```

`command` is the slash-command name **without** the leading `/` (see [Session
Commands](../reference/02-session-commands.md) for the full list). `args` is optional. The command channel only accepts
recognized commands: an unknown command returns an `error` frame — it is **not** treated as free-text AI input. Use a
`message` frame for that.

The `done` command (`/done`) compresses the conversation and replies with a data-carrying `status` frame (`"Conversation
compressed"` or `"Nothing to compress"`). With arguments, it then processes their joined text as a follow-up user
message:

```json
{"type":"command","session_id":"my-session","command":"done","args":["Summarize the remaining work"]}
```

### Concurrency

One connection processes one client frame at a time; frames sent while one runs wait and run in order afterwards. The
exceptions are `cancel` and `run_log`: both are read and answered immediately, even during a running frame.
`run_log` does not acquire the parent session lock.
Across connections, a per-session lock rejects a `message` or `command` while that session is processing another
request or inbox work:

```json
{
  "type": "error",
  "message": "Session 'my-session' is busy processing another request. Please wait."
}
```

Wait for completion before sending the next request. AI turns normally end with `cost`; commands end with a `status`
containing `data`, or an `error`. `done` with arguments sends its command status before the follow-up AI turn, so also
wait for that turn's `cost`. Errors can end requests without a cost frame. Use separate connections for concurrent work
on different sessions, and avoid creating/resuming the same session while it is busy.

### Server to Client

For each JSON text input that passes decoding and protocol validation, the server sends an acknowledgement before
dispatching it. This confirms receipt, not successful execution:

```json
{
  "type": "ack",
  "request_id": "req-001",
  "message_type": "message",
  "session_id": "my-session",
  "status": "received"
}
```

`request_id` and `session_id` are omitted when the input did not include them. Malformed JSON and validation failures do
not produce `ack`; they produce an `error` frame instead, except decoded `run_log` requests, whose validation failures
use a `run_log` reply with an `error` field. If a decoded frame fails protocol validation, its error echoes `request_id`. Some later errors, including busy-session and unknown-command errors, omit it. Response streams do not
echo request IDs; the `cost` frame closing a `message` turn does (see Cost tracking).

The `ack` for a `session` frame additionally carries `capabilities`, omitted on other acks:

- `message_attachments_v1` — `message` frames may include `attachments`.
- `session_control_v1` — the server accepts `cancel` and `close` frames and a bind `role`, and echoes a request's
  `request_id` on the cost frame that closes its turn.
- `run_events_v1` — delegated `tap` runs have their own live event stream and `run_log` replay.

A successful AI turn arrives as a **stream** of frames: zero or more `thinking`, `tool_use`, `tool_result`, and
`assistant` frames, terminated by a final `cost` frame that marks the end of the turn.

**Assistant response:**

```json
{
  "type": "assistant",
  "content": "The auth module validates credentials and creates sessions.",
  "session_id": "my-session"
}
```

`step` is an optional assistant field used by external workflow JSONL output; ordinary WebSocket session responses omit
it.

**Thinking content** (extended thinking models):

```json
{
  "type": "thinking",
  "content": "I will inspect the available tools first.",
  "session_id": "my-session"
}
```

**Delegated specialist runs (`run_events_v1`):**

After binding the parent with a `session` frame, every connection bound to it receives `run_event` frames for
`tap(action="run")` specialists. These arrive immediately, both during parent turns and between them, including after
`cost` with `pending_work: true`. Disconnected clients do not stop recording; reconnect, bind, and request a replay.
Tap child thinking, text, and tools no longer arrive as plain parent `thinking`, `assistant`, `tool_use`, or
`tool_result` frames. The parent's own output and the specialist's eventual inbox handback keep their existing shapes.
CLI, JSONL, and ACP tap output are unchanged.

```json
{"type":"run_event","session_id":"my-session","run_id":"tap-developer-cto-a3f1c2","role":"developer:cto","seq":17,"event":{"kind":"assistant","content":"Checking the implementation…"}}
```

`seq` starts at 1 for each run ID and increments for every emitted event. Resuming the same run ID continues its
sequence. `event` has one of these shapes:

```json
{"kind":"prompt","content":"Review the implementation"}
{"kind":"assistant","content":"Response chunk"}
{"kind":"thinking","content":"Reasoning chunk"}
{"kind":"tool_use","tool":"read","tool_id":"call-1","params":{"path":"src/main.rs"}}
{"kind":"tool_result","tool_id":"call-1","content":"File contents","success":true}
{"kind":"usage","cost":0.12,"input_tokens":1200,"output_tokens":300}
{"kind":"status","status":"running"}
{"kind":"status","status":"failed","error":"Failure details"}
```

Status is `running` at launch and `done`, `failed`, or `cancelled` at termination. `error` is optional. Prompt events
contain the task sent to the child on the initial and every resumed turn. Tool results include only terminal updates.
Usage reflects the child's cumulative `octomind.usage` metadata. An update that cannot be mapped to a run event is
logged and skipped; it does not stop the specialist or prevent subsequent events from streaming.

Request replay without waiting for a parent turn to finish:

```json
{"type":"run_log","session_id":"my-session","run_id":"tap-developer-cto-a3f1c2","request_id":"log-1"}
```

A valid request receives the usual `ack`, then:

```json
{"type":"run_log","session_id":"my-session","run_id":"tap-developer-cto-a3f1c2","role":"developer:cto","status":"running","source":"live","seq":3,"truncated":false,"events":[{"kind":"status","status":"running"},{"kind":"prompt","content":"Review the implementation"},{"kind":"assistant","content":"Checking…"}],"request_id":"log-1"}
```

While the run is running in this parent's registry, `source` is `live`. Consecutive assistant or thinking chunks are
coalesced in the snapshot. Its content buffer is capped at 4 MiB per run; oldest events are dropped when necessary and
`truncated` becomes `true`. A terminal event frees the live buffer. A resume starts a fresh buffer for that turn.

The live snapshot and its `seq` are captured atomically with event emission. While replay is outstanding, buffer
incoming `run_event` frames for that run. Install the snapshot, discard buffered/live frames with `seq <=` the reply's
`seq`, then apply frames with larger sequence numbers in order. This avoids gaps and duplicate chunks even if events
arrive before the reply. With `truncated: true`, the snapshot explicitly omits older content.

Otherwise, `source` is `disk`: the existing session loader reads `<sessions dir>/<run_id>.jsonl.zst`. Stored user,
assistant, thinking, tool-call, and tool-result messages map to the corresponding events; system messages and markers
are skipped. Disk replay reflects the loader's restored conversation (including compression/restoration boundaries).
Persisted tool messages currently have no success field, so disk `tool_result` events omit `success`; clients must not
interpret absence as success. `truncated` is `false`. Status comes from the parent's registry when known, otherwise
`unknown`; `role` is omitted when unknown. `seq` is the last observed sequence for a retained run record, or 0 without
one. `request_id` is echoed when supplied.

Run IDs must match the generated `tap-<role-slug>-<6 lowercase hex digits>` form. Validation and replay failures use a
`run_log` reply with an `error` field, never a turn-level `error` frame. Invalid IDs are rejected without an `ack`;
valid requests receive an `ack` before replay. Failed replies echo the session ID, run ID, and optional request ID,
omit `role`, and have this shape:

```json
{"type":"run_log","session_id":"my-session","run_id":"tap-missing-000000","status":"unknown","source":"disk","seq":0,"truncated":false,"events":[],"error":"run not found: tap-missing-000000","request_id":"log-1"}
```

Successful replies omit `error`. When neither live data nor a session file exists, the error message is
`run not found: <id>`. Replay failures do not mark an open parent turn as failed.

**Tool execution:**

```json
{
  "type": "tool_use",
  "tool": "capability",
  "tool_id": "call_123",
  "server": "runtime",
  "params": {"action": "list"},
  "session_id": "my-session"
}
```

**Tool result:**

```json
{
  "type": "tool_result",
  "tool": "capability",
  "tool_id": "call_123",
  "server": "runtime",
  "content": "No capabilities installed in any tap.",
  "success": true,
  "session_id": "my-session"
}
```

**Cost tracking** — all counters and cost are cumulative for the session, not just this request:

```json
{
  "type": "cost",
  "session_tokens": 9500,
  "session_cost": 0.0,
  "input_tokens": 5000,
  "output_tokens": 1000,
  "cache_read_tokens": 3000,
  "cache_write_tokens": 500,
  "reasoning_tokens": 0,
  "session_id": "my-session"
}
```

`pending_work: true` says the turn left work that will stream a follow-up turn on its own; `cancelled: true` says a
`cancel` frame stopped the turn. Both are omitted when false.

`request_id` echoes the id of the client frame whose turn this cost closes. Costs of turns the session runs by itself —
injected work drained ahead of a `message`, or a background inbox turn — carry none, so a client matches its request's
end exactly instead of counting cost frames. Every injected turn drained ahead of a message closes with its own cost,
including one that failed.

**Status:**

```json
{
  "type": "status",
  "message": "Command '/mcp list' executed successfully",
  "session_id": "my-session",
  "data": { "command_type": "mcp" }
}
```

Both `session_id` and `data` are optional. The connection-time welcome status omits `session_id`. Command completion
statuses always include `data`, either with command metadata (for plain handled commands) or the command's structured
JSON result (e.g. `mcp list`, `info`). This lets clients distinguish command completion from the connection/session
status frames.

**Error:**

```json
{
  "type": "error",
  "message": "session_id cannot be empty",
  "request_id": "req-001"
}
```

`request_id` appears only when the server can associate the error with a client-supplied ID.

**MCP notification:**

```json
{
  "type": "mcp_notification",
  "server": "filesystem",
  "method": "notifications/tools/list_changed",
  "params": {},
  "tool_id": "call_123"
}
```

`tool_id` is optional and appears when an MCP progress token can be associated with a tool call.

**Skill lifecycle:**

```json
{
  "type": "skill",
  "action": "activate",
  "name": "programming-rust",
  "trigger": "file(Cargo.toml)",
  "session_id": "my-session"
}
```

**Grounded behavior evolution lifecycle:**

```json
{
  "type": "evolution",
  "action": "promoted",
  "id": "evo-schema-check-a1b2c3d4",
  "name": "evolved-schema-check-a1b2c3",
  "kind": "validator",
  "state": "active",
  "scope": { "project": "octomind", "domain": "developer" },
  "session_id": "my-session"
}
```

Evolution is a dedicated event rather than a `status` with `data`, because existing clients treat data-bearing statuses
as command completion.

**Injected message** — a message added to the session by something other than the user, emitted just before the AI
processes it:

```json
{
  "type": "injected",
  "source_kind": "schedule",
  "source_label": "schedule abc12345",
  "content": "Run the test suite",
  "session_id": "my-session"
}
```

`source_kind` is one of: `schedule`, `monitor`, `background_agent`, `background_job`, `tap_run`, `skill`,
`skill_validator`, `inject`, `webhook`, `guardrail_hook`, `guardrail_validator`.

After a session is established, the server runs a background monitor that watches the session inbox (schedules,
background agents, webhooks). These can fire **asynchronously without any user prompt**, producing `injected` frames
followed by the normal `thinking`/`tool_use`/`tool_result`/`assistant`/`cost` stream. Clients should handle server
frames arriving at any time, not only in direct response to a `message`.

## Client Examples

### JavaScript/TypeScript

```typescript
// Start the server with --allow-origin matching your page's origin.
const ws = new WebSocket('ws://127.0.0.1:8080');
let sessionId = '';
let promptSent = false;

ws.onopen = () => {
  ws.send(JSON.stringify({
    type: 'session',
    request_id: 'create-1',
    session_id: 'my-session'
  }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  switch (msg.type) {
    case 'status':
      if (msg.session_id && !promptSent) {
        sessionId = msg.session_id;
        promptSent = true;
        ws.send(JSON.stringify({
          type: 'message',
          request_id: 'message-1',
          session_id: sessionId,
          content: 'Explain the auth module'
        }));
      }
      break;
    case 'assistant':
      console.log('AI:', msg.content);
      break;
    case 'tool_use':
      console.log('Tool:', msg.tool, msg.params);
      break;
    case 'cost':
      console.log('Turn complete; session tokens:', msg.session_tokens);
      break;
    case 'error':
      console.error('Error:', msg.message);
      break;
  }
};
```

### Python

Install the client dependency first:

```bash
python3 -m pip install websockets
```

```python
import asyncio
import json
import websockets

async def main():
    async with websockets.connect('ws://127.0.0.1:8080') as ws:
        # Create session
        await ws.send(json.dumps({
            'type': 'session',
            'session_id': 'my-session'
        }))

        while True:
            msg = json.loads(await ws.recv())
            if msg['type'] == 'error':
                raise RuntimeError(msg['message'])
            if msg['type'] == 'status' and msg.get('session_id'):
                session_id = msg['session_id']
                break

        # Send message
        await ws.send(json.dumps({
            'type': 'message',
            'session_id': session_id,
            'content': 'Explain the auth module'
        }))

        # Process responses
        async for message in ws:
            msg = json.loads(message)
            if msg['type'] == 'assistant':
                print(f"AI: {msg['content']}")
            elif msg['type'] == 'cost':
                break
            elif msg['type'] == 'error':
                raise RuntimeError(msg['message'])

asyncio.run(main())
```

## Common questions

| Symptom | What to check |
|---------|---------------|
| Browser handshake returns 403 | Start with `--allow-origin` matching the page's exact origin, as above. |
| An `ack` arrives but no assistant text | A command completes with `status.data`; also handle terminal `error` frames. |
| Session is busy | Wait for completion on the other connection or for background work to finish. |
| Attachment is missing or unsupported | Check the configured media root, unique ID filename, and model modality support. |
| Events arrive while the UI is idle | Inbox work produces unsolicited streams; keep receiving frames. |
| Binary upload is rejected | Send JSON text frames; store media out of band and send attachment references. |

The server has no TLS. `OCTOMIND_SERVER_TOKEN` authenticates every client; without it, any local process can connect.
Use an authenticated TLS proxy for remote deployment and a process boundary per identity.

## Validation

- `message.session_id` and `command.session_id` must be non-empty strings; `session.session_id` is optional and is
  passed to session setup when present

- `content` must be non-empty unless the message carries at least one attachment
- `request_id` is optional, but when provided must be non-empty and no more than 256 bytes
- Message `content` is limited to 10MB
- Attachment `id` must be exactly 24 ASCII alphanumeric characters
- Commands must be non-empty strings (without leading `/`)
- Command `args` is optional
- `session.cwd`, when present, must be an absolute path to an existing directory
- `session.role`, when present, must be non-empty
- `cancel.session_id`, `close.session_id`, and `run_log.session_id` must be non-empty
- `run_log.run_id` must be a generated tap run ID

A malformed JSON frame returns an `error` with a message starting `Invalid JSON:` and the connection **stays open** —
the same is true for validation failures, so clients can recover and keep sending.

### Transport limits

Separate from content validation, the transport layer enforces:

- **Max frame and message size: 10MB.** Larger frames/messages are rejected by the WebSocket layer.
- **Unmasked frames are rejected.** Per spec, client frames must be masked; standard clients do this automatically.
- **Ping/Pong:** the server replies to client `Ping` frames with `Pong` to keep the connection alive — also while a
  turn is running.
- **Close:** a client `Close` frame is answered with the server's own close frame before the socket ends.

## See also

- [ACP Protocol](02-acp-protocol.md) — alternate stdio transport.
- [Protocol source](../../src/websocket/protocol.rs) and [server source](../../src/websocket/server.rs).
- [CLI flags](../../src/commands/server.rs), [paths](../../src/directories.rs), and [logging
  filter](../../src/logging/tracing_setup.rs).

- [Structured Output](../usage/11-structured-output.md) — the JSONL output mode shares this same `ServerMessage` schema.
- [Session Commands](../reference/02-session-commands.md) — the commands usable over the `command` channel.
