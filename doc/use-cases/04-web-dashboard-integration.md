# Web Dashboard Integration

Use this guide to connect a browser dashboard to Octomind's WebSocket server. It covers session setup, request
completion, media references, and deployment boundaries for frontend developers.

## Start the server

Run from the checkout the assistant should work on, with provider credentials configured.

```bash
octomind server --host 127.0.0.1 --port 8080
```

`--host` defaults to `127.0.0.1` and `--port` (short `-p`) defaults to `8080`, so a bare `octomind server` binds to
`ws://127.0.0.1:8080`. The optional `TAG` positional selects a tap agent such as `assistant:concierge` or
`developer:general`; omit it to use the root `default`. A plain name resolves only when explicitly defined under local
`[[roles]]`; unknown local roles and missing tap tags fail resolution or session initialization.

```bash
octomind server assistant:concierge -p 8080
```

Because the dashboard connects from a browser, you must allowlist the page's origin — the server refuses any handshake
carrying an unlisted `Origin` header:

```bash
octomind server assistant:concierge -p 8080 --allow-origin http://localhost:3000
```

Pass `--allow-origin` once per origin. See [Browser origins](../integration/01-websocket-server.md#browser-origins) for
why this is not optional.

For a same-host production proxy serving `https://ai.yourcompany.com`, keep the backend on loopback:

```bash
octomind server assistant:concierge --host 127.0.0.1 --port 8080 --allow-origin https://ai.yourcompany.com
```

> The server enforces the exact browser-origin allowlist, but it does not provide user authentication, authorization, or
> TLS. Any permitted client that can reach the socket can drive sessions under the process's shared credentials. Put
> non-local deployments behind a reverse proxy that supplies TLS and authentication.

## Configure a production proxy

This nginx example assumes your certificate, password file, and frontend build already exist at the shown paths. Replace
the domain and paths with your deployment values. Serve the page from the same origin allowed above; the browser
connects to `wss://ai.yourcompany.com/ws`.

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

## Connect from JavaScript

Run this in a page served from the allowed origin. Each call creates a fresh session unless you pass a saved name. It
handles one foreground request per connection; keep a long-lived event consumer for sessions that schedule or delegate
background work. Closing a socket is not a protocol cancellation request.

```javascript
function askOctomind(url, question, requestedSession) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    let activeSession = '';
    let promptSent = false;
    let settled = false;
    const parts = [];
    const timer = setTimeout(() => finish(new Error('Timed out waiting for the server')), 600000);

    function finish(error) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      ws.close();
      if (error) reject(error);
      else resolve({ sessionId: activeSession, answer: parts.join('\n') });
    }

    ws.onopen = () => ws.send(JSON.stringify({
      type: 'session', request_id: 'session-1', session_id: requestedSession,
    }));

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === 'error') {
          finish(new Error(msg.message));
          return;
        }
        // The welcome has no session_id; ack is receipt, not completed setup.
        if (msg.type === 'status' && msg.session_id && !msg.data && !promptSent) {
          activeSession = msg.session_id;
          promptSent = true;
          ws.send(JSON.stringify({
            type: 'message', request_id: 'question-1',
            session_id: activeSession, content: question,
          }));
        } else if (msg.session_id === activeSession) {
          if (msg.type === 'assistant') parts.push(msg.content);
          if (msg.type === 'tool_use') console.log('tool:', msg.tool, msg.params);
          if (msg.type === 'injected') console.log('inbox:', msg.source_label, msg.content);
          if (msg.type === 'cost' && promptSent) finish();
        }
      } catch (error) {
        finish(error);
      }
    };
    ws.onerror = () => finish(new Error('WebSocket connection failed'));
    ws.onclose = () => finish(new Error('Connection closed before completion'));
  });
}

askOctomind('ws://127.0.0.1:8080', 'Explain how authentication works')
  .then(console.log)
  .catch(console.error);
```

## Execute session commands

On an established connection, send a bare command name without `/`. For a session established as `dev-session`:

```json
{"type":"command","request_id":"info-1","session_id":"dev-session","command":"info","args":[]}
```

Wait for a `status` carrying `data` to complete a command such as `info`; commands do not generally emit `cost`. A
normal successful foreground message ends with `cost`; on failure handle `error` without waiting for cost. `request_id`
appears on acknowledgements and some errors, but not on assistant, cost, or command-status events. Track one outstanding
operation per connection and do not assume every error has a correlation ID.

## Send media references

A `session` acknowledgement advertises `message_attachments_v1`. Upload media through your own backend before sending
one atomic `message` containing text and attachment references; this server has no upload endpoint. The media root is
`OCTOMIND_MEDIA_ROOT` (default `/home/octo/.octomind/media`), independent of `OCTOMIND_DATA_DIR`.

For a local image fixture, use an existing `screenshot.png` (at most 5 MiB). Stop the earlier server and restart it with
the same media root:

```bash
mkdir -p "$HOME/.octomind-media"
cp screenshot.png "$HOME/.octomind-media/ABCDEFGHIJKLMNOPQRSTUVWX.png"
wc -c < screenshot.png
OCTOMIND_MEDIA_ROOT="$HOME/.octomind-media" \
  octomind server --allow-origin http://localhost:3000
```

After establishing `dev-session`, send this frame, replacing `size` with the file's byte count:

```json
{
  "type": "message",
  "request_id": "image-1",
  "session_id": "dev-session",
  "content": "Explain this screenshot.",
  "attachments": [{
    "id": "ABCDEFGHIJKLMNOPQRSTUVWX",
    "kind": "image",
    "media_type": "image/png",
    "name": "screenshot.png",
    "size": 1024
  }]
}
```

The ID must be exactly 24 ASCII letters/digits and match exactly one regular, non-symlink file named `ID.extension`. The
model must support vision for images or video for videos. Audio references are checked for readability but are not
forwarded to the model. See the [attachment contract](../integration/01-websocket-server.md#client-to-server) for full
payload details.

## Operate multiple sessions

Use separate connections for concurrent foreground work; the connection loop awaits each request before reading the
next. The helper above opens one connection per call and omits the session name to get distinct sessions:

```javascript
Promise.all([
  askOctomind('ws://127.0.0.1:8080', 'Review the auth module'),
  askOctomind('ws://127.0.0.1:8080', 'Suggest tests for the API'),
]).then(console.log).catch(console.error);
```

Across connections, a message or command that finds its session locked returns a busy error. Wait for completion before
another operation on the same session. Disconnecting does not delete session history; pass a returned `sessionId` as the
helper's third argument to resume it. Session IDs are not user authorization boundaries: all clients share the server
process's credentials, filesystem access, and configured tools.

## Common questions

- **Why does the browser handshake return 403?** Allow the page's exact origin, including its scheme and port. Native
  clients without an `Origin` header bypass this check; an allowlist does not authenticate them.
- **Why did an ack arrive before a failure?** Ack means a parsed, validated input was received, not that its session
  exists, it acquired a lock, or execution succeeded. Invalid JSON or semantic validation errors get no ack.
- **Why is a reply incomplete?** Handle `error`, early socket close, and a client timeout. Assistant events contain
  response blocks, not guaranteed token deltas. Keep consuming events for sessions with background producers.
- **Why did a large payload disconnect?** Both frame/message limits and content validation use 10 MiB. The serialized
  envelope counts toward the transport limit; oversized frames can close the connection without an application error.

## Protocol Messages

| Direction | Type | Purpose |
|-----------|------|---------|
| Client -> Server | `session` | Create or resume a session (no AI call). With no `session_id` the server creates an auto-named session; with a `session_id` it resumes that session if it exists on disk, otherwise creates one with that name. |
| Client -> Server | `message` | Send user input (field `content`, max 10 MiB; optional `attachments`) |
| Client -> Server | `command` | Execute a session command (field `command`, bare name without the leading `/`; optional `args` array) |
| Server -> Client | `ack` | Receipt acknowledgement when a parsed, validated client input is read (`message_type`, optional `request_id`, optional `session_id`, `status = "received"`; session acks also advertise `capabilities`). Malformed/invalid input returns `error` instead. |
| Server -> Client | `assistant` | AI response text (`content`) |
| Server -> Client | `thinking` | Extended thinking (`content`, if the model supports it) |
| Server -> Client | `tool_use` | Tool being called (`tool`, `tool_id`, `server`, `params`) |
| Server -> Client | `tool_result` | Tool execution result (`tool`, `tool_id`, `server`, `content`, `success`) |
| Server -> Client | `cost` | Cumulative session usage/cost; completes a successful foreground message, also emitted by background processing |
| Server -> Client | `status` | Free-form status text in `message` (e.g. the connection welcome, `Session created: <id>` / `Session resumed: <id>`, command-executed notices, `Session ended`, `Conversation compressed`). Command completion statuses carry `data`; successful foreground messages end with `cost`. May carry an optional `session_id`. |
| Server -> Client | `error` | Error text in `message`, with `request_id` on some failure paths; it is not guaranteed |
| Server -> Client | `mcp_notification` | Notification forwarded from an MCP server (`server`, `method`, `params`, optional `tool_id`) |
| Server -> Client | `skill` | Skill lifecycle event (`action` = activate/use/forget, `name`, optional `trigger`) |
| Server -> Client | `evolution` | Grounded behavior lifecycle event (`action`, `id`, `name`, `kind`, `state`, `scope`) |
| Server -> Client | `injected` | Inbox input being added to the conversation (`source_kind` = schedule/monitor/background_agent/background_job/tap_run/skill/skill_validator/inject/webhook/guardrail_hook/guardrail_validator, `source_label`, `content`); emitted just before the AI responds so the UI can show what triggered it |

For exhaustive fields, see the [WebSocket reference](../integration/01-websocket-server.md).

## See also

- [WebSocket server reference](../integration/01-websocket-server.md)
- [Session commands](../reference/02-session-commands.md)
- [Event-driven webhooks](02-event-driven-agent.md)
