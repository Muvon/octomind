# WebSocket Server

Octomind provides a WebSocket server for remote AI sessions, enabling programmatic access from web clients, bots, and automation tools.

## Quick Start

```bash
# Start server
octomind server --host 127.0.0.1 --port 8080

# Connect with websocat
websocat ws://127.0.0.1:8080
```

## Starting the Server

```bash
octomind server [TAG] [OPTIONS]
```

| Flag | Default | Description |
|------|---------|-------------|
| `TAG` | config default | Agent tag or role name |
| `--host` | `127.0.0.1` | Bind address |
| `--port` | `8080` | Port |
| `--sandbox` | `false` | Restrict filesystem writes |

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

`session_id` is optional — omit it to create an auto-named session, or provide a name to create or resume.

### Server to Client

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
  "server": "core",
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
  "server": "core",
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
  "message": "Processing...",
  "session_id": "my-session"
}
```

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

**Injected message:**
```json
{
  "type": "injected",
  "source_kind": "schedule",
  "source_label": "schedule abc12345",
  "content": "Run the test suite",
  "session_id": "my-session"
}
```

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
      console.error('Error:', msg.content);
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
                print(f"Error: {msg['content']}")

asyncio.run(main())
```

## Validation

- `session_id` (when provided) and `content` must be non-empty strings
- Message content is limited to 10MB
- Commands must be non-empty strings (without leading `/`)
- Command `args` is optional

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

WebSocket server logs to `~/.local/share/octomind/logs/websocket-debug.log` when `log_level = "debug"`.
