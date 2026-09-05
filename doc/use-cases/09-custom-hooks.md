# Custom Hooks

Use script-backed HTTP listeners to turn external events into messages for a running Octomind session. This guide is for
users building custom integrations and covers setup, executable scripts, request metadata, and troubleshooting.

> **`[[hooks]]` vs `[[hook]]` — two different systems.** This doc covers **HTTP webhook hooks** (`[[hooks]]`, plural, in
> the main config): an external POST request runs your script, and on **exit 0 with non-empty stdout** the output is
> injected into the session. That is unrelated to the guardrail `[[hook]]` (singular, in `.agents/guardrails.toml`),
> which runs after a tool result and has the **inverted** rule — a **non-zero exit with non-empty stdout** injects its
> stdout. If you want post-tool-result policy hooks, see [Guardrails](../usage/18-guardrails.md). This page is only
> about the webhook listeners.

## Get Started

The shell examples below assume a Unix-like system and an installed, configured Octomind. Create a minimal executable
script in your project:

```bash
mkdir -p hooks
cat > hooks/my-hook.sh <<'SH'
#!/bin/sh
set -eu
body=$(cat)
[ -n "$body" ] || exit 0
printf 'External event from %s:\n%s\nPlease summarize this event.\n' "$HOOK_NAME" "$body"
SH
chmod +x hooks/my-hook.sh
```

Add this to `hooks.toml` in Octomind's config directory. On macOS/Linux the default directory is
`~/.local/share/octomind/config`; on Windows it is `%LOCALAPPDATA%/octomind/config`. With `OCTOMIND_DATA_DIR` set, use
its `config` subdirectory. If `OCTOMIND_CONFIG_PATH` is set, put `hooks.toml` beside that selected config file.

```toml
[[hooks]]
name = "my-hook"
bind = "127.0.0.1:9876"
script = "./hooks/my-hook.sh"
timeout = 30
```

Start from the project directory containing `hooks/`. Relative script paths resolve from the Octomind process's working
directory; use an absolute script path when a service starts elsewhere.

```bash
printf '%s\n' 'Standby for webhook events.' | \
  octomind run --name my-agent --daemon --format jsonl --hook my-hook
```

From another terminal, post an event:

```bash
curl -i -X POST http://127.0.0.1:9876/events \
  -H 'Content-Type: text/plain' \
  --data-binary 'Build 42 passed.'
```

Expect HTTP `200` with body `ok`. This acknowledges that the script's output was queued, not that the AI has finished
processing it. Watch the running session for the response.

## Configure and Activate Hooks

A hook is an HTTP listener, an executable script, and a session inbox. Each request follows this path:

```text
External System (HTTP POST only)
    |
    v
Hook HTTP Listener (bind address:port)
    |
    | passes raw HTTP body to stdin
    | passes headers as HOOK_HEADER_* env vars
    | passes method, path, query as env vars
    v
Your Script (any language, any logic)
    |
    | exit 0 + non-empty stdout → inject message into AI session  (HTTP 200 "ok")
    | exit 0 + empty stdout      → skipped                        (HTTP 204)
    | exit non-zero              → not injected; stderr logged at error level (HTTP 500)
    v
AI Agent Session (processes message with its configured role and tool grants)
```

You control everything between the HTTP request and what the AI sees.

**The listener accepts POST only.** A GET, PUT, or any other method returns `405 Method Not Allowed` and your script is
never run. The script cannot set the response body: stdout goes to the session inbox. If your event source requires a
challenge response, handle it in an adapter or proxy before forwarding events to Octomind.

### Configuration Fields

| Field | Required/default | Meaning |
|---|---|---|
| `name` | Required | Unique identifier selected by `--hook` |
| `bind` | Required | Numeric IP address and port, such as `127.0.0.1:9876`; not a hostname |
| `script` | Required | Executable file path; invoked directly, without a shell command parser |
| `timeout` | `30` seconds | Script execution limit; valid range `1`–`3600` |

Config loading validates non-empty fields, unique hook names and bind strings, address syntax, and timeout range across
all configured hooks. Before each selected hook binds, startup checks that its script exists and is a regular file; Unix
also requires an execute bit. Unselected hooks do not undergo this filesystem check.

Activate hooks with `octomind run --hook NAME`; repeat `--hook` to select several. The ACP CLI parses the same flag, but
its session paths do not start webhook listeners. Use `run` for HTTP webhook delivery.

### Keep the Listener Alive

Hook listeners start for both interactive and non-interactive `octomind run` sessions and stop when the session ends. A
normal non-interactive run exits once it has no scheduled entries or background work left. Use `--daemon` to keep it
alive between webhook requests; after the process exits, the listener is gone and new HTTP connections fail rather than
reaching a session. Use `--daemon` for an unattended hook-driven agent.

When stdin is a terminal, a daemon may start with empty input and idle waiting for hooks. With **no** TTY and no piped
stdin, the run instead fails immediately with `No input provided via stdin`. So pipe an initial message in:

```bash
printf '%s\n' 'Standby for webhook events.' | \
  octomind run --name my-agent --daemon --format jsonl --hook my-hook
```

## Observe Incoming Events

In `--format jsonl` mode, each dequeued hook message emits an `injected` record before model processing. For the
quick-start request, shown formatted for readability (the actual JSONL record occupies one line):

```json
{
  "type": "injected",
  "source_kind": "webhook",
  "source_label": "webhook my-hook",
  "content": "External event from my-hook:\nBuild 42 passed.\nPlease summarize this event.",
  "session_id": "my-agent"
}
```

Multiple pending inbox messages may be processed together in one AI turn. Downstream consumers can match
`"type":"injected"` with `"source_kind":"webhook"` to tell hook-driven turns apart from user turns.

## Write an Event Adapter

These scripts define example input shapes, shown by the local POST commands below. Adapt your service's payload to those
shapes; Octomind supplies the raw body and metadata without interpreting service-specific events. Save each script under
the existing `hooks/` directory, then use the multi-hook configuration below to activate it.

### Python: Issue Tracker Adapter

Save this executable as `./hooks/jira.py`.

```python
#!/usr/bin/env python3
"""Process the issue-event input shape documented below."""
import json
import os
import sys

payload = json.load(sys.stdin)
event = os.environ.get("HOOK_HEADER_X_ISSUE_EVENT", "")

if event == "issue_created":
    issue = payload["issue"]
    key = issue["key"]
    summary = issue["fields"]["summary"]
    description = issue["fields"].get("description", "No description")
    priority = (issue["fields"].get("priority") or {}).get("name", "Unspecified")
    assignee = (issue["fields"].get("assignee") or {}).get("displayName", "Unassigned")

    print(f"""New issue {key} ({priority}): {summary}
Assigned to: {assignee}

Description:
{description}

Please:
1. Analyze if this issue relates to any recent code changes
2. Identify the relevant source files
3. Suggest an implementation approach if it's a feature, or root cause if it's a bug""")

elif event == "issue_updated":
    changelog = payload.get("changelog", {}).get("items", [])
    status_change = next((c for c in changelog if c["field"] == "status"), None)
    if status_change and status_change["toString"] == "In Review":
        key = payload["issue"]["key"]
        print(f"Issue {key} moved to In Review. Please review the associated code changes.")
    else:
        sys.exit(0)  # Ignore other updates (empty stdout -> HTTP 204)
else:
    sys.exit(0)  # Ignore unknown events (empty stdout -> HTTP 204)
```

### Node.js: Chat Mention Adapter

Save this executable as `./hooks/slack.js`.

```javascript
#!/usr/bin/env node
const payload = JSON.parse(require('fs').readFileSync(0, 'utf8'));

// Only react to app mentions
if (payload.event?.type !== 'app_mention') {
  process.exit(0);
}

const user = payload.event.user;
const text = payload.event.text.replace(/<@[A-Z0-9]+>/g, '').trim();
const channel = payload.event.channel;

console.log(`Chat request from <@${user}> in #${channel}:

${text}

Summarize the request concisely. Do not send a reply to the chat service.`);
```

### Bash: Push Event Adapter

Save this executable as `./hooks/github-push.sh` for the multi-hook configuration below.

```bash
#!/bin/bash
set -euo pipefail
# Requires jq. Minimal hook: extract essentials, let the AI figure out the rest

payload=$(cat)
branch=$(printf '%s' "$payload" | jq -r '.ref | sub("^refs/heads/"; "")')

# Only care about main and develop
case "$branch" in
  main|develop) ;;
  *) exit 0 ;;
esac

commits=$(printf '%s' "$payload" | jq -r '.commits[] | "- \(.message) (\(.author.name))"')
files=$(printf '%s' "$payload" | jq -r '.commits[] | (.added[]?, .modified[]?, .removed[]?)' | sort -u)

echo "Push to $branch:
$commits

Files changed:
$files

Review these changes for issues."
```

### Ruby: Custom Monitoring Alert

Save this executable as `./hooks/alerts.rb`.

```ruby
#!/usr/bin/env ruby
require 'json'

payload = JSON.parse($stdin.read)
severity = payload['severity']
service = payload['service']
message = payload['message']
metrics = payload['metrics'] || {}

# Only alert on warning and critical
exit 0 unless %w[warning critical].include?(severity)

puts <<~MSG
  #{severity.upcase} alert from #{service}: #{message}

  Metrics: #{metrics.map { |k, v| "#{k}=#{v}" }.join(', ')}

  Please:
  1. Check the #{service} source code for potential causes
  2. Look at recent changes that might have caused this
  3. Suggest immediate mitigation steps
MSG
```

### Go: Compiled Deployment Adapter

Save the following as `hooks/deploy.go`. Go source cannot contain a shebang; compile it and configure the resulting
executable as the hook's `script`:

```bash
go build -o hooks/deploy hooks/deploy.go
```

```go
package main

import (
    "encoding/json"
    "fmt"
    "io"
    "os"
)

type DeployEvent struct {
    Environment string `json:"environment"`
    Version     string `json:"version"`
    Status      string `json:"status"`
    Services    []struct {
        Name   string `json:"name"`
        Health string `json:"health"`
    } `json:"services"`
}

func main() {
    data, err := io.ReadAll(os.Stdin)
    if err != nil {
        fmt.Fprintln(os.Stderr, err)
        os.Exit(1)
    }
    var event DeployEvent
    if err := json.Unmarshal(data, &event); err != nil {
        fmt.Fprintln(os.Stderr, err)
        os.Exit(1)
    }

    if event.Status != "completed" {
        return // Ignore incomplete deployments: empty stdout produces HTTP 204.
    }

    unhealthy := []string{}
    for _, s := range event.Services {
        if s.Health != "healthy" {
            unhealthy = append(unhealthy, s.Name)
        }
    }

    if len(unhealthy) > 0 {
        fmt.Printf("Deploy %s to %s completed but %d services unhealthy: %v\n",
            event.Version, event.Environment, len(unhealthy), unhealthy)
        fmt.Println("\nInvestigate the unhealthy services and suggest fixes.")
    } else {
        fmt.Printf("Deploy %s to %s successful. All %d services healthy.\n",
            event.Version, event.Environment, len(event.Services))
        fmt.Println("\nRun a quick smoke test on the key API endpoints.")
    }
}
```

## Run Multiple Hooks

The Python, Node.js, Bash, and Ruby examples require their respective interpreters; the Bash adapter also needs `jq`.
Mark the scripts executable before starting:

```bash
chmod +x hooks/jira.py hooks/slack.js hooks/github-push.sh hooks/alerts.rb
```

Add these entries to your config-directory `hooks.toml`. Each script path is relative to the launch directory:

```toml
[[hooks]]
name = "github"
bind = "127.0.0.1:9001"
script = "./hooks/github-push.sh"
timeout = 30

[[hooks]]
name = "jira"
bind = "127.0.0.1:9002"
script = "./hooks/jira.py"
timeout = 30

[[hooks]]
name = "monitoring"
bind = "127.0.0.1:9003"
script = "./hooks/alerts.rb"
timeout = 15

[[hooks]]
name = "slack"
bind = "127.0.0.1:9004"
script = "./hooks/slack.js"
timeout = 10
```

```bash
printf '%s\n' 'Standby for adapter events.' | \
  octomind run --name ops-agent --daemon --format jsonl \
  --hook github \
  --hook jira \
  --hook monitoring \
  --hook slack
```

One AI agent, four event sources, each with its own script in its own language. Each hook binds a distinct port (the
`bind` addresses must be unique). Events enter the same conversation, subject to its normal context and compression
limits. Scripts may run concurrently; do not depend on HTTP arrival order to serialize side effects.

### Test Each Adapter

Send a POST to the bind address and watch the session's JSONL stream for the injected turn:

```bash
curl -X POST http://127.0.0.1:9001/ \
  -H 'Content-Type: application/json' \
  -d '{"ref":"refs/heads/main","commits":[{"message":"smoke test","author":{"name":"CI"},"modified":["README.md"]}]}'
```

Use matching fixtures for the other three adapters:

```bash
curl -i -X POST http://127.0.0.1:9002/ \
  -H 'Content-Type: application/json' -H 'X-Issue-Event: issue_created' \
  -d '{"issue":{"key":"APP-42","fields":{"summary":"Login fails","description":"Login returns 500","assignee":null}}}'
curl -i -X POST http://127.0.0.1:9004/ \
  -H 'Content-Type: application/json' \
  -d '{"event":{"type":"app_mention","user":"U42","channel":"C42","text":"Review the latest changes"}}'
curl -i -X POST http://127.0.0.1:9003/ \
  -H 'Content-Type: application/json' \
  -d '{"severity":"warning","service":"api","message":"Latency increased","metrics":{"p95_ms":800}}'
```

To use the compiled Go adapter, add its configuration and activate it:

```toml
[[hooks]]
name = "deploy"
bind = "127.0.0.1:9005"
script = "./hooks/deploy"
timeout = 30
```

```bash
printf '%s\n' 'Standby for deployment events.' | \
  octomind run --name deploy-agent --daemon --format jsonl --hook deploy
```

```bash
curl -i -X POST http://127.0.0.1:9005/ \
  -H 'Content-Type: application/json' \
  -d '{"environment":"staging","version":"1.2.3","status":"completed","services":[{"name":"api","health":"unhealthy"}]}'
```

A `200` with body `ok` means the script injected a message; a `204` means it exited 0 with empty stdout (filtered out);
a `500` means the script exited non-zero or execution failed (check the error logs). In the agent's `--format jsonl`
output you should see an `injected` record with `source_kind` equal to `webhook`.

## Script Design Patterns

### Filter Early and Print a Complete Request

Exit zero without stdout to ignore an event cleanly. Send diagnostic errors to stderr, and reserve stdout for the
message the AI should process. For a script that only accepts push events, place this before processing its body:

```bash
[ "${HOOK_HEADER_X_GITHUB_EVENT:-}" = "push" ] || exit 0
```

The adapters above include the event identity, relevant details, and a concrete request. Printing a chat event does not
send a reply to that chat service; configure the necessary tools and explicitly request delivery if you need it.

### Validate a Signature Before Printing

The listener does not authenticate requests itself. For an adapter whose contract uses a SHA-256 HMAC in
`X-Hub-Signature-256`, this complete Python script fails when the secret is absent or the signature is invalid. It reads
the body once, verifies those exact bytes, then parses them:

```python
#!/usr/bin/env python3
import hashlib
import hmac
import json
import os
import sys

secret = os.environ.get("WEBHOOK_SECRET")
if not secret:
    sys.exit("WEBHOOK_SECRET is required")
body = sys.stdin.buffer.read()
signature = os.environ.get("HOOK_HEADER_X_HUB_SIGNATURE_256", "")
expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
if not hmac.compare_digest(signature, expected):
    sys.exit("Invalid webhook signature")
payload = json.loads(body)
print(f"Verified event: {payload['message']}\nPlease summarize it.")
```

Save it as `hooks/signed.py`, make it executable, and supply your shared `WEBHOOK_SECRET` to the Octomind process.
`WEBHOOK_SECRET` is defined by this script, not an Octomind config field. For a local test, both terminals can use the
explicit test secret below:

```bash
chmod +x hooks/signed.py
export WEBHOOK_SECRET='local-test-only'
```

```toml
[[hooks]]
name = "signed"
bind = "127.0.0.1:9006"
script = "./hooks/signed.py"
timeout = 30
```

```bash
printf '%s\n' 'Standby for signed events.' | \
  octomind run --name signed-agent --daemon --format jsonl --hook signed
```

In the sending terminal:

```bash
export WEBHOOK_SECRET='local-test-only'
body='{"message":"Build 42 passed"}'
signature=$(printf '%s' "$body" | python3 -c \
  'import hashlib,hmac,os,sys; print("sha256="+hmac.new(os.environ["WEBHOOK_SECRET"].encode(),sys.stdin.buffer.read(),hashlib.sha256).hexdigest())')
curl -i -X POST http://127.0.0.1:9006/ \
  -H 'Content-Type: application/json' \
  -H "X-Hub-Signature-256: $signature" --data-binary "$body"
```

### Allow More Script Processing Time

For the compiled deployment adapter, increase its existing entry's timeout if needed:

```toml
[[hooks]]
name = "deploy"
bind = "127.0.0.1:9005"
script = "./hooks/deploy"
timeout = 120
```

Replace the earlier `deploy` entry; do not append a duplicate. The timeout covers writing script stdin and waiting for
output, not the AI's response or the preceding HTTP body upload. On expiry the listener returns `504` and kills the
direct child process; it does not guarantee cleanup of descendants launched by the script.

## Common Questions

**Why does startup say `No input provided via stdin`?** A non-TTY daemon startup needs a non-empty piped prompt. Use the
complete piped startup command under Get Started.

**Why did startup fail before listening?** Check that the selected hook name exists, its script is executable, its
interpreter is installed, and its numeric bind address is free. Names and bind strings must be unique in config.

**Why did the listener stop?** Listeners live with the `run` session. Use `--daemon` for an unattended process that must
wait between requests; an interactive `run --hook` can also wait at its prompt. A normal non-interactive run can exit
once its pending schedules and background work are exhausted.

**Why is there no AI response in curl?** The HTTP response acknowledges script execution and inbox insertion. Read the
session output. Hook scripts cannot choose the HTTP response body or status themselves.

**Why was an event skipped or rejected?** Empty/whitespace-only stdout plus exit zero produces `204`. Non-zero exit
produces `500` and logs stderr. Invalid JSON is your script's responsibility. There is no listener-level replay
deduplication: implement it in your adapter if your event sender retries requests.

Test filtering and the POST-only rule against the quick-start hook:

```bash
curl -i -X POST http://127.0.0.1:9876/events --data-binary ''
curl -i http://127.0.0.1:9876/events
```

These return `204` and `405`, respectively, while that listener is running.

## Listener Reference

**HTTP status codes returned to the caller:**

| Situation | Status | Injected? |
|---|---|---|
| Non-POST method | `405 Method Not Allowed` | no — script not run |
| Body could not be read | `400 Bad Request` | no |
| Script exit 0, non-empty stdout | `200 OK` (body `ok`) | yes |
| Script exit 0, empty/whitespace-only stdout | `204 No Content` | no — skipped |
| Script non-zero exit / failed to spawn / output IO error | `500 Internal Server Error` | no; non-zero exits log stderr, other failures log their error |
| Script ran longer than `timeout` | `504 Gateway Timeout` | no |

### Request Environment

The script inherits the Octomind process environment, plus these request-specific variables:

| Variable | Example | Description |
|----------|---------|-------------|
| `HOOK_NAME` | `jira-webhook` | Which hook triggered |
| `HOOK_METHOD` | `POST` | HTTP method |
| `HOOK_PATH` | `/webhook/jira` | Request path |
| `HOOK_QUERY` | `token=abc` | Query string |
| `HOOK_CONTENT_TYPE` | `application/json` | Content-Type header |
| `HOOK_SESSION` | `my-agent` | Session name |
| `HOOK_HEADER_X_GITHUB_EVENT` | `push` | Any header as `HOOK_HEADER_*` |

Use these to route different event types in a single script, validate signatures, or filter by source.

Header names are uppercased and hyphens become underscores: `X-Issue-Event` becomes `HOOK_HEADER_X_ISSUE_EVENT`. Only
header values representable as text are exported. Any POST path is accepted; use `HOOK_PATH` and `HOOK_QUERY` in your
script if you need routing. `0.0.0.0` binds all IPv4 interfaces; the examples bind loopback for local callers.

Implementation: [HTTP listener and script execution](../../src/session/webhook_listener.rs), [config
validation](../../src/config/validation.rs), and [CLI session lifecycle](../../src/session/chat/session/main_loop.rs).

## See also

- [Daemon and Hooks](../integration/03-daemon-and-hooks.md)
- [Event-Driven Agent](02-event-driven-agent.md)
- [Guardrails](../usage/18-guardrails.md)
- [Environment Variables](../reference/04-environment-variables.md)
