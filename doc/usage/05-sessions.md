# Sessions

Sessions persist conversations, tools, runtime settings, and usage across interactive, piped, daemon, ACP, and WebSocket work.

## Starting Sessions

```bash
# Default tag (assistant:concierge)
octomind run

# A tap agent from the registry, addressed as category:variant
octomind run developer:general

# An explicit local [[roles]] entry from the shipped config
octomind run assistant

# Named session
octomind run --name feature-auth

# Main-purpose model override
octomind run -m anthropic:claude-sonnet-4-6
```

With no argument, `octomind run` uses the configured default tag, `assistant:concierge`. Bare names such as `assistant` refer to roles in the shipped config; `category:variant` names such as `developer:general` are tap agents resolved from the registry.

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

Start a fresh session mid-conversation:
```
/new
/new Auth Refactor    # with a title
```

## Output Formats

| Mode | Behavior |
|------|----------|
| Interactive (no `--format`, TTY) | Terminal session with colors, markdown, and animations |
| `--format plain` | Runs **non-interactively**, reading the prompt from stdin. Output is still styled (colors, markdown) when attached to a TTY and unstyled when piped. |
| `--format jsonl` | Runs non-interactively and always emits structured JSON Lines, regardless of TTY. Ideal for automation. |

The key effect of passing **any** `--format` value is that the session runs once, non-interactively, reading its prompt from stdin instead of the terminal. `plain` does not strip formatting by itself; piping does.

## Daemon Mode

Keep a session alive in the background so other processes can inject messages into it:

```bash
octomind run --name ci-watcher --daemon --format jsonl
```

Send messages to it with `octomind send`:
```bash
echo "Check build status" | octomind send --name ci-watcher
```

Daemon mode is meant to run non-interactively. Pair it with `--format jsonl` (or pipe stdin) so the session reads injected messages rather than the terminal. The `--format` pairing is a recommendation, not a hard requirement: a `--daemon` run attached to a TTY with no `--format` starts with an empty initial input and stays interactive-capable.

See [Daemon and Hooks](../integration/03-daemon-and-hooks.md) for webhook integration.

## Session Commands

Commands typed at the session prompt control the session without sending a message to the model. The grouped list below covers the full command surface; see the [Session Commands Reference](../reference/02-session-commands.md) for every flag and subcommand.

**Session lifecycle:** `/help`, `/exit` (alias `/quit`), `/clear`, `/list`, `/new`, `/rename`

`/?` is registered for completion but is not dispatched as help; use `/help`.

**Monitoring:** `/status`, `/info`, `/report`, `/context`, `/loglevel`

**Model & behavior:** `/model`, `/role`, `/effort`, `/prompt`

**Context & compression:** `/done`, `/context`

**Media:** `/image`, `/video`, `/copy`

**MCP, tools & capabilities:** `/mcp`, `/run`, `/plan`, `/skill`, `/schedule`

`/status` is the only activity dashboard: the default view is concise and
active-only across agents, MCP background jobs, and command monitors. Use
`/status agents`, `/status monitors`, or `/status jobs` for the full category
view. Status is scoped to the current process and session.

**Account, learning & sharing:** `/usage`, `/login`, `/learning`, `/share`, `/analyze`

`/share` uploads the current session log and opens a receipt URL; `/analyze` instead starts a localhost bridge and opens the octomind.run viewer pointed at your machine, so the log never leaves it.

There is no `/workflow` session command. Multi-step workflows run from the external CLI instead: `octomind workflow <file.toml>`.

## Cost Monitoring

Track token usage and spending:

```
/info
```

Shows:
- Token counts (input, output, cached, reasoning)
- Cost per request and cumulative
- Estimated cache savings
- Per-tool, per-response, per-request (input), and per-compression token averages (each shown only when nonzero)
- Cache marker stats (system / tool / content markers, non-cached tokens)
- Compression statistics (when any compression has happened)

Configure `max_session_spending_threshold` and `max_request_spending_threshold`
when you need session- or request-level spending stops; see the
[Configuration Reference](../reference/03-config-reference.md#user-interface).

### Adjusting model and behavior mid-session

A few commands change runtime settings without touching your global config:

- `/model <provider:model>` switches the active model and **saves it into the session file**, so resuming restores it. It does not change your global config.
- `/effort <level>` sets the reasoning effort for the session (`low`, `medium`, `high`, `xhigh`, `max`) and also saves it to the session file. It mirrors the `reasoning_effort` config field and is ignored by non-thinking models. See [Configuration](03-configuration.md).
- `/loglevel <none|info|debug>` changes logging verbosity for the running session only. It is **never** saved to the session file or global config.

## Multimodal (Vision)

Attach images for AI analysis:

```
/image screenshot.png
/image /path/to/diagram.jpg
/image                       # no argument: attach an image from the system clipboard
```

Supported image formats: PNG, JPEG, GIF, WebP. Images larger than 5 MB are rejected, and images are automatically resized to fit within 1568x1568.

Attach videos:
```
/video demo.mp4
```

`/video` requires a path; it has no clipboard support (running it with no argument is a no-op). Supported video formats: mp4, mov, avi, webm, mkv, m4v, 3gp. Videos larger than 100 MB are rejected.

Attachments are queued onto your **next** message rather than sent immediately, and vision/video support depends on the active model. Use `/model` to check or switch to a vision-capable model.

## Context Management

As sessions grow, manage context to control costs:

| Command | Effect |
|---------|--------|
| `/done` | Complete task with cleanup, forced compression, and summary |
| `/context` | View current context (same as `/context all`) |
| `/context all` | Show all messages |
| `/context assistant` | Show only assistant messages |
| `/context user` | Show only user messages |
| `/context tool` | Show only tool messages |
| `/context system` | Show only system messages |
| `/context large` | Show only large messages (content longer than 1000 characters) |

An unrecognized filter silently falls back to showing all messages.

Automatic compression also runs as sessions grow. See [Compression](08-compression.md).

## Project Instructions

Octomind auto-loads `AGENTS.md` from the session working directory as a user message at session start. `AGENTS.md` is the only project-instruction filename recognized by this loader; `INSTRUCTIONS.md` and `CONSTRAINTS.md` are not consulted. An empty `AGENTS.md` is ignored.

## Session Storage

Sessions are stored in `~/.local/share/octomind/sessions/` (on Windows, `%LOCALAPPDATA%\octomind\sessions\`). Each session is an append-only, zstd-compressed JSONL log file named `<session_name>.jsonl.zst`. Every line is an independent zstd frame recording conversation messages, tool calls, cost and token snapshots, and compression markers — so the file grows as the session continues rather than being rewritten.

Auto-generated session names follow the pattern `YYMMDD-<project-basename>-HHMM-<uuid>`, where `<uuid>` is the first 4 characters of a UUID. A `--name` you pass replaces this generated name.

Because the file is compressed, `cat` or `jq` will show binary, not text. To inspect a session, resume it (`octomind run --resume <name>`), use `/share` to upload it for viewing, or `/analyze` to open it in the local viewer.
