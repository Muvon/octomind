# Web Dashboard Integration

Embed Octomind's AI agent runtime into a web application through its real-time WebSocket server.

## The Problem

Your team wants an AI assistant accessible from a web dashboard — no terminal required. Developers should be able to ask questions about the codebase, request code reviews, and get help directly from a browser.

## Solution

Run the WebSocket server and connect from your web frontend.

### Step 1: Start the Server

```bash
octomind server --host 127.0.0.1 --port 8080
```

`--host` defaults to `127.0.0.1` and `--port` (short `-p`) defaults to `8080`, so a bare `octomind server` binds to `ws://127.0.0.1:8080`. The optional `TAG` positional selects a tap agent such as `assistant:concierge` or `developer:general`; omit it to use the root `default`. A plain name resolves only when explicitly defined under local `[[roles]]`; unknown local roles and missing tap tags fail session initialization.

```bash
octomind server assistant:concierge -p 8080
```

Because the dashboard connects from a browser, you must allowlist the page's origin — the server refuses any handshake carrying an unlisted `Origin` header:

```bash
octomind server assistant:concierge -p 8080 --allow-origin http://localhost:3000
```

Pass `--allow-origin` once per origin. See [Browser origins](../integration/01-websocket-server.md#browser-origins) for why this is not optional.

For production, bind to `0.0.0.0` behind a reverse proxy with TLS:

```bash
octomind server assistant:concierge --host 0.0.0.0 --port 8080 --allow-origin https://dashboard.example.com
```

> The server enforces the exact browser-origin allowlist, but it does not provide user authentication, authorization, or TLS. Any permitted client that can reach the socket can drive sessions under the process's shared credentials. Put non-local deployments behind a reverse proxy that supplies TLS and authentication.

### Step 2: Connect from JavaScript

```typescript
function askOctomind(url: string, question: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    const requestedSession = 'dev-session';
    let activeSession = '';
    let promptSent = false;
    const parts: string[] = [];

    ws.onopen = () => {
      ws.send(JSON.stringify({
        type: 'session',
        request_id: 'create-dev-session',
        session_id: requestedSession,
      }));
    };

    ws.onmessage = (event) => {
      const msg = JSON.parse(event.data);

      // Ignore the connection welcome and the immediate session ack. Send the
      // prompt only after session creation/resume returns the actual ID.
      if (msg.type === 'status' && msg.session_id && !promptSent) {
        activeSession = msg.session_id;
        promptSent = true;
        ws.send(JSON.stringify({
          type: 'message',
          request_id: 'question-1',
          session_id: activeSession,
          content: question,
        }));
      } else if (msg.type === 'assistant' && msg.session_id === activeSession) {
        parts.push(msg.content);
      } else if (msg.type === 'tool_use' && msg.session_id === activeSession) {
        console.log('tool:', msg.tool, msg.params);
      } else if (msg.type === 'cost' && msg.session_id === activeSession) {
        ws.close();
        resolve(parts.join(''));
      } else if (msg.type === 'error') {
        ws.close();
        reject(new Error(msg.message));
      }
    };

    ws.onerror = () => reject(new Error('WebSocket connection failed'));
  });
}

askOctomind('ws://127.0.0.1:8080', 'Explain how authentication works')
  .then(appendToChat)
  .catch(showError);
```

For command frames, use the bare command name without `/`, for example
`{"type":"command","session_id":"dev-session","command":"info"}`. Wait for
the preceding AI turn's `cost` frame before sending it, because overlapping work
for one session is rejected rather than queued.

### Step 3: Production Setup with nginx

```nginx
# /etc/nginx/sites-available/octomind
server {
    listen 443 ssl;
    server_name ai.yourcompany.com;

    ssl_certificate /etc/letsencrypt/live/ai.yourcompany.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/ai.yourcompany.com/privkey.pem;

    auth_basic "Octomind";
    auth_basic_user_file /etc/nginx/octomind.htpasswd;

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

        # Wait for the create/resume status. The connection welcome and ack do
        # not establish the session for subsequent application messages.
        while True:
            msg = json.loads(await ws.recv())
            if msg['type'] == 'error':
                raise RuntimeError(msg['message'])
            if msg['type'] == 'status' and msg.get('session_id'):
                session_id = msg['session_id']
                break

        await ws.send(json.dumps({
            'type': 'message',
            'session_id': session_id,
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
| Server -> Client | `ack` | Immediate receipt acknowledgement for each valid JSON text input (`message_type`, optional `request_id`, optional `session_id`, `status = "received"`). Malformed/invalid input returns `error` instead. |
| Server -> Client | `assistant` | AI response text (`content`) |
| Server -> Client | `thinking` | Extended thinking (`content`, if the model supports it) |
| Server -> Client | `tool_use` | Tool being called (`tool`, `tool_id`, `server`, `params`) |
| Server -> Client | `tool_result` | Tool execution result (`tool`, `tool_id`, `server`, `content`, `success`) |
| Server -> Client | `cost` | Token usage and cost; emitted once after each completed AI turn (use it as the end-of-turn signal) |
| Server -> Client | `status` | Free-form status text in `message` (e.g. the connection welcome, `Session created: <id>` / `Session resumed: <id>`, command-executed notices, `Session ended`, `Conversation compressed`). Command completion statuses carry `data`; AI turns still end with `cost`. May carry an optional `session_id`. |
| Server -> Client | `error` | Error text in `message`, with optional `request_id` when the failed input provided one |
| Server -> Client | `mcp_notification` | Notification forwarded from an MCP server (`server`, `method`, `params`, optional `tool_id`) |
| Server -> Client | `skill` | Skill lifecycle event (`action` = activate/use/forget, `name`, optional `trigger`) |
| Server -> Client | `evolution` | Grounded behavior lifecycle event (`action`, `id`, `name`, `kind`, `state`, `scope`) |
| Server -> Client | `injected` | Non-user input being added to the conversation (`source_kind` = schedule/background_agent/tap_run/skill/skill_validator/inject/webhook/guardrail_hook/guardrail_validator, `source_label`, `content`); emitted just before the AI responds so the UI can show what triggered it |

> For the authoritative, exhaustive wire-format spec (every field and JSON example) see [doc/integration/01-websocket-server.md](../integration/01-websocket-server.md). When the two docs differ, that reference and the source win.

## Multi-Session Support

Each `session_id` is independent. Multiple sessions under the same process identity can run concurrently:

```typescript
const alice = new OctomindClient(url, 'alice-session', handlers);
const bob = new OctomindClient(url, 'bob-session', handlers);

alice.send('Review the auth module');
bob.send('Help me write tests for the API');
// Both sessions run independently
```

Concurrency is across **different** `session_id`s. Requests to the **same** `session_id` are serialized by a per-session lock: if you send a second `message` or `command` while that session is still processing, the server replies immediately with an `error` payload (`Session '<id>' is busy processing another request. Please wait.`) — it does not queue the request. A dashboard sending overlapping input to one session must wait for the turn's `cost` message (or handle the busy error) before sending again.

## Key Points

- WebSocket sessions reuse the session and tool pipeline, with the protocol-specific command and lifecycle behavior documented above
- Sessions are stateful — context persists across messages
- Tool execution (file reading, shell commands) is streamed in real-time
- A `cost` message is emitted once after each completed AI turn — use it as the end-of-turn signal, not the free-form `status` text
- Every valid JSON input gets an immediate `ack`; use optional client `request_id` values if the UI needs correlation
- User `message` `content` is capped at 10 MB, and the WebSocket frame/message size limit is also 10 MB; larger input returns a validation error
- The server checks browser origins but has no built-in authentication, authorization, or TLS; use an authenticated TLS reverse proxy for non-local deployments
- Cost tracking is per-session via `cost` messages
