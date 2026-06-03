# Daemon Mode and Webhook Hooks

Run Octomind as a persistent background session that reacts to external events.

> **`octomind send` and `--hook` webhooks are `run`-only.** The message-injection listener (the Unix socket / named pipe that `octomind send` connects to) and any activated `--hook` webhook listeners are started **only** on the `octomind run` path — both interactive `octomind run` and non-interactive `octomind run --format ...` (`init_session_runtime`, `src/session/chat/session/main_loop.rs`). The WebSocket `octomind server` and `octomind acp` drive their sessions over their own client transports and do **not** bind an inject socket or webhook listener — `octomind send` cannot reach them, and `acp`'s `--hook` flag, while accepted, starts nothing. Daemon mode (`--daemon`) is special only because it keeps the loop alive *after* the first turn so injected messages still have somewhere to land.

## Daemon Mode

Start a session that stays alive after processing, accepting injected messages:

```bash
octomind run --name ci-watcher --daemon --format jsonl
```

| Flag | Purpose |
|------|---------|
| `--name` | Optional. Without it the session gets an auto-generated name. Provide a stable name if you want to inject messages with `octomind send`. |
| `--daemon` | Keep the session alive after the first turn, waiting for injected messages. |
| `--format <plain\|jsonl>` | Run non-interactively. Required for `--daemon` (see below). `jsonl` is recommended for programmatic consumers; `plain` also works. |
| `--hook <NAME>` | Activate a configured webhook listener. Repeatable. |

`--daemon` requires **non-interactive mode**, not `jsonl` specifically. A session is non-interactive when you pass any `--format` value *or* when stdin is piped rather than a TTY (`is_interactive_session = format.is_none() && stdin.is_terminal()`). Passing `--daemon` from an interactive terminal without `--format` will not enter daemon mode.

### Other long-lived background sessions

`octomind run --daemon` is not the only long-lived entry point — `octomind server` (WebSocket) and `octomind acp` also run persistent sessions. But they reach you differently:

- **`octomind server`** — a WebSocket server (see [WebSocket Server](01-websocket-server.md)). Clients send messages over the WebSocket connection; there is no inject socket and no `--hook` support.
- **`octomind acp`** — the Agent Client Protocol bridge. The ACP client drives the session over stdio; `octomind acp` accepts a `--hook` flag but does not start a webhook listener from it, and it exposes no `octomind send` socket.

All entry points share the same internal inbox abstraction, but only `octomind run` binds the external `octomind send` and `--hook` webhook listeners described below.

### Sending Messages

Inject a message into a running session by name. Pass it as an argument or pipe it via stdin:

```bash
# As an argument
octomind send --name ci-watcher "Check the build status"

# Or piped from stdin
echo "Check the build status" | octomind send --name ci-watcher
```

The listener replies on the wire with `ok\n` on success or `error: ...\n` on failure; the `send` command surfaces a non-zero exit when it gets an error. If no session by that name is running, `send` fails immediately:

```
no running session named 'ci-watcher' (socket not found at ...)
```

#### IPC endpoints

Each running session exposes one per-name IPC endpoint that `octomind send` connects to:

| Platform | Endpoint | Extra |
|----------|----------|-------|
| Unix (macOS/Linux) | Unix socket at `~/.local/share/octomind/run/<name>.sock` | PID written to `<name>.pid` |
| Windows | Named pipe `\\.\pipe\octomind-<name>` | — |

These files are created on session start and auto-cleaned when the session exits (a stale socket from a crash is removed on next bind). The injected message is trimmed; an empty message is rejected with `error: empty message`.

## Webhook Hooks

HTTP webhook listeners that pipe payloads through scripts and inject output into the session.

### Configuration

```toml
[[hooks]]
name = "github-push"
bind = "0.0.0.0:9876"
script = "/path/to/process-github-push.sh"
timeout = 30
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique hook identifier (referenced by `--hook`). Must be unique across `[[hooks]]`. |
| `bind` | string | required | HTTP listener `address:port`. Must parse as a socket address and be unique across `[[hooks]]`. |
| `script` | string | required | Path to the executable script |
| `timeout` | u64 | 30 | Script timeout in seconds (must be > 0, max 3600) |

> **Startup validation.** Config validation rejects duplicate hook names, duplicate bind addresses, empty/invalid bind addresses, and timeouts outside `1..=3600`. When a hook is activated, it is validated again before binding: the bind address must parse, the script must exist and be a regular file, and on Unix it must be executable (`chmod +x`). Any failure aborts session start, so a missing `chmod +x` is a startup error, not a silent no-op.

### Activating Hooks

```bash
octomind run --name ci-watcher --daemon --format jsonl --hook github-push
```

Multiple hooks can be activated (repeat `--hook`):
```bash
octomind run --name ci-watcher --daemon --format jsonl --hook github-push --hook slack-notify
```

### Script Interface

Only **POST** requests invoke the script. Non-POST requests are rejected with `405` *before* the script is ever spawned, so `HOOK_METHOD` (see below) is effectively always `POST`.

When a POST arrives:

| Channel | Content |
|---------|---------|
| **stdin** | Raw HTTP request body |
| **stdout** | Message to inject into the session (on exit 0) |
| **stderr** | Error info (logged on non-zero exit) |

**Exit codes:**
- `0` -- success. stdout is **trimmed** (leading/trailing whitespace stripped) and injected as a user message. If stdout is **empty after trimming, nothing is injected** and the listener returns `204` — even on exit 0.
- Non-zero -- failure. stderr is logged and the listener returns `500`.

#### HTTP response contract

External senders (GitHub, Slack, etc.) can interpret these status codes:

| Status | Meaning |
|--------|---------|
| `200 ok` | Script succeeded; trimmed stdout injected into the session |
| `204` | Script succeeded but produced empty output; nothing injected |
| `400` | Failed to read the request body |
| `405` | Non-POST request rejected (script not run) |
| `500` | Script failed to spawn, hit an I/O error, or exited non-zero |
| `504` | Script exceeded `timeout` seconds and was killed |

> **Debugging "why didn't my message inject?"** If your sender logs a `204`, the script ran fine but printed nothing (after trimming) — check that it actually echoes a message. A `405` means the request reached the listener but was not a POST.

### Environment Variables

Available to hook scripts:

| Variable | Description |
|----------|-------------|
| `HOOK_NAME` | Hook identifier |
| `HOOK_METHOD` | HTTP method — always `POST` (non-POST requests never reach the script) |
| `HOOK_PATH` | Request path |
| `HOOK_QUERY` | Query string |
| `HOOK_CONTENT_TYPE` | Content-Type header |
| `HOOK_SESSION` | Session name |
| `HOOK_HEADER_*` | Each HTTP header (uppercased, hyphens to underscores) |

### Example: GitHub Push Hook

Script (`/path/to/process-github-push.sh`):

```bash
#!/bin/bash
# Read JSON payload from stdin
payload=$(cat)

# Extract info
repo=$(echo "$payload" | jq -r '.repository.full_name')
branch=$(echo "$payload" | jq -r '.ref' | sed 's|refs/heads/||')
pusher=$(echo "$payload" | jq -r '.pusher.name')
commits=$(echo "$payload" | jq -r '.commits | length')

# Output message to inject into session
echo "New push to $repo ($branch) by $pusher: $commits commit(s). Please review the changes."
```

## Unified Inbox

All injected messages flow through a unified inbox system. Each session has its own isolated queue with async notification support.

**Message sources:**
- **Schedule** -- scheduled messages from the `schedule` tool
- **BackgroundAgent** -- completed async agent jobs
- **TapRun** -- completed tap run (specialist agent) jobs
- **Skill** -- skill activations requiring content injection
- **SkillValidator** -- skill validation results
- **Inject** -- external injection via `octomind send`
- **Webhook** -- HTTP webhook requests
- **GuardrailHook** -- output from a guardrail post-result `[[hook]]` script (see [Guardrails](../usage/18-guardrails.md))
- **GuardValidator** -- output from a guardrail end-of-turn `[[validator]]` (see [Guardrails](../usage/18-guardrails.md))

Messages are drained in **FIFO arrival order** — the queue is a per-session `VecDeque` and the loop pops from the front. The source kind only sets the display label and icon shown next to the injected turn; it does **not** affect ordering or priority. A message is processed in the order it arrived, regardless of which source produced it.

### Background agent completions

Async agent jobs (`agent` tool) inject their result through the **BackgroundAgent** source when they finish, with a wrapper prefix the AI sees on its next turn:

- Success: `[Async agent '<name>' completed]` followed by the agent output.
- Failure: `[Async agent '<name>' failed]` followed by the error.

These are tracked by a background job manager that caps the number of concurrent async jobs (`max_concurrent`); attempts to launch beyond the limit are rejected.

## Use Cases

### CI/CD Monitoring

```bash
# Start daemon
octomind run --name ci-bot --daemon --format jsonl --hook github-push

# GitHub webhook posts to http://server:9876/
# Script processes payload, injects summary
# AI analyzes changes and reports issues
```

### Slack Integration

```bash
octomind run --name slack-bot --daemon --format jsonl --hook slack-event
```

### Automated Code Review

```bash
octomind run --name reviewer --daemon --format jsonl --hook github-pr
# Script extracts PR details
# AI reviews code and posts comments
```

## Further Reading

See [Custom Hooks](../use-cases/09-custom-hooks.md) for comprehensive hook development guide with examples in Python, Node.js, Bash, Ruby, and Go -- including signature validation, event filtering, and multi-hook architecture patterns.
