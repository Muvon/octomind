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
  // Send a message
  ws.send(JSON.stringify({
    id: 'msg_001',
    content: 'List files in the src directory'
  }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  console.log(`[${msg.type}]`, msg.content);
};
```

## 📋 Protocol Specification

### Message Format

All messages are JSON-encoded and follow a simple, CLI-aligned protocol.

### Client → Server (Input)

**Simple text-based input, just like terminal:**

```json
{
  "content": "Fix the authentication bug",
  "session_id": "sess_abc123"  // Optional: resume existing session
}
```

**Fields:**
- `content` (string, required): The actual input text (user message, command, anything)
  - Examples: `"Fix the bug"`, `"/help"`, `"/run analyze"`
- `session_id` (string, optional): Session ID for resuming existing sessions
  - If omitted, creates new session
  - If provided, resumes existing session

**Note:** Communication is sequential within a session, so `session_id` is sufficient for correlation. No separate message IDs needed.

**Commands:**
All CLI commands work via WebSocket. Just send them as content:
- `"/help"` - Show available commands
- `"/info"` - Display session info
- `"/model"` - Show current model
- `"/run <command>"` - Execute custom commands
- Any other `/command` from the CLI

### Server → Client (Output)

**Typed messages for different kinds of output:**

```json
{
  "id": "srv_001",
  "type": "assistant",
  "content": "I'll help you fix the authentication bug...",
  "meta": null,
  "session_id": "sess_abc123"
}
```

**Fields:**
- `id` (string): Unique message ID (server-generated, UUID)
- `type` (string): Message type (see below)
- `content` (string): The actual content (format depends on type)
- `meta` (object, optional): Structured metadata (varies by type)
- `session_id` (string, optional): Session ID (always present after first message)

**Note:** Since communication is sequential within a session, `session_id` is sufficient for correlation.

**Message Types:**

| Type | Description | Content | Metadata |
|------|-------------|---------|----------|
| `assistant` | AI assistant response | Response text | None |
| `tool_use` | Tool execution notification (AI intends to use tool) | Human-readable description | `{"tool": "name", "tool_id": "id", "server": "server_name", "params": {...}}` |
| `tool_result` | Tool execution completed | Tool output text | `{"tool": "name", "tool_id": "id", "server": "server_name", "success": bool, "duration_ms": number}` |
| `cost` | Cost and token usage | Human-readable summary | `{"session_tokens": number, "session_cost": number, ...}` |
| `error` | Error message | Error description | `{"error_type": "string", "recoverable": bool}` (optional) |
| `status` | Status/info message | Status text | Optional context |



## 📊 Message Flow Examples

### Example 1: Simple User Message

**Client sends:**
```json
{
  "content": "What files are in the src directory?"
}
```

**Server responds (multiple messages):**

```json
// 1. Tool execution notification
{
  "id": "srv_001",
  "type": "tool_use",
  "content": "Executing: list_files(...)",
  "meta": {
    "tool": "list_files",
    "tool_id": "call_abc123",
    "server": "filesystem",
    "params": {"directory": "src"}
  },
  "session_id": "sess_abc123"
}

// 2. Tool result
{
  "id": "srv_002",
  "type": "tool_result",
  "content": "src/main.rs\nsrc/lib.rs\nsrc/config/\n...",
  "meta": {
    "tool": "list_files",
    "tool_id": "call_abc123",
    "server": "filesystem",
    "success": true,
    "duration_ms": 45
  },
  "session_id": "sess_abc123"
}

// 3. Assistant response
{
  "id": "srv_003",
  "type": "assistant",
  "content": "The src directory contains:\n- main.rs (entry point)\n- lib.rs (library root)\n...",
  "session_id": "sess_abc123"
}

// 4. Cost information
{
  "id": "srv_004",
  "type": "cost",
  "content": "Session: 1,234 tokens ($0.0025)",
  "meta": {
    "session_tokens": 1234,
    "session_cost": 0.0025,
    "input_tokens": 800,
    "output_tokens": 434,
    "cached_tokens": 500
  },
  "session_id": "sess_abc123"
}
```

### Example 2: Command Execution

**Client sends:**
```json
{
  "content": "/help",
  "session_id": "sess_abc123"
}
```

**Server responds:**
```json
{
  "id": "srv_005",
  "type": "status",
  "content": "Available commands:\n/help - Show this help\n/info - Display session info\n...",
  "session_id": "sess_abc123"
}
```

### Example 3: Error Handling

**Client sends:**
```json
{
  "content": "Analyze the code",
  "session_id": "invalid_session"
}
```

**Server responds:**
```json
{
  "id": "srv_006",
  "type": "error",
  "content": "Session not found: invalid_session. Please start a new session by omitting session_id."
}
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

  send(content) {
    const message = {
      content,
      ...(this.sessionId && { session_id: this.sessionId })
    };
    this.ws.send(JSON.stringify(message));
  }

  handleMessage(msg) {
    // Store session ID from first message
    if (msg.session_id && !this.sessionId) {
      this.sessionId = msg.session_id;
      console.log('Session ID:', this.sessionId);
    }

    // Handle different message types
    switch (msg.type) {
      case 'assistant':
        console.log('\x1b[32m%s\x1b[0m', msg.content); // Green
        break;
      case 'tool_use':
        const toolInfo = msg.meta ? `${msg.meta.tool} | ${msg.meta.server}` : '';
        console.log('\x1b[36m🔧 %s [%s]\x1b[0m', msg.content, toolInfo); // Cyan
        break;
      case 'tool_result':
        const icon = msg.meta?.success ? '✓' : '✗';
        console.log('\x1b[90m%s %s\x1b[0m', icon, msg.content); // Gray
        break;
      case 'cost':
        console.log('\x1b[33m💰 %s\x1b[0m', msg.content); // Yellow
        break;
      case 'error':
        console.log('\x1b[31m❌ %s\x1b[0m', msg.content); // Red
        break;
      case 'status':
        console.log('\x1b[34mℹ️  %s\x1b[0m', msg.content); // Blue
        break;
    }
  }

  close() {
    this.ws.close();
  }
}

// Usage
const client = new OctomindClient();
client.send('List files in src directory');
client.send('/help');
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

            # Receive welcome message
            welcome = await ws.recv()
            self.handle_message(json.loads(welcome))

            # Interactive loop
            while True:
                content = input('> ')
                if content == 'exit':
                    break
                await self.send(content)

                # Receive all responses for this message
                while True:
                    response = await ws.recv()
                    msg = json.loads(response)
                    self.handle_message(msg)

                    # Stop after cost message (last message)
                    if msg['type'] == 'cost':
                        break

    async def send(self, content):
        message = {
            'content': content
        }
        if self.session_id:
            message['session_id'] = self.session_id

        await self.ws.send(json.dumps(message))

    def handle_message(self, msg):
        # Store session ID
        if msg.get('session_id') and not self.session_id:
            self.session_id = msg['session_id']
            print(f'Session ID: {self.session_id}')

        # Display message
        msg_type = msg['type']
        content = msg['content']

        if msg_type == 'assistant':
            print(f'\033[32m{content}\033[0m')  # Green
        elif msg_type == 'tool_use':
            meta = msg.get('meta', {})
            tool_info = f"{meta.get('tool', '')} | {meta.get('server', '')}"
            print(f'\033[36m🔧 {content} [{tool_info}]\033[0m')  # Cyan
        elif msg_type == 'tool_result':
            icon = '✓' if msg.get('meta', {}).get('success') else '✗'
            print(f'\033[90m{icon} {content}\033[0m')  # Gray
        elif msg_type == 'cost':
            print(f'\033[33m💰 {content}\033[0m')  # Yellow
        elif msg_type == 'error':
            print(f'\033[31m❌ {content}\033[0m')  # Red
        elif msg_type == 'status':
            print(f'\033[34mℹ️  {content}\033[0m')  # Blue

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
