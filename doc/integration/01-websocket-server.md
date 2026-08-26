# WebSocket Server

Octomind provides a WebSocket server for remote AI sessions, enabling programmatic access from web clients, bots, and automation tools.

## Quick Start

```bash
# Start server
octomind server --host 127.0.0.1 --port 8080

# Connect with websocat
websocat ws://127.0.0.1:8080
```

On connect, the server sends a single `status` frame (`"Connected to Octomind WebSocket server..."`). Nothing else happens until you send a `session` message — that must be the **first frame** you send. Only after a session is established will `message` and `command` frames work.

## Starting the Server

```bash
octomind server [TAG] [OPTIONS]
```

| Flag | Default | Description |
|------|---------|-------------|
| `TAG` | config default | Agent tag (e.g. `developer:general`) or role name (e.g. `developer`) |
| `--host` | `127.0.0.1` | Bind address |
| `--port`, `-p` | `8080` | Port |
| `--sandbox` | `false` | Restrict all filesystem writes to the current working directory |
| `--allow-origin` | none | Browser origin permitted to connect. Repeatable |

## Browser origins

WebSocket upgrades are covered by neither CORS nor the same-origin policy. Without an origin check, **any** page the operator visits -- including a third-party iframe -- can open a socket to a loopback-bound server and drive the agent with the full configured toolset, reading every response frame back. Binding to `127.0.0.1` is not a boundary against this.

The server therefore refuses any handshake that carries an `Origin` header not listed in `--allow-origin`, with `HTTP 403` before the welcome frame:

```bash
octomind server --allow-origin http://localhost:3000 --allow-origin https://dashboard.example.com
```

Origins are matched exactly, as sent by the browser (scheme, host, and port; no trailing slash). Native clients -- `websocat`, the Python and Node examples below, anything that is not a browser -- send no `Origin` header and connect without configuration.

## Single principal per process

The server has no notion of a user. One process serves one identity: config, MCP server processes, and OAuth tokens are process-global and shared by every session, so all tool calls from every session go out with the same credentials. `session_id` is a name, not a capability -- any connection may resume any session.

To serve multiple users, run one process per user, each with its own `HOME`. The data directory derives from it, which gives each process a separate OAuth keystore, session store, and config.

## Protocol

Communication uses JSON messages over WebSocket.

### Client to Server

**Message** -- send user input:
```json
{
  "type": "message",
  "request_id": "req-001",
  "session_id": "my-session",
  "content": "Explain the auth module"
}
```

**Command message** -- execute session command:
```json
{
  "type": "command",
  "request_id": "req-002",
  "session_id": "my-session",
  "command": "mcp",
  "args": ["list"]
}
```

`request_id` is optional on every client frame. When present, the server echoes it in the immediate `ack` frame and in validation errors, so clients can correlate accepted/rejected inputs without relying only on ordering.

**Message with attachments** -- media uploaded out-of-band and referenced by opaque ID:
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

`attachments` is optional. `content` may be empty when at least one attachment is present. `kind` is `image`, `video`, or `audio`. `id` is exactly 24 ASCII alphanumeric characters and is never interpreted as a path: the server locates the file in the media root (`OCTOMIND_MEDIA_ROOT`, default `/home/octo/.octomind/media`) whose name starts with `<id>.`. The writer must store the file as `<id>.<ext>` — the extension is required, both because format detection needs it and so the file stays browsable in a Files UI — and there must be exactly one such file, or the attachment is reported as not found. The file must be a regular file (symlinks are rejected). Before any file is opened, the server checks that the session's model supports the requested modality (vision for `image`, video for `video`) and rejects the whole message with an `error` frame otherwise. `audio` attachments are validated for readability only and are not forwarded to the model yet.

`command` is the slash-command name **without** the leading `/` (see [Session Commands](../reference/02-session-commands.md) for the full list). `args` is optional. The command channel only accepts recognized commands: an unknown command returns `{"type":"error","message":"Unknown command: '...'..."}` — it is **not** treated as free-text AI input. Use a `message` frame for that.

The `done` command (`/done`) is special: it compresses the conversation and replies with a data-carrying `status` frame (`"Conversation compressed"` or `"Nothing to compress"`). If you supply `args`, they are joined and immediately processed as a follow-up user message after compression.

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
  "session_id": "my-session"
}
```

`session_id` is optional. Omit it to create an auto-named session. If you provide a name, the server resumes the on-disk session at `~/.local/share/octomind/sessions/<session_id>.jsonl.zst` if it exists, otherwise it creates a new session with that name. The `status` reply distinguishes the two: `"Session created: <id>"` vs `"Session resumed: <id>"` (a `session` message never makes an AI call).

`message` and `command` frames require an established session. Sending one for a `session_id` that is neither in memory nor on disk returns:

```json
{
  "type": "error",
  "message": "Session not found: my-session. Send a \"session\" message first to create or resume a session."
}
```

The server never auto-creates a session from a `message`/`command` frame.

#### Concurrency

Each session is processed **serially**. While a session is busy handling a `message` or `command`, any concurrent `message`/`command` for that same session is rejected (not queued) with:

```json
{
  "type": "error",
  "message": "Session 'my-session' is busy processing another request. Please wait."
}
```

Wait for the prior request to finish — i.e. for its terminating `cost` frame (see below) — before sending the next one. Different `session_id`s run independently.

### Server to Client

For every valid JSON text input frame (`session`, `message`, or `command`), the server first sends an immediate acknowledgement before doing any longer work:

```json
{
  "type": "ack",
  "request_id": "req-001",
  "message_type": "message",
  "session_id": "my-session",
  "status": "received"
}
```

`request_id` and `session_id` are omitted when the input did not include them. Malformed JSON and validation failures do not produce `ack`; they produce an `error` frame instead. If a validation failure includes a `request_id`, the error echoes it.

The `ack` for a `session` frame additionally carries `"capabilities": ["message_attachments_v1"]`, advertising that `message` frames may include `attachments`. It is omitted on other acks.

Responses to a single `message` arrive as a **stream** of frames: zero or more `thinking`, `tool_use`, `tool_result`, and `assistant` frames, terminated by a final `cost` frame that marks the end of the turn.

**Assistant response:**
```json
{
  "type": "assistant",
  "content": "The auth module handles...",
  "session_id": "my-session"
}
```

**Thinking content** (extended thinking models):
```json
{
  "type": "thinking",
  "content": "Let me analyze...",
  "session_id": "my-session"
}
```

**Tool execution:**
```json
{
  "type": "tool_use",
  "tool": "view",
  "tool_id": "call_123",
  "server": "filesystem",
  "params": {"path": "src/auth.rs"},
  "session_id": "my-session"
}
```

**Tool result:**
```json
{
  "type": "tool_result",
  "tool": "view",
  "tool_id": "call_123",
  "server": "filesystem",
  "content": "file contents...",
  "success": true,
  "session_id": "my-session"
}
```

**Cost tracking:**
```json
{
  "type": "cost",
  "session_tokens": 15000,
  "session_cost": 0.045,
  "input_tokens": 5000,
  "output_tokens": 1000,
  "cache_read_tokens": 3000,
  "cache_write_tokens": 500,
  "reasoning_tokens": 0,
  "session_id": "my-session"
}
```

**Status:**
```json
{
  "type": "status",
  "message": "Command 'mcp' executed successfully",
  "session_id": "my-session",
  "data": { "command_type": "mcp" }
}
```

Both `session_id` and `data` are optional. The connection-time welcome status omits `session_id`. Command completion statuses always include `data`, either with command metadata (for plain handled commands) or the command's structured JSON result (e.g. `mcp list`, `info`). This lets clients distinguish command completion from the connection/session status frames.

**Error:**
```json
{
  "type": "error",
  "message": "Invalid session ID",
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
  "params": {}
}
```

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

**Injected message** -- a message added to the session by something other than the user, emitted just before the AI processes it:
```json
{
  "type": "injected",
  "source_kind": "schedule",
  "source_label": "schedule abc12345",
  "content": "Run the test suite",
  "session_id": "my-session"
}
```

`source_kind` is one of: `schedule`, `background_agent`, `tap_run`, `skill`, `skill_validator`, `inject`, `webhook`, `guardrail_hook`, `guardrail_validator`.

After a session is established, the server runs a background monitor that watches the session inbox (schedules, background agents, webhooks). These can fire **asynchronously without any user prompt**, producing `injected` frames followed by the normal `thinking`/`tool_use`/`tool_result`/`assistant`/`cost` stream. Clients should handle server frames arriving at any time, not only in direct response to a `message`.

## Client Examples

### JavaScript/TypeScript

```typescript
const ws = new WebSocket('ws://127.0.0.1:8080');

ws.onopen = () => {
  // Create session
  ws.send(JSON.stringify({
    type: 'session',
    session_id: 'my-session'
  }));

  // Send message
  ws.send(JSON.stringify({
    type: 'message',
    session_id: 'my-session',
    content: 'Explain the auth module'
  }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  switch (msg.type) {
    case 'assistant':
      console.log('AI:', msg.content);
      break;
    case 'tool_use':
      console.log('Tool:', msg.tool, msg.params);
      break;
    case 'cost':
      console.log(`Cost: $${msg.session_cost}`);
      break;
    case 'error':
      console.error('Error:', msg.message);
      break;
  }
};
```

### Python

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

        # Send message
        await ws.send(json.dumps({
            'type': 'message',
            'session_id': 'my-session',
            'content': 'Explain the auth module'
        }))

        # Process responses
        async for message in ws:
            msg = json.loads(message)
            if msg['type'] == 'assistant':
                print(f"AI: {msg['content']}")
            elif msg['type'] == 'error':
                print(f"Error: {msg['message']}")

asyncio.run(main())
```

## Validation

- `session_id` (when provided) must be a non-empty string
- `content` must be non-empty unless the message carries at least one attachment
- `request_id` is optional, but when provided must be non-empty and no more than 256 bytes
- Message `content` is limited to 10MB
- Attachment `id` must be exactly 24 ASCII alphanumeric characters
- Commands must be non-empty strings (without leading `/`)
- Command `args` is optional

A malformed JSON frame returns `{"type":"error","message":"Invalid JSON: ..."}` and the connection **stays open** — the same is true for validation failures, so clients can recover and keep sending.

### Transport limits

Separate from content validation, the transport layer enforces:

- **Max frame size: 10MB.** Frames larger than this are rejected by the WebSocket layer.
- **Unmasked frames are rejected.** Per spec, client frames must be masked; standard clients do this automatically.
- **Ping/Pong:** the server replies to client `Ping` frames with `Pong` to keep the connection alive.

## Security

The server binds to `127.0.0.1` by default (localhost only). For production:

- Use a reverse proxy (nginx, Caddy) with TLS
- Add authentication at the proxy layer
- Rate limit connections
- Never expose directly to the internet without auth

```nginx
# nginx reverse proxy example
location /ws {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

## Logging

The WebSocket server writes file logs to `~/.local/share/octomind/logs/websocket-debug.log`. The file is always opened; verbosity follows the configured `log_level` (`none` / `info` / `debug`, default `info`). Set `log_level = "debug"` for full request/message tracing.

## See also

- [Structured Output](../usage/11-structured-output.md) — the JSONL output mode shares this same `ServerMessage` schema.
- [Session Commands](../reference/02-session-commands.md) — the commands usable over the `command` channel.
