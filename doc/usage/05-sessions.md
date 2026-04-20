# Sessions

All interaction with Octomind happens through sessions. A session is a conversation with context, tool access, and cost tracking.

## Starting Sessions

```bash
# Default role
octomind run

# Specific role
octomind run developer

# Tap agent
octomind run octomind:assistant
octomind run developer:rust

# Named session
octomind run --name feature-auth

# Model override
octomind run -m anthropic:claude-sonnet-4
```

## Resuming Sessions

```bash
# Resume by name
octomind run --resume feature-auth

# Resume most recent
octomind run --resume-recent
```

List all sessions:
```
/list
/list 2    # Page 2
```

Switch session mid-conversation:
```
/session feature-auth
```

## Output Formats

| Format | Use Case |
|--------|----------|
| Interactive (default) | Terminal with colors, markdown, animations |
| `--format plain` | Piped output, scripts |
| `--format jsonl` | Structured JSON Lines for automation |

## Daemon Mode

Keep a session alive in the background:

```bash
octomind run --name ci-watcher --daemon --format jsonl
```

Send messages to it:
```bash
echo "Check build status" | octomind send --name ci-watcher
```

See [Daemon and Hooks](../integration/03-daemon-and-hooks.md) for webhook integration.

## Session Commands

All 23 commands available at the session prompt. See [Session Commands Reference](../reference/02-session-commands.md) for details.

**Session management:** `/help`, `/exit`, `/list`, `/session`, `/clear`

**Monitoring:** `/info`, `/report`, `/model`, `/role`, `/loglevel`

**Context:** `/context`, `/summarize`, `/truncate`, `/done`

**Media:** `/image`, `/video`, `/copy`

**MCP & tools:** `/mcp`

**Commands & workflows:** `/run`, `/workflow`, `/prompt`, `/plan`

## Cost Monitoring

Track token usage and spending:

```
/info
```

Shows:
- Token counts (input, output, cached, reasoning)
- Cost per request and cumulative
- Cache savings
- Compression statistics

Set spending limits in config:
```toml
max_session_spending_threshold = 5.0   # USD per session
max_request_spending_threshold = 1.0   # USD per request
```

## Multimodal (Vision)

Attach images for AI analysis:

```
/image screenshot.png
/image /path/to/diagram.jpg
```

Supported formats: PNG, JPEG, GIF, WebP. Vision support depends on the current model. Use `/model` to check or switch to a vision-capable model.

Attach videos:
```
/video demo.mp4
```

## Context Management

As sessions grow, manage context to control costs:

| Command | Effect |
|---------|--------|
| `/summarize` | AI-powered compression of history |
| `/truncate` | Remove oldest messages |
| `/done` | Complete task with cleanup and summary |
| `/context` | View current context |
| `/context large` | Show only large messages |

Automatic compression is also available. See [Compression](08-compression.md).

## Custom Instructions

Octomind auto-loads project files into sessions:

- **`INSTRUCTIONS.md`** -- loaded as a user message at session start
- **`CONSTRAINTS.md`** -- appended to every user request in `<constraints>` tags

Configure in `config.toml`:
```toml
custom_instructions_file_name = "INSTRUCTIONS.md"
custom_constraints_file_name = "CONSTRAINTS.md"
```

## Session Storage

Sessions are stored in `~/.local/share/octomind/sessions/`. Each session is a JSON file containing the full conversation history, tool calls, and metadata.
