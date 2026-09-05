# Sessions

Start, resume, and inspect Octomind conversations from the terminal. This guide also covers piped runs, background
operation, saved settings, and entry points for ACP and WebSocket clients.

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

With no argument, `octomind run` uses the configured default tag, `assistant:concierge`. Bare names such as `assistant`
refer to roles in the shipped config; `category:variant` names such as `developer:general` are tap agents resolved from
the registry.

## Resuming Sessions

```bash
# Resume by name
octomind run --resume feature-auth

# Resume most recent
octomind run --resume-recent

# Pick a recent session interactively; Escape starts a new one
octomind run --resume
```

`--name feature-auth` resumes that session if it already exists. Without an explicit tag, a resumed session keeps its
saved role; an explicit tag selects a different role. `--resume-recent` searches the current working directory's
sessions. Bare `--resume` requires an interactive terminal; scripts must provide a session name.

List saved sessions, 15 per page:

```text
/list
/list 2
```

Start a fresh session mid-conversation:

```text
/new
/new Auth Refactor
```

`/new Auth Refactor` gives the new session a display title. To retitle the current session without changing the name
used by `--resume` or `send`, use `/rename`; no argument clears the title:

```text
/rename Auth Refactor
/rename
```

## Output Formats

| Mode | Behavior |
|------|----------|
| Interactive (no `--format`, TTY) | Terminal session with colors, markdown, and animations |
| `--format plain` | Reads a prompt from stdin and runs non-interactively; piped stdin selects non-interactive rendering |
| `--format jsonl` | Runs non-interactively and always emits structured JSON Lines, regardless of TTY. Ideal for automation. |

Pipe a nonempty prompt, rather than passing it as a positional argument:

```bash
printf '%s\n' 'Explain the authentication module.' | octomind run developer:general --format plain
printf '%s\n' 'List the project entry points.' | octomind run --format jsonl > session-events.jsonl
```

Piped stdin also selects non-interactive operation without `--format`. JSONL is an event stream, not a single JSON
result. Plain-mode terminal detection uses stdin; it is not an unconditional color-stripping flag.

For schema-constrained final responses, see [Structured Output](11-structured-output.md) for `--schema` examples.

## Daemon Mode

Keep a session alive in the background so other processes can inject messages into it:

```bash
printf '%s\n' 'Wait for further instructions.' | \
  octomind run --name ci-watcher --daemon --format jsonl > ci-watcher.jsonl 2> ci-watcher.err &
```

Send messages to it with `octomind send`:

```bash
echo "Check build status" | octomind send --name ci-watcher
octomind send --name ci-watcher "Summarize your progress."
```

The shell's `&` backgrounds this process; `--daemon` keeps it listening after a turn. Wait for startup to finish before
sending. A successful `send` acknowledges message delivery, not completion of the model's work. Non-TTY stdin must
contain a prompt even in daemon mode; `/dev/null` produces an empty-input error.

With terminal stdin, `--daemon --format jsonl` can start without an initial prompt. `--daemon` without `--format` on a
terminal enters the interactive input path. For unattended use, prefer the explicit piped example above.

See [Daemon and Hooks](../integration/03-daemon-and-hooks.md) for webhook integration.

## Session Commands

Recognized slash commands dispatch to session handlers. Some, such as `/done`, `/run`, and `/prompt`, can invoke models.
Unknown commands are treated as ordinary user input. See the [Session Commands
Reference](../reference/02-session-commands.md) for detailed arguments.

| Surface | Commands and concrete examples |
|---------|--------------------------------|
| Lifecycle | `/help`, `/exit`, `/quit`, `/clear`, `/list 2`, `/new Auth Refactor`, `/rename Auth Refactor` |
| Monitoring | `/status`, `/info`, `/report`, `/loglevel debug` |
| Model and behavior | `/model octohub:auto`, `/role assistant`, `/effort high`, `/prompt` |
| Context and compression | `/done`, `/context tool`, `/context large` |
| Media and clipboard | `/image screenshot.png`, `/video demo.mp4`, `/copy` |
| Tools and planning | `/mcp`, `/run`, `/plan`, `/skill`, `/schedule` |
| Account, learning, and viewing | `/usage`, `/login`, `/learning list`, `/share`, `/analyze` |

`/?` is registered for completion but is not dispatched as help; use `/help`.

`/status` is the activity dashboard: the default view is concise and active-only across agents, MCP background jobs, and
command monitors. Use `/status agents`, `/status monitors`, or `/status jobs` for the full category view. Status is
scoped to the current process and session.

For example, inspect available tools, the supervisor-managed plan, and background activity:

```text
/mcp
/plan
/status agents
/status monitors
/status jobs
```

`/run`, `/prompt`, `/skill`, and `/schedule` with no arguments list their available entries. `/plan` inspects the plan;
the supervisor owns plan updates. `/share` uploads the log for viewing. `/analyze` starts a localhost bridge for the
browser viewer without uploading the log through the share endpoint.

There is no `/workflow` session command. See [Workflows](09-workflows.md) for stdin-driven CLI examples.

## Cost Monitoring

Track token usage and spending:

```text
/info
/report
```

`/info` shows:

- Token counts (input, output, cached, reasoning)
- Cumulative session cost; `/report` supplies the detailed usage breakdown
- Estimated cache savings
- Per-tool, per-response, per-request (input), and per-compression token averages (each shown only when nonzero)
- Cache marker stats (system / tool / content markers, non-cached tokens)
- Compression statistics (when any compression has happened)
- Request/turn timings, learning usage, and available agent/supervisor statistics

Supervisor counters are process-global and reset on restart; concurrent ACP/WebSocket sessions can mix those counters.
They are separate from persisted session totals.

Edit these root keys before any table header to set USD thresholds (both default to `0.0`, disabled):

```toml
max_session_spending_threshold = 5.0
max_request_spending_threshold = 1.0
```

The session threshold asks a terminal user whether to continue and resets its checkpoint on acceptance; piped input and
ACP/WebSocket decline automatically. The request threshold stops further work for the current request. These checks use
recorded spend, so a provider call can cross a threshold before the next check.

## Adjust Model and Behavior

A few commands change runtime settings without touching your global config:

- `/model <provider:model>` switches the active model and **saves it into the session file**, so resuming restores it.
  It does not change your global config.
- `/effort <level>` sets the reasoning effort for the session (`low`, `medium`, `high`, `xhigh`, `max`) and also saves
  it to the session file. It mirrors the `reasoning_effort` config field and is ignored by non-thinking models. See
  [Configuration](03-configuration.md).
- `/loglevel <none|info|debug>` changes logging verbosity for the running session only. It is **never** saved to the
  session file or global config.

```text
/model octohub:auto
/effort high
/loglevel debug
```

## Multimodal (Vision)

Attach images for AI analysis:

```text
/image screenshot.png
Explain the error shown in this screenshot.
```

Use an existing image path. With an image copied to the system clipboard, attach it without a path:

```text
/image
Describe this image.
```

Supported image formats: PNG, JPEG, GIF, WebP. Images larger than 5 MiB are rejected, and images are automatically
resized to fit within 1568x1568.

Attach videos:

```text
/video demo.mp4
Summarize the actions in this video.
```

Use an existing video path. `/video` requires a path; no argument attaches nothing after the model capability check.
Supported video formats: mp4, mov, avi, webm, mkv, m4v, 3gp. Videos larger than 100 MiB are rejected. Interactive
`Ctrl+V` can also attach a copied video file or clipboard image.

Attachments are queued onto your **next** message rather than sent immediately, and vision/video support depends on the
active model. Use `/model` to check or switch to a vision-capable model.

## Context Management

As sessions grow, manage context to control costs:

| Command | Effect |
|---------|--------|
| `/done` | Force context compression; start lesson extraction when learning is enabled |
| `/context` | View current context (same as `/context all`) |
| `/context all` | Show all messages |
| `/context assistant` | Show only assistant messages |
| `/context user` | Show only user messages |
| `/context tool` | Show only tool messages |
| `/context system` | Show only system messages |
| `/context large` | Show messages whose content exceeds 1000 UTF-8 bytes |

An unrecognized filter silently falls back to showing all messages.

Automatic compression also runs as sessions grow. See [Compression](08-compression.md).

```text
/context large
/done
/learning list
```

`/done` bypasses automatic compression thresholds and keeps the session open. Lesson extraction uses the pre-compression
transcript and runs asynchronously; the learning list may not update immediately.

## Project Instructions

See [Configuration](03-configuration.md#project-instructions-and-template-variables) for the `AGENTS.md` loader and a
project-instruction example.

## Connect an ACP or WebSocket Client

Configure an ACP client to launch this command over stdio:

```bash
octomind acp developer:general --name editor-session
```

ACP creates sessions when the client requests them. See [ACP Protocol](../integration/02-acp-protocol.md) for the client
lifecycle. To start a WebSocket server for a client:

```bash
octomind server developer:general --host 127.0.0.1 --port 8080
```

Browser clients must have their exact origin allowed, for example:

```bash
octomind server --allow-origin http://localhost:3000
```

See [WebSocket Server](../integration/01-websocket-server.md) for session and message JSON payloads. Command
availability and lifecycle effects depend on the transport; this guide's interactive transcripts target the CLI.

## Session Storage

Sessions are stored in `~/.local/share/octomind/sessions/` (on Windows, `%LOCALAPPDATA%\octomind\sessions\`). Each
session is an append-only, zstd-compressed JSONL log file named `<session_name>.jsonl.zst`. Every line is an independent
zstd frame recording conversation messages, tool calls, cost and token snapshots, and compression markers — so the file
grows as the session continues rather than being rewritten.

`OCTOMIND_DATA_DIR` changes the parent data directory. Runtime processes, monitors, and MCP connections must be
initialized again on restart; saving a conversation does not preserve running processes.

Auto-generated session names follow the pattern `YYMMDD-<project-basename>-HHMM-<uuid>`, where `<uuid>` is the first 4
characters of a UUID. A `--name` you pass replaces this generated name.

To inspect a saved session, resume it and open the local viewer:

```bash
octomind run --resume feature-auth
```

```text
/analyze
```

If you have `zstd` installed, decompress the log before passing it to text tools (Unix default path):

```bash
zstd -dc ~/.local/share/octomind/sessions/feature-auth.jsonl.zst
```

## Troubleshooting

**Why did the response stop?** Check `/info` for recorded spend and `/status` for active work. To cancel a current
operation, press `Ctrl+C`; to exit interactive input, use `/exit` or `Ctrl+D`.

**Why does `send` say no running session?** A saved session file is insufficient. Start or resume the named session
first, wait for initialization, and send from the same machine/user runtime environment.

**Why is an attachment rejected?** Check the path, size, and active model's image/video support. Attach first, then send
your text; attaching alone does not request an answer.

## See also

- [Roles](06-roles.md)
- [Session Commands Reference](../reference/02-session-commands.md)
- [Compression](08-compression.md)
- [Daemon and Hooks](../integration/03-daemon-and-hooks.md)
