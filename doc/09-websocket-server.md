# WebSocket Server

> **Real-time AI development sessions over WebSocket**

The WebSocket server provides remote access to Octomind's AI capabilities through a WebSocket interface. It uses the same session logic as the CLI, ensuring feature parity and consistent behavior.

## 🚀 Quick Start

### Start the Server

```bash
# Start with default settings (localhost:8080, developer role)
octomind server

# Custom host and port
octomind server --host 0.0.0.0 --port 3000

# Use assistant role (chat-only, no tools)
octomind server --role assistant
```

### Connect with a Client

```javascript
const ws = new WebSocket('ws://localhost:8080');

ws.onopen = () => {
  // First create a session
  ws.send(JSON.stringify({ type: 'session' }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  if (msg.type === 'status' && msg.session_id) {
    // Session ready — send a message
    ws.send(JSON.stringify({
      type: 'message',
      session_id: msg.session_id,
      content: 'List files in the src directory'
    }));
  }
  console.log(`[${msg.type}]`, msg);
};
```

## 📋 Protocol Specification

### Message Format

All messages are JSON-encoded.

### Client → Server (Input)

Every message **must** include a `type` field.

#### `session` — Create or resume a session

```json
{ "type": "session" }
{ "type": "session", "session_id": "my-feature-x" }
```

- `session_id` absent → create new auto-named session
- `session_id` present → resume if exists on disk, otherwise create with that name
- No AI call is made. Server responds with `status` containing the `session_id`.

#### `message` — Send user input to an existing session

```json
{
  "type": "message",
  "session_id": "my-feature-x",
  "content": "Fix the authentication bug"
}
```

- `session_id` **required** — must refer to an established session
- `content` **required** — the user input sent to the AI

#### `command` — Execute a session command

```json
{ "type": "command", "session_id": "my-feature-x", "command": "info" }
{ "type": "command", "session_id": "my-feature-x", "command": "mcp", "args": ["list"] }
{ "type": "command", "session_id": "my-feature-x", "command": "model", "args": ["openrouter:anthropic/claude-sonnet-4"] }
{ "type": "command", "session_id": "my-feature-x", "command": "role", "args": ["assistant"] }
```

- `session_id` **required**
- `command` **required** — command name without the leading `/`
- `args` optional — array of string arguments
- Equivalent to typing `/command [args...]` in the CLI
- Server responds with `status` (or `status` + `meta` for structured output)

**Available commands** (same as CLI `/commands`):
`help`, `info`, `model`, `role`, `mcp`, `context`, `truncate`, `clear`, `loglevel`, `workflow`, `run`, `prompt`, `save`, `done`, `report`, `summarize`, `plan`, `list`

**Fields summary:**

| Field | Type | `session` | `message` | `command` |
|-------|------|-----------|-----------|-----------|
| `type` | string | required | required | required |
| `session_id` | string | optional | required | required |
| `content` | string | ignored | required | ignored |
| `command` | string | ignored | ignored | required |
| `args` | string[] | ignored | ignored | optional |


### Server → Client (Output)

All server messages are JSON objects tagged by `"type"`. Each variant carries only its own typed fields — no generic `content`/`meta` bag.

**Message Types:**

| Type | Description | Fields |
|------|-------------|--------|
| `assistant` | AI assistant response | `content`, `session_id` |
| `thinking` | AI thinking/reasoning content | `content`, `session_id` |
| `tool_use` | Tool execution intent (AI about to call a tool) | `tool`, `tool_id`, `server`, `params`, `session_id` |
| `tool_result` | Tool execution completed | `tool`, `tool_id`, `server`, `content`, `success`, `session_id` |
| `cost` | Token usage and cost summary | `session_tokens`, `session_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `session_id` |
| `status` | Status/info message | `message`, `session_id` (optional), `data` (optional, structured command output) |
| `error` | Error message | `message` |

**Examples:**

```json
{ "type": "assistant",   "content": "I'll help you fix that...", "session_id": "my-feature-x" }
{ "type": "thinking",    "content": "Let me reason through this...", "session_id": "my-feature-x" }
{ "type": "tool_use",    "tool": "list_files", "tool_id": "call_abc", "server": "filesystem", "params": {"directory": "src"}, "session_id": "my-feature-x" }
{ "type": "tool_result", "tool": "list_files", "tool_id": "call_abc", "server": "filesystem", "content": "src/main.rs\nsrc/lib.rs", "success": true, "session_id": "my-feature-x" }
{ "type": "cost",        "session_tokens": 1234, "session_cost": 0.0025, "input_tokens": 1000, "output_tokens": 200, "cache_read_tokens": 30, "cache_write_tokens": 4, "reasoning_tokens": 0, "session_id": "my-feature-x" }
{ "type": "status",      "message": "Session created: my-feature-x", "session_id": "my-feature-x" }
{ "type": "status",      "message": "Command '/info' executed successfully", "session_id": "my-feature-x", "data": { ... } }
{ "type": "error",       "message": "Session not found: nonexistent." }
```



## 📊 Message Flow Examples

### Example 1: Create a session

**Client sends:**
```json
{ "type": "session" }
```

**Server responds:**
```json
{ "type": "status", "message": "Session created: dev-20260218-120000-octomind", "session_id": "dev-20260218-120000-octomind" }
```

### Example 2: Create a named session (or resume if it exists)

**Client sends:**
```json
{ "type": "session", "session_id": "my-feature-x" }
```

**Server responds:**
```json
{ "type": "status", "message": "Session created: my-feature-x", "session_id": "my-feature-x" }
```
or if it already existed on disk:
```json
{ "type": "status", "message": "Session resumed: my-feature-x", "session_id": "my-feature-x" }
```

### Example 3: Send a user message

**Client sends:**
```json
{ "type": "message", "session_id": "my-feature-x", "content": "What files are in the src directory?" }
```

**Server responds (multiple messages):**
```json
{ "type": "tool_use",    "tool": "list_files", "tool_id": "call_abc", "server": "filesystem", "params": {"directory": "src"}, "session_id": "my-feature-x" }
{ "type": "tool_result", "tool": "list_files", "tool_id": "call_abc", "server": "filesystem", "content": "src/main.rs\nsrc/lib.rs\n...", "success": true, "session_id": "my-feature-x" }
{ "type": "assistant",   "content": "The src directory contains...", "session_id": "my-feature-x" }
{ "type": "cost",        "session_tokens": 1234, "session_cost": 0.0025, "input_tokens": 1000, "output_tokens": 200, "cache_read_tokens": 30, "cache_write_tokens": 4, "reasoning_tokens": 0, "session_id": "my-feature-x" }
```

### Example 4: Execute a command

**Client sends:**
```json
{ "type": "command", "session_id": "my-feature-x", "command": "info" }
```

**Server responds:**
```json
{ "type": "status", "message": "Command '/info' executed successfully", "session_id": "my-feature-x", "data": { ... } }
```

**Client sends:**
```json
{ "type": "command", "session_id": "my-feature-x", "command": "model", "args": ["openrouter:anthropic/claude-sonnet-4"] }
```

**Server responds:**
```json
{ "type": "status", "message": "Command '/model openrouter:anthropic/claude-sonnet-4' executed successfully", "session_id": "my-feature-x" }
```

### Example 5: Error — session not found

**Client sends:**
```json
{ "type": "message", "session_id": "nonexistent", "content": "Hello" }
```

**Server responds:**
```json
{ "type": "error", "message": "Session not found: nonexistent. Send a \"session\" message first to create or resume a session." }
```


## 🔧 Client Implementation Examples

### JavaScript/TypeScript

```javascript
class OctomindClient {
  constructor(url = 'ws://localhost:8080') {
    this.ws = new WebSocket(url);
    this.sessionId = null;

    this.ws.onopen = () => console.log('Connected to Octomind');
    this.ws.onmessage = (event) => this.handleMessage(JSON.parse(event.data));
    this.ws.onerror = (error) => console.error('WebSocket error:', error);
    this.ws.onclose = () => console.log('Disconnected from Octomind');
  }

  createSession(sessionId = null) {
    const msg = { type: 'session' };
    if (sessionId) msg.session_id = sessionId;
    this.ws.send(JSON.stringify(msg));
  }

  send(content) {
    this.ws.send(JSON.stringify({
      type: 'message',
      session_id: this.sessionId,
      content,
    }));
  }

  command(cmd, ...args) {
    this.ws.send(JSON.stringify({
      type: 'command',
      session_id: this.sessionId,
      command: cmd,
      ...(args.length && { args }),
    }));
  }

  handleMessage(msg) {
    // Store session ID from status messages
    if (msg.session_id && !this.sessionId) {
      this.sessionId = msg.session_id;
      console.log('Session ID:', this.sessionId);
    }

    switch (msg.type) {
      case 'assistant':
        console.log('\x1b[32m%s\x1b[0m', msg.content); // Green
        break;
      case 'thinking':
        console.log('\x1b[35m🤔 %s\x1b[0m', msg.content); // Magenta
        break;
      case 'tool_use':
        console.log('\x1b[36m🔧 %s | %s(%s)\x1b[0m', msg.server, msg.tool, JSON.stringify(msg.params)); // Cyan
        break;
      case 'tool_result':
        const icon = msg.success ? '✓' : '✗';
        console.log('\x1b[90m%s [%s] %s\x1b[0m', icon, msg.tool, msg.content); // Gray
        break;
      case 'cost':
        console.log('\x1b[33m💰 %d tokens ($%s)\x1b[0m', msg.session_tokens, msg.session_cost.toFixed(4)); // Yellow
        break;
      case 'error':
        console.log('\x1b[31m❌ %s\x1b[0m', msg.message); // Red
        break;
      case 'status':
        console.log('\x1b[34mℹ️  %s\x1b[0m', msg.message); // Blue
        break;
    }
  }

  close() {
    this.ws.close();
  }
}

// Usage
const client = new OctomindClient();
client.createSession();
// After receiving status with session_id:
// client.send('List files in src directory');
// client.command('info');
```


### Python

```python
import asyncio
import websockets
import json

class OctomindClient:
    def __init__(self, url='ws://localhost:8080'):
        self.url = url
        self.session_id = None

    async def connect(self):
        async with websockets.connect(self.url) as ws:
            self.ws = ws

            # Create a new session
            await ws.send(json.dumps({'type': 'session'}))

            # Wait for session confirmation
            welcome = json.loads(await ws.recv())
            self.handle_message(welcome)

            # Interactive loop
            while True:
                content = input('> ')
                if content == 'exit':
                    break
                await self.send(content)

                # Receive all responses until cost message (last in sequence)
                while True:
                    response = json.loads(await ws.recv())
                    self.handle_message(response)
                    if response['type'] == 'cost':
                        break

    async def send(self, content):
        await self.ws.send(json.dumps({
            'type': 'message',
            'session_id': self.session_id,
            'content': content,
        }))

    async def command(self, cmd, *args):
        msg = {'type': 'command', 'session_id': self.session_id, 'command': cmd}
        if args:
            msg['args'] = list(args)
        await self.ws.send(json.dumps(msg))

    def handle_message(self, msg):
        # Store session ID
        if msg.get('session_id') and not self.session_id:
            self.session_id = msg['session_id']
            print(f'Session ID: {self.session_id}')

        msg_type = msg['type']

        if msg_type == 'assistant':
            print(f'\033[32m{msg["content"]}\033[0m')  # Green
        elif msg_type == 'thinking':
            print(f'\033[35m🤔 {msg["content"]}\033[0m')  # Magenta
        elif msg_type == 'tool_use':
            print(f'\033[36m🔧 {msg["server"]} | {msg["tool"]}({msg["params"]})\033[0m')  # Cyan
        elif msg_type == 'tool_result':
            icon = '✓' if msg.get('success') else '✗'
            print(f'\033[90m{icon} [{msg["tool"]}] {msg["content"]}\033[0m')  # Gray
        elif msg_type == 'cost':
            print(f'\033[33m💰 {msg["session_tokens"]} tokens (${msg["session_cost"]:.4f})\033[0m')  # Yellow
        elif msg_type == 'error':
            print(f'\033[31m❌ {msg["message"]}\033[0m')  # Red
        elif msg_type == 'status':
            print(f'\033[34mℹ️  {msg["message"]}\033[0m')  # Blue

# Usage
client = OctomindClient()
asyncio.run(client.connect())
```


## 🔒 Security Considerations

### Current Implementation

The current implementation is designed for **local development** and **trusted networks**:

- ✅ No authentication (assumes trusted environment)
- ✅ No encryption (use reverse proxy for TLS)
- ✅ No rate limiting (single connection at a time)
- ✅ No CORS headers (WebSocket doesn't use CORS)

### Production Deployment

For production use, consider:

1. **Reverse Proxy with TLS**
   ```nginx
   server {
       listen 443 ssl;
       server_name octomind.example.com;

       ssl_certificate /path/to/cert.pem;
       ssl_certificate_key /path/to/key.pem;

       location / {
           proxy_pass http://localhost:8080;
           proxy_http_version 1.1;
           proxy_set_header Upgrade $http_upgrade;
           proxy_set_header Connection "upgrade";
           proxy_set_header Host $host;
       }
   }
   ```

2. **Authentication** (add to your reverse proxy or implement in Octomind)
   - API keys in headers
   - JWT tokens
   - OAuth2

3. **Rate Limiting** (at reverse proxy level)
   - Limit connections per IP
   - Limit messages per second

4. **Firewall Rules**
   - Restrict access to known IPs
   - Use VPN for remote access

## 🐛 Troubleshooting

### Connection Refused

```bash
# Check if server is running
ps aux | grep octomind

# Check if port is in use
lsof -i :8080

# Try different port
octomind server --port 3000
```

### Session Not Found

- Session IDs are only valid while server is running
- Sessions are stored in memory (not persisted across server restarts)
- Omit `session_id` to create a new session

### Message Too Large

- Default limit: 10MB per message
- Large outputs are automatically truncated
- Use pagination for large results

### Server Stops Responding

- Check server logs for errors
- Restart the server
- Check system resources (memory, CPU)

## 📚 Additional Resources

- [Session Commands](./05-sessions.md) - All available `/commands`
- [MCP Tools](./06-advanced.md#mcp-tools) - Available tools and their usage
- [Configuration](./03-configuration.md) - Server configuration options
- [Layers & Commands](./07-command-layers.md) - Custom commands and layers

## 🎯 Use Cases

### Remote Development

```javascript
// Connect from any machine
const client = new OctomindClient('ws://dev-server.local:8080');
client.send('Analyze the authentication flow');
```

### IDE Integration

```javascript
// VS Code extension
const octomind = new OctomindClient();
octomind.send(`Fix the error at ${filePath}:${lineNumber}`);
```

### CI/CD Integration

```bash
# Automated code review
echo "Review the changes in PR #123" | \
  websocat ws://localhost:8080
```

### Team Collaboration

```javascript
// Shared session for pair programming
const client = new OctomindClient();
client.send('Explain how the caching system works');
// Share session_id with teammate
```

## 🚀 Future Enhancements

Planned features for future releases:

- [ ] Multiple concurrent connections per session
- [ ] Streaming responses (real-time token-by-token)
- [ ] Built-in authentication
- [ ] Rate limiting
- [ ] Session management API (list, delete, export)
- [ ] Binary message support (for images)
- [ ] Metrics and monitoring endpoints

## 📝 Notes

- **Sequential Processing**: Messages are processed one at a time (like terminal)
- **Session Persistence**: ✅ Sessions are saved to disk (same as CLI)
- **Session Resumption**: ✅ Can resume sessions from disk or memory
- **WebSocket Compression**: ✅ Enabled (per-message deflate)
- **Session Isolation**: Each session is independent
- **Same Logic**: Uses identical session logic as CLI (feature parity guaranteed)
- **Working Directory**: Server runs from current directory (same as CLI)
