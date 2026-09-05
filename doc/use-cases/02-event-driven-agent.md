# Event-Driven Webhooks

Use this guide to connect external HTTP events to a persistent Octomind session. It covers webhook scripts, daemon
startup, manual messages, and delivery troubleshooting.

## Get started

The shell examples require Bash, `jq`, and an installed, authenticated Octomind. Start in the checkout the agent should
inspect; receiving a webhook does not fetch or update that checkout.

### Architecture

```text
GitHub/Slack/PagerDuty
    |
    | HTTP POST
    v
Webhook Hook (HTTP listener on port 9876)
    |
    | stdin: raw HTTP body
    v
Hook Script (github-push.sh)
    |
    | stdout: message for AI (only on exit 0)
    v
Daemon Session Inbox
    |
    v
AI processes event, takes action
```

The **inbox** is the daemon's queue of pending events. When a hook script exits 0 with non-empty output, that stdout is
added to the session as the **next user message** — the AI responds to it on its next turn, just as if you had typed it
interactively. The daemon processes each external event as a separate user turn when it reaches the head of the queue.

## Write a hook script

Create `hooks/github-push.sh` in the checkout with the contents below:

```bash
mkdir -p hooks
```

The contract is simple and decided by the script's **exit code**:

- **Exit 0 with non-empty stdout** — the trimmed stdout is injected as the next user message (listener responds `200
  ok`).
- **Exit 0 with empty stdout** — nothing is injected; the listener responds `204 No Content`.
- **Non-zero exit** — nothing is injected; the listener responds `500` and logs the script's stderr at error level.

For a deliberately ignored event, exit `0` without printing anything so the sender receives `204`. Reserve a non-zero
exit for actual script failures, which return `500` and may trigger the sender's retry or alerting policy.

```bash
#!/bin/bash
# ./hooks/github-push.sh
set -euo pipefail

payload=$(cat)

repo=$(echo "$payload" | jq -r '.repository.full_name')
branch=$(echo "$payload" | jq -r '.ref' | sed 's|refs/heads/||')
pusher=$(echo "$payload" | jq -r '.pusher.name')
commits=$(echo "$payload" | jq -r '.commits | length')

# Only react to main branch
if [ "$branch" != "main" ]; then
  exit 0  # Empty stdout = nothing injected (sender gets HTTP 204)
fi

# Files changed
files=$(echo "$payload" | jq -r '.commits[].modified[]' | sort -u | sed -n '1,20p')

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
chmod +x ./hooks/github-push.sh
```

## Configure the hook

Add this block to a `.toml` file in Octomind's config directory: `$OCTOMIND_DATA_DIR/config` when overridden, otherwise
`~/.local/share/octomind/config` on Linux/macOS or `%LOCALAPPDATA%/octomind/config` on Windows. Relative script paths
resolve from the process working directory.

```toml
[[hooks]]
name = "github-push"
bind = "127.0.0.1:9876"
script = "./hooks/github-push.sh"
timeout = 30
```

Each hook needs a **unique `name`** and a **unique `bind` address**. `bind` must be a numeric IP and port (for example,
`127.0.0.1:9876`), `script` must be non-empty, and `timeout` must be between 1 and 3600 seconds (default 30). Octomind
validates these at startup and refuses to launch if any rule is violated. An activated script must exist, be a file, and
be executable on Unix.

## Start the daemon

```bash
printf 'Wait for events and review the local checkout when requested.\n' \
  | octomind run developer:general --name code-monitor --daemon --format jsonl --hook github-push
```

`--daemon` keeps a non-interactive session waiting for inbox work. Use `--format jsonl` for machine-readable output.
With terminal stdin you can omit the initial prompt; with non-terminal stdin (including `/dev/null`), startup requires a
non-empty prompt, as above. `--name` is optional, but gives senders a predictable session name; reusing it resumes
existing history.

## Deliver an event

Send this local fixture from another terminal. A `200` response acknowledges queueing, not completion of the AI review:

```bash
curl -i http://127.0.0.1:9876/ \
  -H 'Content-Type: application/json' -H 'X-GitHub-Event: push' \
  --data '{
    "repository":{"full_name":"example/project"}, "ref":"refs/heads/main",
    "pusher":{"name":"ci"}, "commits":[{"modified":["src/main.rs"]}]
  }'
```

The listener implements plain HTTP POST handling; it does not verify webhook signatures or provide TLS. For an external
sender, put authentication and TLS in your ingress proxy and forward verified requests to this loopback listener. The
script receives request headers if you implement sender verification there.

### Multiple Hooks

You can activate multiple configured hooks by repeating `--hook`. For example, add a second listener using the same
script (keep the `github-push` block above once):

```toml
[[hooks]]
name = "github-push-secondary"
bind = "127.0.0.1:9877"
script = "./hooks/github-push.sh"
timeout = 15
```

Stop the first daemon before starting this replacement, because both activate the listener on port 9876:

```bash
printf 'Wait for push events.\n' \
  | octomind run developer:general --name code-monitor --daemon --format jsonl \
      --hook github-push --hook github-push-secondary
```

### Injecting Messages Manually

Besides webhooks, you can inject messages directly:

```bash
echo "Summarize what happened in the last hour" | octomind send --name code-monitor
```

`octomind send` connects to the running session over a per-session Unix socket (`<run_dir>/<stem>.sock`, with long names
shortened and hashed) or, on Windows, a named pipe (`\\.\pipe\octomind-<name>`). It only works **while a daemon/session
with that name is live** — if no such session is running, it fails with `no running session named '<name>'`. The message
must be non-empty, and `send` reads back `ok` on success or an error string. (You can pass the message as an argument
instead of piping it via stdin.) The CLI prints a send receipt, not the AI response:

```bash
octomind send --name code-monitor 'Summarize the events received so far.'
```

On Unix, `run_dir` is `$XDG_RUNTIME_DIR/octomind` when set, otherwise the system temp directory plus `octomind-<uid>`.
It is host-local and is not beneath `OCTOMIND_DATA_DIR`.

### Reacting to Background Work

Background agent and tap results also arrive through the inbox; see [multi-agent
delegation](05-multi-agent-delegation.md). They are system-managed continuations and may be batched together. Webhooks
and `send` carry separate user turns. JSONL emits an `injected` event with `source_kind`, `source_label`, `content`, and
`session_id` for each inbox message.

## Common questions

- **Why does startup say no input was provided?** Use the piped startup prompt above when stdin is not a terminal.
- **Why does the hook fail before listening?** Check that the selected hook exists, the script is executable, and its
  port is free. Only hooks named by `--hook` are activated.
- **Why did HTTP 200 arrive before a review?** It acknowledges the script output entering the in-memory inbox. It is not
  a durable delivery receipt or an AI completion signal; pending inbox messages do not survive process exit.
- **Why did a turn fail while the daemon stayed up?** The daemon logs API response failures and continues listening;
  one-shot runs return those errors. Setup or preparation errors can still terminate the daemon.
- **Where are the changed files?** The agent can use its configured tools on the local checkout. The sample webhook only
  supplies metadata and does not synchronize source files.

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

The listener serves all paths — it only requires the method to be `POST` — so `HOOK_PATH` reflects whatever URL the
sender used. Every request header with a text-decodable value is also exposed as `HOOK_HEADER_<NAME>` (uppercased,
dashes replaced with underscores).

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
    exit 0  # Unknown event: empty stdout, nothing injected (HTTP 204)
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

A non-zero script exit returns `500` to the sender (body `Script error (exit N)`) and logs the script's stderr at error
level — so a 500 you see in your webhook provider usually means the hook script failed or deliberately bailed, not that
Octomind is down.

## See also

- [CLI reference](../reference/01-cli-reference.md)
- [Custom hooks](09-custom-hooks.md)
- [Multi-agent delegation](05-multi-agent-delegation.md)
