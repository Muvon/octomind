# Use Case: Event-Driven Agent with Webhooks

Run Octomind as a persistent daemon that reacts to external events -- GitHub pushes, Slack messages, monitoring alerts.

## The Problem

You want an AI agent that monitors events and acts autonomously: reviewing pushed code, responding to incidents, or summarizing deployments. Polling is wasteful; you need event-driven reactions.

## Solution

Combine daemon mode with webhook hooks.

### Architecture

```
GitHub/Slack/PagerDuty
    |
    | HTTP POST
    v
Webhook Hook (HTTP listener on port 9876)
    |
    | stdin: raw HTTP body
    v
Hook Script (process-event.sh)
    |
    | stdout: message for AI (only on exit 0)
    v
Daemon Session Inbox
    |
    v
AI processes event, takes action
```

The **inbox** is the daemon's queue of pending events. When a hook script exits 0 with
non-empty output, that stdout is added to the session as the **next user message** -- the
AI responds to it on its next turn, just as if you had typed it interactively. The daemon
drains this inbox continuously, so events are processed one turn at a time.

### Step 1: Write a Hook Script

The contract is simple and decided by the script's **exit code**:

- **Exit 0 with non-empty stdout** -- the trimmed stdout is injected as the next user message (listener responds `200 ok`).
- **Exit 0 with empty stdout** -- nothing is injected; the listener responds `204 No Content`.
- **Non-zero exit** -- nothing is injected; the listener responds `500` and logs the script's stderr at error level.

So `exit 1` is how you tell Octomind "this event isn't interesting, drop it" -- the
webhook sender will see an HTTP 500, which may trigger its retry or alerting.

```bash
#!/bin/bash
# /opt/hooks/github-push.sh

payload=$(cat)

repo=$(echo "$payload" | jq -r '.repository.full_name')
branch=$(echo "$payload" | jq -r '.ref' | sed 's|refs/heads/||')
pusher=$(echo "$payload" | jq -r '.pusher.name')
commits=$(echo "$payload" | jq -r '.commits | length')

# Only react to main branch
if [ "$branch" != "main" ]; then
  exit 1  # Non-zero = nothing injected (sender gets HTTP 500)
fi

# Files changed
files=$(echo "$payload" | jq -r '.commits[].modified[]' | sort -u | head -20)

cat <<EOF
Push to $repo/$branch by $pusher ($commits commits).

Changed files:
$files

Please:
1. Review the changes for potential issues
2. Check if any tests might be affected
3. Summarize the changes in 2-3 sentences
EOF
```

Make it executable:
```bash
chmod +x /opt/hooks/github-push.sh
```

### Step 2: Configure the Hook

```toml
[[hooks]]
name = "github-push"
bind = "0.0.0.0:9876"
script = "/opt/hooks/github-push.sh"
timeout = 30
```

Each hook needs a **unique `name`** and a **unique `bind` address**. `bind` must be a
valid `host:port` socket address, `script` must be non-empty, and `timeout` must be
between 1 and 3600 seconds (default 30). Octomind validates these at startup and refuses
to launch if any rule is violated.

### Step 3: Start the Daemon

```bash
octomind run --name code-monitor --daemon --format jsonl --hook github-push
```

`--daemon` keeps the process running indefinitely, draining its inbox and waiting for the
next event. It must run **non-interactively** -- a session decides interactivity from
`stdin` being a terminal and `--format` being unset, so use `--format jsonl` (recommended:
output is machine-parseable) or pipe stdin. `--name` is required so external tools can
reach the session via `octomind send`.

### Step 4: Point GitHub Webhook

In your GitHub repo settings:
- Webhook URL: `http://your-server:9876/`
- Content type: `application/json`
- Events: "Push"

Now every push to `main` triggers an AI review.

### Multiple Hooks

Monitor different event sources simultaneously:

```toml
[[hooks]]
name = "github-push"
bind = "0.0.0.0:9876"
script = "/opt/hooks/github-push.sh"
timeout = 30

[[hooks]]
name = "slack-mention"
bind = "0.0.0.0:9877"
script = "/opt/hooks/slack-mention.sh"
timeout = 15

[[hooks]]
name = "pagerduty-alert"
bind = "0.0.0.0:9878"
script = "/opt/hooks/pagerduty-alert.sh"
timeout = 30
```

```bash
octomind run --name ops-agent --daemon --format jsonl \
  --hook github-push \
  --hook slack-mention \
  --hook pagerduty-alert
```

### Injecting Messages Manually

Besides webhooks, you can inject messages directly:

```bash
echo "Summarize what happened in the last hour" | octomind send --name ops-agent
```

`octomind send` connects to the running session over a per-session Unix socket
(`<data>/run/<name>.sock`) or, on Windows, a named pipe (`\\.\pipe\octomind-<name>`). It
only works **while a daemon/session with that name is live** -- if no such session is
running, it fails with `no running session named '<name>'`. The message must be non-empty,
and `send` reads back `ok` on success or an error string. (You can pass the message as an
argument instead of piping it via stdin.)

### Reacting to Background Work

There is a third event source besides webhooks and manual `send`: **completed background
agents**. When a delegated async agent (an `agent_<name>` job spawned with `async = true`,
or a background tap run) finishes, its result is pushed into the same inbox the daemon
drains, prefixed with `[Async agent 'NAME' completed]` or `[Async agent 'NAME' failed]`.
The daemon processes these identically to webhook and `send` messages, so a long-running
agent can fire off work and react to its own results asynchronously.

## Hook Script Environment

Your script receives rich context via environment variables:

| Variable | Example |
|----------|---------|
| `HOOK_NAME` | `github-push` |
| `HOOK_METHOD` | `POST` |
| `HOOK_PATH` | `/` (whatever path the sender POSTed to) |
| `HOOK_QUERY` | `repo=foo&action=push` (raw URL query string, empty if none) |
| `HOOK_CONTENT_TYPE` | `application/json` |
| `HOOK_SESSION` | `code-monitor` |
| `HOOK_HEADER_X_GITHUB_EVENT` | `push` |

The listener serves all paths -- it only requires the method to be `POST` -- so `HOOK_PATH`
reflects whatever URL the sender used. Every request header is also exposed as
`HOOK_HEADER_<NAME>` (uppercased, dashes replaced with underscores).

Use these to route different event types in a single script:

```bash
#!/bin/bash
event="$HOOK_HEADER_X_GITHUB_EVENT"
payload=$(cat)   # read the HTTP body once — stdin can only be consumed a single time

case "$event" in
  push)
    echo "Code pushed: $(echo "$payload" | jq -r '.commits | length') commits"
    ;;
  pull_request)
    echo "PR $(echo "$payload" | jq -r '.action'): $(echo "$payload" | jq -r '.pull_request.title')"
    ;;
  *)
    exit 1  # Unknown event: nothing injected (sender gets HTTP 500)
    ;;
esac
```

## HTTP Responses

The listener returns a status code the webhook sender can use for retry and health logic:

| Status | Meaning |
|--------|---------|
| `200` | Script exited 0 with output; message injected (body `ok`) |
| `204` | Script exited 0 with empty output; nothing injected |
| `400` | Request body could not be read |
| `405` | Request was not `POST` |
| `500` | Script exited non-zero, or failed to spawn / had an IO error |
| `504` | Script exceeded its `timeout` |

A non-zero script exit returns `500` to the sender (body `Script error (exit N)`) and logs
the script's stderr at error level -- so a 500 you see in your webhook provider usually
means the hook script failed or deliberately bailed, not that Octomind is down.

## Key Points

- `--daemon` keeps the session alive between events, draining its inbox; it must run
  non-interactively (use `--format jsonl` or pipe stdin).
- The daemon is resilient: if a turn hits an API error, it logs it and keeps listening
  rather than exiting (unlike a one-shot non-interactive run, which exits non-zero).
- `--hook` activates webhook listeners (multiple allowed); each needs a unique name and bind.
- Hook script contract: exit 0 + non-empty stdout = inject (HTTP 200); exit 0 + empty
  stdout = nothing injected (HTTP 204); non-zero exit = nothing injected (HTTP 500).
- Injected text becomes a literal user message; the AI answers it on its next turn.
- `octomind send --name X` injects a message manually -- but only while a session named `X`
  is actually running.
- JSONL output is parseable by downstream tools.
- The AI has full tool access -- it can read the actual files, not just the webhook payload.
