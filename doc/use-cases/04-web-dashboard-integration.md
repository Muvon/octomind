# Use Case: Web Dashboard with AI Assistant

Embed Octomind into a web application as a real-time AI coding assistant using the WebSocket server.

## The Problem

Your team wants an AI assistant accessible from a web dashboard -- no terminal required. Developers should be able to ask questions about the codebase, request code reviews, and get help directly from a browser.

## Solution

Run the WebSocket server and connect from your web frontend.

### Step 1: Start the Server

```bash
octomind server --host 127.0.0.1 --port 8080
```

`--host` defaults to `127.0.0.1` and `--port` (short `-p`) defaults to `8080`, so a bare `octomind server` already binds to `ws://127.0.0.1:8080`. The optional `TAG` argument before the flags selects an agent (`role:tag`, e.g. `developer:general`) or a role name; omit it to use the default role from your config. The shipped config defines the roles `assistant`, `task_refiner`, `task_researcher`, and `reduce` -- if you pass a name that does not exist as a role and is not provided by a tap, the server falls back to the first configured role silently, so use a real role:

```bash
octomind server assistant -p 8080
```

For production, bind to `0.0.0.0` behind a reverse proxy with TLS:

```bash
octomind server assistant --host 0.0.0.0 --port 8080
```

> The server performs no authentication, authorization, TLS, or origin checking itself. Any client that can reach the socket can drive a session. Always put it behind a reverse proxy that handles TLS and auth (see Step 3), and never bind `0.0.0.0` on an untrusted network.

### Step 2: Connect from JavaScript

```typescript
class OctomindClient {
  private ws: WebSocket;
  private sessionId: string;
  private handlers: {
    onMessage: (content: string) => void;
    onToolUse: (tool: string, params: any) => void;
    onCost: (cost: number) => void;
    onError: (error: string) => void;
  };

  constructor(url: string, sessionId: string, handlers: typeof this.handlers) {
    this.sessionId = sessionId;
    this.handlers = handlers;
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      // Initialize session
      this.ws.send(JSON.stringify({
        type: 'session',
        session_id: this.sessionId
      }));
    };

    this.ws.onmessage = (event) => {
      const msg = JSON.parse(event.data);
      switch (msg.type) {
        case 'status':
          // The server replies to `session` with a status carrying the
          // ACTUAL session_id ("Session created: <id>" / "Session resumed: <id>").
          // When you omit session_id the server auto-names the session, so
          // always capture the returned id and use it for subsequent messages.
          if (msg.session_id) this.sessionId = msg.session_id;
          break;
        case 'assistant':
          this.handlers.onMessage(msg.content);
          break;
        case 'tool_use':
          this.handlers.onToolUse(msg.tool, msg.params);
          break;
        case 'cost':
          // `cost` is emitted once after each completed AI turn — the
          // canonical end-of-turn signal.
          this.handlers.onCost(msg.session_cost);
          break;
        case 'error':
          this.handlers.onError(msg.message);
          break;
      }
    };
  }

  send(message: string) {
    this.ws.send(JSON.stringify({
      type: 'message',
      session_id: this.sessionId,
      content: message
    }));
  }

  command(cmd: string) {
    // `command` is the bare command name WITHOUT the leading slash —
    // the server prepends '/'. Sending '/info' would become '//info'.
    // Unknown commands come back as an `error` payload.
    this.ws.send(JSON.stringify({
      type: 'command',
      session_id: this.sessionId,
      command: cmd
    }));
  }
}

// Usage
const ai = new OctomindClient('ws://localhost:8080', 'dev-session', {
  onMessage: (content) => appendToChat(content),
  onToolUse: (tool, params) => showToolActivity(tool),
  onCost: (cost) => updateCostDisplay(cost),
  onError: (error) => showError(error),
});

ai.send('Explain how authentication works in this project');
ai.command('info');  // Get session stats (note: bare name, no leading slash)
```

### Step 3: Production Setup with nginx

```nginx
# /etc/nginx/sites-available/octomind
server {
    listen 443 ssl;
    server_name ai.yourcompany.com;

    ssl_certificate /etc/letsencrypt/live/ai.yourcompany.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/ai.yourcompany.com/privkey.pem;

    # WebSocket proxy
    location /ws {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }

    # Static frontend
    location / {
        root /var/www/dashboard;
        try_files $uri /index.html;
    }
}
```

### Python Client

```python
import asyncio
import json
import websockets

async def ask_octomind(question: str) -> str:
    async with websockets.connect('ws://127.0.0.1:8080') as ws:
        await ws.send(json.dumps({
            'type': 'session',
            'session_id': 'api-session'
        }))

        await ws.send(json.dumps({
            'type': 'message',
            'session_id': 'api-session',
            'content': question
        }))

        response_parts = []
        async for message in ws:
            msg = json.loads(message)
            if msg['type'] == 'assistant':
                response_parts.append(msg['content'])
            elif msg['type'] == 'cost':
                # `cost` is emitted once after each completed AI turn — the
                # canonical end-of-turn marker. `status` text is free-form and
                # never reliably signals completion, so don't parse it for that.
                break

        return ''.join(response_parts)

# Usage
answer = asyncio.run(ask_octomind("What does the login function do?"))
```

## Protocol Messages

| Direction | Type | Purpose |
|-----------|------|---------|
| Client -> Server | `session` | Create or resume a session (no AI call). With no `session_id` the server creates an auto-named session; with a `session_id` it resumes that session if it exists on disk, otherwise creates one with that name. |
| Client -> Server | `message` | Send user input (field `content`, max 10 MB) |
| Client -> Server | `command` | Execute a session command (field `command`, bare name without the leading `/`; optional `args` array) |
| Server -> Client | `assistant` | AI response text (`content`) |
| Server -> Client | `thinking` | Extended thinking (`content`, if the model supports it) |
| Server -> Client | `tool_use` | Tool being called (`tool`, `tool_id`, `server`, `params`) |
| Server -> Client | `tool_result` | Tool execution result (`tool`, `tool_id`, `server`, `content`, `success`) |
| Server -> Client | `cost` | Token usage and cost; emitted once after each completed AI turn (use it as the end-of-turn signal) |
| Server -> Client | `status` | Free-form status text in `message` (e.g. the connection welcome, `Session created: <id>` / `Session resumed: <id>`, command-executed notices, `Session ended`, `Conversation compressed`). Not a machine-readable completion marker -- use `cost` for that. May carry an optional `session_id` and structured `data`. |
| Server -> Client | `error` | Error text in `message` |
| Server -> Client | `mcp_notification` | Notification forwarded from an MCP server (`server`, `method`, `params`) |
| Server -> Client | `skill` | Skill lifecycle event (`action` = activate/use/forget, `name`, optional `trigger`) |
| Server -> Client | `injected` | Non-user input being added to the conversation (`source_kind` = schedule/background_agent/tap_run/skill/skill_validator/inject/webhook/guardrail_hook/guardrail_validator, `source_label`, `content`); emitted just before the AI responds so the UI can show what triggered it |

> For the authoritative, exhaustive wire-format spec (every field and JSON example) see [doc/integration/01-websocket-server.md](../integration/01-websocket-server.md). When the two docs differ, that reference and the source win.

## Multi-Session Support

Each `session_id` is independent. Multiple users can have concurrent sessions:

```typescript
const alice = new OctomindClient(url, 'alice-session', handlers);
const bob = new OctomindClient(url, 'bob-session', handlers);

alice.send('Review the auth module');
bob.send('Help me write tests for the API');
// Both sessions run independently
```

Concurrency is across **different** `session_id`s. Requests to the **same** `session_id` are serialized by a per-session lock: if you send a second `message` or `command` while that session is still processing, the server replies immediately with an `error` payload (`Session '<id>' is busy processing another request. Please wait.`) -- it does not queue the request. A dashboard sending overlapping input to one session must wait for the turn's `cost` message (or handle the busy error) before sending again.

## Key Points

- The WebSocket server provides the same capabilities as the CLI
- Sessions are stateful -- context persists across messages
- Tool execution (file reading, shell commands) is streamed in real-time
- A `cost` message is emitted once after each completed AI turn -- use it as the end-of-turn signal, not the free-form `status` text
- User `message` `content` is capped at 10 MB, and the WebSocket frame/message size limit is also 10 MB; larger input returns a validation error
- The server has no built-in authentication, authorization, or TLS -- use a reverse proxy with TLS for production and never expose the server directly to the internet
- Cost tracking is per-session via `cost` messages
