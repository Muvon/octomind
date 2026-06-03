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

## Protocol

Communication uses JSON messages over WebSocket.

### Client to Server

**Session message** -- send user input:
```json
{
  "type": "message",
  "session_id": "my-session",
  "content": "Explain the auth module"
}
```

**Command message** -- execute session command:
```json
{
  "type": "command",
  "session_id": "my-session",
  "command": "mcp",
  "args": ["list"]
}
```

`command` is the slash-command name **without** the leading `/` (see [Session Commands](../reference/02-session-commands.md) for the full list). `args` is optional. The command channel only accepts recognized commands: an unknown command returns `{"type":"error","message":"Unknown command: '...'..."}` — it is **not** treated as free-text AI input. Use a `message` frame for that.

The `done` command (`/done`) is special: it compresses the conversation and replies with a `status` frame (`"Conversation compressed"` or `"Nothing to compress"`). If you supply `args`, they are joined and immediately processed as a follow-up user message after compression.

**Session creation** (auto-named):
```json
{
  "type": "session"
}
```

**Session creation** (named or resume):
```json
{
  "type": "session",
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
  "data": { "...": "structured command output" }
}
```

Both `session_id` and `data` are optional. The connection-time welcome status omits `session_id`. The `data` field is present only for commands that return structured output (e.g. `mcp list`, `info`) — it carries that command's JSON result.

**Error:**
```json
{
  "type": "error",
  "message": "Invalid session ID"
}
```

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

- `session_id` (when provided) and `content` must be non-empty strings
- Message `content` is limited to 10MB
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
