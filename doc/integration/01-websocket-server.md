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
first: a browser page whose origin is not in `--allow-origin` is refused with `403` whatever it sends, and an
allowlisted origin still needs the token, or it gets `401`.

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
  "role": "developer:rust",
  "capabilities": ["elicitation_v1"]
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

`resume` is optional: `{"stream": "<id>", "after": <stream_seq>}` from a client that was reading this session's output
and lost its connection. See [Stream position and resume](#stream-position-and-resume).

`capabilities` is optional: what this client implements. Send it on every bind.

- `elicitation_v1`: the client answers MCP servers' questions (see [MCP elicitation](#mcp-elicitation) below). Without
  it the session declines them. It belongs to whichever connection owns the session's output.
- `commands_v1`: the bind's ack lists the slash commands the session can run in `commands` (see below).

A bind with `resume` takes the session's output over: from then on every frame the session produces goes to this
connection, including the rest of a turn another connection started. A bind without `resume` takes it only when no live
connection owns it (the owner disconnected); otherwise output stays where it is until this connection sends a `message`
or `command`, whose turn it then reads.

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

**Elicitation response** — answer an MCP server's question (see [MCP elicitation](#mcp-elicitation)):

```json
{"type": "elicitation_response", "request_id": "req-007", "session_id": "my-session", "id": "e1", "result": {"action": "accept", "content": {"size": "L"}}}
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

**Observe** — make the connection a read-only observer of every session on the server (see [Observer connections](#observer-connections)):

```json
{"type": "observe", "sessions": [{"session_id": "my-session", "stream": "3f9a0c6e2b7d41e8a5c09d6f1b2e7a34", "after": 41}]}
```

`sessions` is optional: one mark per session saying where an earlier observer connection stopped reading. `observe` is
answered by an `observing` frame, not an `ack`.

### Concurrency

One connection processes one client frame at a time; frames sent while one runs wait and run in order afterwards. The
exceptions are `cancel`, `run_log` and `elicitation_response`: all are read and answered immediately, even during a
running frame.
`run_log` does not acquire the parent session lock.
Across connections, a per-session lock rejects a `message` or `command` while that session is processing another
request or inbox work:

```json
{
  "type": "error",
  "message": "Session 'my-session' is busy processing another request. Please wait.",
  "request_id": "req-001"
}
```

The busy error echoes the rejected frame's `request_id` (omitted when the frame had none).

Wait for completion before sending the next request. AI turns normally end with `cost`; commands end with a `status`
containing `data`, or an `error`. `done` with arguments sends its command status before the follow-up AI turn, so also
wait for that turn's `cost`. Errors can end requests without a cost frame. Use separate connections for concurrent work
on different sessions.

A turn does not depend on the connection that started it. If that client disconnects, the turn keeps running and its
frames are held for the session's next bind (see [Stream position and resume](#stream-position-and-resume)); until it
ends, other frames for the session get the busy error. One connection owns a session's output at a time: the one whose
`message` or `command` started the current turn, unless a later bind took it over (see `resume` above). Background turns
(schedules, delegated results) stream to that owner, or are only held when no connection owns the output.

### Server to Client

For each JSON text input that passes decoding and protocol validation, the server sends an acknowledgement before
dispatching it — except a `session` bind, which is acknowledged once the session is bound (or the bind failed), so its
ack can report the session's stream position. This confirms receipt, not successful execution:

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
use a `run_log` reply with an `error` field. If a decoded frame fails protocol validation, its error echoes `request_id`. Some later errors, including unknown-command errors, omit it; busy-session errors echo it. Response streams do not
echo request IDs; the `cost` frame closing a `message` turn does (see Cost tracking).

The `ack` for a `session` frame additionally carries `capabilities`, omitted on other acks:

- `message_attachments_v1` — `message` frames may include `attachments`.
- `session_control_v1` — the server accepts `cancel` and `close` frames and a bind `role`, and echoes a request's
  `request_id` on the cost frame that closes its turn.
- `run_events_v1` — delegated `tap` runs have their own live event stream and `run_log` replay.
- `stream_resume_v1` — session output carries `stream_seq`, and a bind may `resume` (below).
- `elicitation_v1` — MCP servers can ask the user for input mid-tool-call (below). A client opts in by listing
  `"capabilities": ["elicitation_v1"]` in its `session` bind; without it the session declines every elicitation.
- `commands_v1` — a client that lists it in its `session` bind gets `commands` on that bind's ack: the slash commands
  the session can run, in `/help` order, each `{"name", "description", "hint"?}`. `name` has no leading `/` and is what
  a `command` frame carries (custom commands are listed as `run <name>`). `hint` is the argument shape, such as
  `[provider:model]`, and is absent only when the command takes no argument. Commands that act on the CLI's own terminal
  (`copy`, `reply`, `clear`, `new`, `image`, `video`, `analyze`, `exit`) are left out. The list follows the config the
  session runs under, so read it from every bind's ack. Without the capability the ack has no `commands`.
- `observe_v1` — the server accepts an `observe` frame and turns that connection into a read-only observer (see
  [Observer connections](#observer-connections)). It is listed in the `observing` frame too.

```json
{"type":"ack","message_type":"session","session_id":"my-session","status":"received","capabilities":["…","commands_v1"],"commands":[{"name":"help","description":"Show this help message"},{"name":"model","description":"View or change current AI model","hint":"[provider:model]"}],"stream":"3f9a0c6e2b7d41e8a5c09d6f1b2e7a34","stream_seq":0}
```

#### Provider retries

A `retry` frame identifies a failed model request while the turn waits to try again. `purpose` is `main`,
`supervisor`, or `compression` (compression decisions and summaries). `provider` and `model` identify the request
sent by Octomind; proxy aliases such as `octohub:auto` are resolved upstream. `attempt` counts retries, starting
at 1, and `max_retries` excludes the original request. `reason` is a short cause such as `HTTP 503`,
`connection error`, or the generic `request failed`; it contains no response body or credentials.
Clients can display `message` directly. A retry frame does not end the turn.

```json
{"type":"retry","purpose":"compression","provider":"octohub","model":"auto","attempt":1,"max_retries":1,"delay_ms":30000,"reason":"request failed","message":"Model request failed [compression, octohub:auto] (request failed) — retrying in 30s, retry 1/1","session_id":"my-session","stream_seq":7}
```

#### Stream position and resume

A session loaded in the server process owns one output **stream**, identified by an opaque hex id that changes when the
process restarts or the session is closed and loaded again. Every frame the session produces — `assistant`, `thinking`,
`tool_use`, `tool_result`, `cost`, `status`, `error`, `mcp_notification`, `skill`, `evolution`, `injected`, `retry`, `elicitation` —
carries a top-level `stream_seq`: 1, 2, 3 … per stream, with no gaps, including frames produced while no connection is
bound. Frames that answer one client frame on that connection only carry none: `ack`, `run_log`, the bind's own
`status`, the `close` reply, and errors that refuse a frame before its turn starts (busy, `Session not found`, a bad
attachment, `Internal error`, malformed or invalid input).
`run_event` keeps its own per-run `seq`.

```json
{"type":"assistant","content":"The auth module validates credentials.","session_id":"my-session","stream_seq":7}
```

A successful bind's ack reports the stream and the highest `stream_seq` assigned so far (0 if none):

```json
{"type":"ack","request_id":"bind-2","message_type":"session","session_id":"my-session","status":"received","capabilities":["message_attachments_v1","session_control_v1","run_events_v1","stream_resume_v1","elicitation_v1","commands_v1","observe_v1"],"stream":"3f9a0c6e2b7d41e8a5c09d6f1b2e7a34","stream_seq":7,"resumed":true}
```

A failed bind's ack carries no `stream` or `stream_seq`. The server holds the newest frames of each stream, up to 2 MiB
and 4096 frames per stream (oldest dropped first; the newest frame is always kept, however large). A client that lost its connection binds again
with `"resume": {"stream": "<id>", "after": <last stream_seq it received>}`. When the stream matches, every frame
after that point is still held, and those frames fit one replay (8 MiB and 4096 frames, half the connection's outbound
limits, so a single held frame larger than that is never replayed), the ack carries `"resumed": true` and those frames follow it at once, in order and
byte-identical to the first sending, before the bind's `status` and any new frame; then the stream continues live,
including the rest of a turn that was running. Otherwise the ack carries `"resumed": false`, nothing is replayed, and the
client reloads its state from its own store. `resumed` is present only when the bind asked to resume.

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
coalesced in the snapshot. Its buffer is capped at 4 MiB of content or 4096 events per run; oldest events are dropped when necessary and
`truncated` becomes `true`. A terminal event frees the live buffer. A resume starts a fresh buffer for that turn.

The live snapshot and its `seq` are captured atomically with event emission. While replay is outstanding, buffer
incoming `run_event` frames for that run. Install the snapshot, discard buffered/live frames with `seq <=` the reply's
`seq`, then apply frames with larger sequence numbers in order. This avoids gaps and duplicate chunks even if events
arrive before the reply. With `truncated: true`, the snapshot explicitly omits older content.

Otherwise, `source` is `disk`: the existing session loader reads `<sessions dir>/<run_id>.jsonl.zst`. Stored user,
assistant, thinking, tool-call, and tool-result messages map to the corresponding events; system messages and markers
are skipped. Disk replay reflects the loader's restored conversation (including compression/restoration boundaries),
with the same content and event limits as live snapshots. A capped reply sets `truncated: true`.
Persisted tool messages currently have no success field, so disk `tool_result` events omit `success`; clients must not
interpret absence as success. Status comes from the parent's registry when known, otherwise
`unknown`; `role` is omitted when unknown. `seq` is the last observed sequence for a retained run record, or 0 without
one. `request_id` is echoed when supplied. When the session recorded usage, a final `usage` event carries its totals
(cost includes the run's own sub-delegations); a session with no recorded usage emits none.

With `"summary":true`, the reply has the same fields but `events` holds only the latest `usage` event (empty when
none is known) and `truncated` is `false`. Servers that predate the field ignore it and send the full log.

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

After a session is loaded — by a bind, or by a `message` or `command` for a session on disk — the server runs one
background monitor for it, until the session is unloaded (`close`, `/exit`). It watches the session inbox (schedules,
background agents, webhooks, delegated results). These fire **asynchronously without any user prompt**, producing
`injected` frames followed by the normal `thinking`/`tool_use`/`tool_result`/`assistant`/`cost` stream. The monitor
belongs to the session, not to a connection: a timed or idle schedule, or a delegated result that lands after every
client disconnected, still runs, and its frames go on the session's stream. A client that binds later with `resume`
reads them; an [observer](#observer-connections) receives them live. Clients should handle server frames arriving at
any time, not only in direct response to a `message`.

#### MCP elicitation

When an MCP server sends `elicitation/create`, a client that bound with `elicitation_v1` receives an `elicitation`
frame. `params` is MCP's own request, unchanged (`"mode":"form"` with `message` and `requestedSchema`, or URL mode).
`tool_id` is present when the question belongs to a known tool call.

```json
{"type":"elicitation","session_id":"my-session","id":"e1","server":"forms","tool_id":"call-1","params":{"mode":"form","message":"Pick a size","requestedSchema":{"type":"object","properties":{"size":{"type":"string"}}}}}
```

Answer with an `elicitation_response` carrying the same `id` and MCP's own result (`action` is `accept`, `decline` or
`cancel`; `content` accompanies `accept`):

```json
{"type":"elicitation_response","session_id":"my-session","id":"e1","result":{"action":"accept","content":{"size":"L"}}}
```

The response is acknowledged and applied at once, even while the session's turn is running. An `id` with no pending
question (already answered, or its tool call ended) gets an `error` frame echoing the response's `request_id`, with
`"code": "elicitation_gone"` — match the code, since proxies may rewrite error text:

```json
{"type":"error","message":"no pending elicitation 'e1'","request_id":"req-007","code":"elicitation_gone"}
```

Questions are answered by the connection that owns the session's output (see `resume` above), when its latest bind
listed `elicitation_v1`; otherwise the session declines them. When a capable connection takes the output over, the
server re-sends every unanswered `elicitation` frame after the bind's ack and any resumed frames, so clients dedupe by
`id`. A question ends — answered as `cancel` — when its tool call ends, when no live connection owns the output any
more, or when the session is closed. A client should close any open form when the turn's `cost` frame arrives.

#### Observer connections

An observer records what the server's sessions do without driving any of them: one observer connection sees every
session's output, including turns that run while no client is bound. Send `observe` once, before the connection binds
any session (after a bind it is refused with `observe must come before any session bind on a connection`, because a
connection that owns a session's output or answers its questions cannot stop doing so); the connection is an observer
for its lifetime. Any other client frame on it (`session`, `message`, `cancel`, a second `observe`, …) is answered with
`{"type":"error","message":"observer connections are read-only"}` and the connection stays open. An observer never owns a
session's output, never answers elicitations and does not count as a bound client. Any number of observers may be
connected; each gets everything.

Frames the server sends an observer:

1. `observing`, always first, with the same `capabilities` a bind ack lists (`observe_v1` included):

   ```json
   {"type":"observing","capabilities":["message_attachments_v1","…","observe_v1"]}
   ```

2. For every stream open on the server, `stream_open`, then that stream's replay. A stream that opens later (a session
   is loaded) sends `stream_open` with `last_seq` 0 and `gap` false before its first frame:

   ```json
   {"type":"stream_open","session_id":"my-session","stream":"3f9a0c6e2b7d41e8a5c09d6f1b2e7a34","last_seq":41,"gap":false}
   ```

3. `observed`, for every frame a session stream numbers, replayed or live. `frame` is the exact frame a bound client
   reads, `stream_seq` included:

   ```json
   {"type":"observed","session_id":"my-session","stream":"3f9a0c6e2b7d41e8a5c09d6f1b2e7a34","frame":{"type":"assistant","content":"Done.","session_id":"my-session","stream_seq":42}}
   ```

4. `stream_closed`, when the session is unloaded (`close`, `/exit`). Loading it again opens a new stream:

   ```json
   {"type":"stream_closed","session_id":"my-session","stream":"3f9a0c6e2b7d41e8a5c09d6f1b2e7a34"}
   ```

Frames of one session arrive in order, and the replay joins the live frames with no gap and no duplicate. The replay
comes from the same 2 MiB hold a `resume` bind uses:

- A mark for the session with the **same** `stream` replays the held frames with `stream_seq` greater than `after`.
  `gap` is `true` when the oldest held frame is numbered above `after + 1`: the hold dropped frames the observer never
  saw, so record that output between `after` and the first replayed `stream_seq` was lost.
- No mark, or a mark with a different `stream` (the session was reloaded or the server restarted), replays every held
  frame. `gap` is `true` when the oldest held `stream_seq` is above 1.

One `observe` replays at most 8 MiB and 4096 frames in all, half the connection's outbound limits, so the replay always
fits. A stream whose frames do not fit replays only its newest ones and sets `gap` to `true` (as does a stream that gets
none). The frames left out are not replayed later: record the range as lost, exactly as for frames the hold dropped. A
single held frame larger than the whole budget is therefore never replayed to an observer.

An observer has the same outbound limits as any connection. One that cannot keep up is disconnected; it reconnects with
its marks. Frames that answer one client frame (`ack`, `run_log`, bind statuses) belong to the connection that sent
the request and are never observed.

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
| An MCP tool's question is declined | Bind with `"capabilities": ["elicitation_v1"]` and answer `elicitation` frames. |
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
- `session.resume`, when present, needs a non-empty `stream` and a numeric `after`
- `cancel.session_id`, `close.session_id`, and `run_log.session_id` must be non-empty
- `elicitation_response.session_id` and `elicitation_response.id` must be non-empty; `result.action` is `accept`,
  `decline` or `cancel`
- `run_log.run_id` must be a generated tap run ID

A malformed JSON frame returns an `error` with a message starting `Invalid JSON:` and the connection **stays open** —
the same is true for validation failures, so clients can recover and keep sending.

### Transport limits

Separate from content validation, the transport layer enforces:

- **Max frame and message size: 10MB.** Larger frames/messages are rejected by the WebSocket layer.
- **Pending input: 256 frames or 10 MiB per connection.** Excess input receives an error with its `request_id` when
  supplied. Already queued frames retain their order; cancellation and elicitation responses remain available.
- **Outgoing queue: 8192 buffered frames or 16 MiB per connection**, with the byte limit also covering the frame being written. A client
  that exceeds either limit is disconnected; its running turn continues and holds output for stream resume.
- **64 concurrent connection handlers**, with a 10-second handshake deadline. Further connections wait for capacity.
- **128 loaded sessions.** Existing sessions can reconnect at capacity. Loading another requires a `close` frame for
  an idle session; disconnecting alone keeps session state and background handbacks available.
- **Unmasked frames are rejected.** Per spec, client frames must be masked; standard clients do this automatically.
- **Ping/Pong:** the server replies to client `Ping` frames with `Pong` to keep the connection alive — also while a
  turn is running.
- **Close:** a client `Close` frame is answered with the server's own close frame before the socket ends.

WebSocket and ACP specialist processes using the same OS runtime directory share one embedding model owner. Other
processes connect to that owner without loading weights. Each process's vector cache holds at most 8192 vectors;
the service admits 16 concurrent clients, bounds protocol lines to 8 MiB, and splits requests into 256-text batches.
If an owner exits, a client re-elects on a transport failure and resubmits the read-only inference once.

## See also

- [ACP Protocol](02-acp-protocol.md) — alternate stdio transport.
- [Protocol source](../../src/websocket/protocol.rs) and [server source](../../src/websocket/server.rs).
- [CLI flags](../../src/commands/server.rs), [paths](../../src/directories.rs), and [logging
  filter](../../src/logging/tracing_setup.rs).

- [Structured Output](../usage/11-structured-output.md) — the JSONL output mode shares this same `ServerMessage` schema.
- [Session Commands](../reference/02-session-commands.md) — the commands usable over the `command` channel.
