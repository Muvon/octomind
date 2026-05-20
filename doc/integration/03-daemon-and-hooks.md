# Daemon Mode and Webhook Hooks

Run Octomind as a persistent background session that reacts to external events.

## Daemon Mode

Start a session that stays alive after processing, accepting injected messages:

```bash
octomind run --name ci-watcher --daemon --format jsonl
```

| Flag | Purpose |
|------|---------|
| `--name` | Required for daemon. Identifies the session. |
| `--daemon` | Keep session alive after processing |
| `--format jsonl` | Structured output for programmatic consumption |

### Sending Messages

Inject messages into a running daemon:

```bash
echo "Check the build status" | octomind send --name ci-watcher
```

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
| `name` | string | required | Unique hook identifier |
| `bind` | string | required | HTTP server address:port |
| `script` | string | required | Path to executable script |
| `timeout` | u32 | 30 | Script timeout (1-3600 seconds) |

### Activating Hooks

```bash
octomind run --name ci-watcher --daemon --format jsonl --hook github-push
```

Multiple hooks can be activated:
```bash
octomind run --daemon --hook github-push --hook slack-notify
```

### Script Interface

When a webhook request arrives:

| Channel | Content |
|---------|---------|
| **stdin** | Raw HTTP body |
| **stdout** | Message to inject into session (on exit 0) |
| **stderr** | Error info (logged on non-zero exit) |

**Exit codes:**
- `0` -- success, stdout is injected as user message
- Non-zero -- failure, stderr is logged

### Environment Variables

Available to hook scripts:

| Variable | Description |
|----------|-------------|
| `HOOK_NAME` | Hook identifier |
| `HOOK_METHOD` | HTTP method (GET, POST, etc.) |
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

All injected messages flow through a unified inbox system:

**Message sources:**
- **Schedule** -- scheduled messages from the `schedule` tool
- **BackgroundAgent** -- completed async agent jobs
- **TapRun** -- completed tap run (specialist agent) jobs
- **Skill** -- skill activations requiring content injection
- **SkillValidator** -- skill validation results
- **Inject** -- external injection via `octomind send`
- **Webhook** -- HTTP webhook requests

Messages are drained in order: scheduled → background → tap run → skill → skill validator → inject → webhook. Each session has an isolated queue with async notification support.

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
octomind run --name slack-bot --daemon --hook slack-event
```

### Automated Code Review

```bash
octomind run --name reviewer --daemon --hook github-pr
# Script extracts PR details
# AI reviews code and posts comments
```

## Further Reading

See [Custom Hooks](../use-cases/09-custom-hooks.md) for comprehensive hook development guide with examples in Python, Node.js, Bash, Ruby, and Go -- including signature validation, event filtering, and multi-hook architecture patterns.
