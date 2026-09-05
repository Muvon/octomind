# Daemon Mode and Webhook Hooks

Run Octomind as a persistent session that reacts to external events. This guide is for automation authors using
`octomind send` or HTTP hooks to enqueue work.

## Daemon Mode

Start a session from a terminal that stays alive after processing, accepting injected messages:

```bash
octomind run --name ci-watcher --daemon --format jsonl
```

| Flag | Purpose |
|------|---------|
| `--name` | Optional. Without it the session gets an auto-generated name. Provide a stable name if you want to inject messages with `octomind send`. |
| `--daemon` | Keep the session alive after the first turn, waiting for injected messages. |
| `--format <plain\|jsonl>` | Select non-interactive output when stdin is a terminal. `jsonl` is recommended for programmatic consumers; `plain` also works. |
| `--hook <NAME>` | Activate a configured webhook listener. Repeatable. |

`--daemon` takes effect in the non-interactive loop. A session is non-interactive when you pass any `--format` value or
when stdin is not a TTY (`is_interactive_session = format.is_none() && stdin.is_terminal()`). From a terminal,
therefore, pass `--format plain` or `--format jsonl`; with piped stdin, `--format` is optional.

For a pipeline, service, or background shell job, provide a non-empty initial prompt. The process reads stdin to EOF
before starting listeners; `/dev/null` and empty pipes fail even with `--daemon`. `--daemon` keeps the process alive but
does not detach it from your terminal:

```bash
printf '%s\n' 'Wait for CI events and summarize each event.' | \
  octomind run --name ci-watcher --daemon --format jsonl >ci-watcher.jsonl 2>ci-watcher.log &
ci_watcher_pid=$!
```

When you have finished with that background process, stop it from the same shell:

```bash
kill -TERM "$ci_watcher_pid"
```

On Unix, SIGTERM cleans up MCP servers and exits with status 130. This is process termination, not a request to finish
the current AI turn; it can leave an IPC file for cleanup at the next bind.

Without `--daemon`, a non-interactive run still waits for pending schedules, monitors, agents, detached shell jobs, and
tap runs; it exits once that work is exhausted. Normal model-call failures are reported and the daemon keeps listening,
though startup or other propagated errors can still terminate the process.

### Other long-lived background sessions

`octomind run --daemon` is not the only long-lived entry point — `octomind server` (WebSocket) and `octomind acp` also
run persistent sessions. But they reach you differently:

- **`octomind server`** — a WebSocket server (see [WebSocket Server](01-websocket-server.md)). Clients send messages
  over the WebSocket connection; there is no inject socket and no `--hook` support.

- **`octomind acp`** — the Agent Client Protocol bridge. The ACP client drives the session over stdio; `octomind acp`
  accepts a `--hook` flag but does not start a webhook listener from it, and it exposes no `octomind send` socket.

All entry points share the same internal inbox abstraction, but only `octomind run` binds the external `octomind send`
and `--hook` webhook listeners described below.

### Sending Messages

Inject a message into a running session by name. Pass it as an argument or pipe it via stdin:

```bash
# As an argument
octomind send --name ci-watcher "Check the build status"

# Or piped from stdin
echo "Check the build status" | octomind send --name ci-watcher
```

A successful send acknowledges enqueueing, not AI completion; read the daemon output for the response. The listener
replies on the wire with `ok\n` on success or `error: ...\n` on failure; the `send` command surfaces a non-zero exit
when it gets an error. If no session by that name is running, `send` fails immediately. The endpoint in the error
depends on your runtime directory and user ID:

```text
no running session named 'ci-watcher' (socket not found at "/tmp/octomind-1000/ci-watcher.sock")
```

### IPC endpoints

Each running session exposes one per-name IPC endpoint that `octomind send` connects to:

| Platform | Endpoint | Extra |
|----------|----------|-------|
| Unix (macOS/Linux) | Unix socket at `$XDG_RUNTIME_DIR/octomind/<name>.sock` (fallback: `<system tmp>/octomind-<uid>/<name>.sock`) | PID written to `<name>.pid` |
| Windows | Named pipe `\\.\pipe\octomind-<name>` | — |

Long Unix socket names are shortened and suffixed with a hash to fit the platform path limit; `send` derives the same
filename. Run the sender as the same OS user with the same runtime environment. `OCTOMIND_DATA_DIR` does not relocate
these host-local endpoints. Avoid two live `run` processes with the same session name.

These files are created on session start and auto-cleaned when the session exits (a stale socket from a crash is removed
on next bind). The injected message is trimmed; an empty message is rejected with `error: empty message`.

## Webhook Hooks

HTTP webhook listeners that pipe payloads through scripts and inject output into the session.

### Configuration

The following Bash example needs `jq` and creates a local executable script. Run it from the directory where you will
launch the daemon:

```bash
mkdir -p hooks
cat >hooks/process-github-push.sh <<'SH'
#!/bin/bash
set -euo pipefail
jq -er '"New push to \(.repository.full_name) (\(.ref | sub("^refs/heads/"; ""))) by \(.pusher.name): \(.commits | length) commit(s). Please review the changes."'
SH
chmod +x hooks/process-github-push.sh
```

Add this entry to your `config/config.toml` under the Octomind data directory (`OCTOMIND_DATA_DIR`, or
`~/.local/share/octomind` on macOS/Linux and `%LOCALAPPDATA%/octomind` on Windows). Relative script paths resolve from
the process launch directory; use an absolute path for service deployments.

```toml
[[hooks]]
name = "github-push"
bind = "127.0.0.1:9876"
script = "./hooks/process-github-push.sh"
timeout = 30
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique hook identifier (referenced by `--hook`). Must be unique across `[[hooks]]`. |
| `bind` | string | required | HTTP listener `address:port`. Must parse as a socket address and be unique across `[[hooks]]`. |
| `script` | string | required | Path to the executable script |
| `timeout` | u64 | 30 | Script timeout in seconds (must be > 0, max 3600) |

> **Startup validation.** Config validation rejects duplicate hook names, duplicate bind addresses, empty/invalid bind
> addresses, and timeouts outside `1..=3600`. When a hook is activated, it is validated again before binding: the bind
> address must parse, the script must exist and be a regular file, and on Unix it must be executable (`chmod +x`). Any
> failure aborts session start, so a missing `chmod +x` is a startup error, not a silent no-op.

### Activating Hooks

```bash
octomind run --name ci-watcher --daemon --format jsonl --hook github-push
```

To activate two listeners, first add another configuration entry with a unique name and bind address:

```toml
[[hooks]]
name = "github-push-secondary"
bind = "127.0.0.1:9877"
script = "./hooks/process-github-push.sh"
timeout = 30
```

```bash
octomind run --name ci-watcher --daemon --format jsonl --hook github-push --hook github-push-secondary
```

These startup examples assume terminal stdin. Use the non-empty pipeline shown above for a background service. Only
`run` starts these listeners; see the transport comparison above.

### Script Interface

Only **POST** requests invoke the script. Non-POST requests are rejected with `405` *before* the script is ever spawned,
so `HOOK_METHOD` (see below) is effectively always `POST`.

When a POST arrives:

| Channel | Content |
|---------|---------|
| **stdin** | Raw HTTP request body |
| **stdout** | Message to inject into the session (on exit 0) |
| **stderr** | Error info (logged on non-zero exit) |

**Exit codes:**

- `0` — success. stdout is **trimmed** (leading/trailing whitespace stripped) and injected as a user message. If stdout
  is **empty after trimming, nothing is injected** and the listener returns `204` — even on exit 0.

- Non-zero — failure. stderr is logged and the listener returns `500`.

### HTTP response contract

External senders (GitHub, Slack, etc.) can interpret these status codes:

| Status | Meaning |
|--------|---------|
| `200 ok` | Script succeeded; trimmed stdout injected into the session |
| `204` | Script succeeded but produced empty output; nothing injected |
| `400` | Failed to read the request body |
| `405` | Non-POST request rejected (script not run) |
| `500` | Script failed to spawn, hit an I/O error, or exited non-zero |
| `504` | Script exceeded `timeout` seconds and was killed |

> **Debugging "why didn't my message inject?"** If your sender logs a `204`, the script ran fine but printed nothing
> (after trimming) — check that it actually echoes a message. A `405` means the request reached the listener but was not
> a POST.

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

### Test a GitHub push payload

With the daemon running, send a local test event from another terminal:

```bash
curl -i 'http://127.0.0.1:9876/push?source=local' \
  -H 'Content-Type: application/json' -H 'X-GitHub-Event: push' \
  --data '{"repository":{"full_name":"example/demo"},"ref":"refs/heads/main","pusher":{"name":"alice"},"commits":[{"id":"abc123"}]}'
```

Expect HTTP `200` with `ok`, followed by an injected turn in the daemon output. The script can read the custom header as
`HOOK_HEADER_X_GITHUB_EVENT`, the path as `HOOK_PATH=/push`, and the query as `HOOK_QUERY=source=local`. All paths
accept POST; there is no configured route filter. The listener does not validate webhook signatures or provide
TLS/authentication. This local example performs no signature validation; add that in your script or proxy before
accepting remote events. Hook `200` acknowledges enqueueing, not completion of the AI's work.

## Unified Inbox

All injected messages flow through a unified inbox system. Each session has its own isolated queue with async
notification support.

**Message sources:**

- **Schedule** — scheduled messages from the `schedule` tool
- **Monitor** — bounded output batches from the `monitor` tool
- **BackgroundAgent** — completed async agent jobs
- **BackgroundJob** — completed detached shell jobs
- **TapRun** — completed tap run (specialist agent) jobs
- **Skill** — skill activations requiring content injection
- **SkillValidator** — skill validation results
- **Inject** — external injection via `octomind send`
- **Webhook** — HTTP webhook requests
- **GuardrailHook** — output from a guardrail post-result `[[hook]]` script (see
  [Guardrails](../usage/18-guardrails.md))

- **GuardValidator** — output from a guardrail end-of-turn `[[validator]]` (see [Guardrails](../usage/18-guardrails.md))

At turn boundaries, the queue drains from the front. Consecutive system-managed results can be batched into one AI turn;
human-shaped injections (`send` and webhooks) each own a separate turn. During an active turn, the runtime can consume a
system result from later in the queue while leaving new user tasks pending. Repeated pending output from the same
monitor is coalesced and bounded, so this is not a strict one-event/one-turn FIFO contract.

### Background agent completions

Async configured-agent jobs (`agent_*` tools) inject their result through the **BackgroundAgent** source when they
finish, with a wrapper prefix the AI sees on its next turn:

- Success: `[Async agent '<name>' completed]` followed by the agent output.
- Failure: `[Async agent '<name>' failed]` followed by the error.

These are tracked by a background job manager with a concurrency cap; attempts to launch beyond the limit are rejected.

## Common questions

| Symptom | What to check |
|---------|---------------|
| `No input provided via stdin` | Pipe a non-empty initial prompt; empty stdin is rejected even for a daemon. |
| `send` cannot find the session | Use a live `run` session, the exact name, the same OS user and runtime directory. |
| Hook fails during startup | Check unique name/bind, an existing executable script, and an available port. |
| HTTP `204` | The script emitted only whitespace or no stdout; print the message you want injected. |
| HTTP `405` | Send POST, as in the `curl --data` example above. |
| HTTP `500` or `504` | Inspect script stderr and timeout; the timeout covers script I/O, not request-body upload. |
| HTTP `200`, but no AI answer yet | The message is queued; inspect the daemon stream and current work. |

## See also

- [Run flags](../../src/commands/run.rs), [listener startup](../../src/session/chat/session/main_loop.rs), and [send
  command](../../src/commands/send.rs).

- [HTTP hook implementation](../../src/session/webhook_listener.rs), [hook validation](../../src/config/validation.rs),
  and [inbox batching](../../src/session/inbox.rs).

- [Custom Hooks](../use-cases/09-custom-hooks.md) — more hook scripting examples.
- [WebSocket Server](01-websocket-server.md) — remote sessions over WebSocket.
- [ACP Protocol](02-acp-protocol.md) — stdio sessions and background inbox updates.
- [Guardrails](../usage/18-guardrails.md) — guardrail scripts, distinct from HTTP `[[hooks]]`.
