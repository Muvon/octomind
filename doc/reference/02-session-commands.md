# Session Commands Reference

All interactive session commands. Type the command at the prompt. There are 25 distinct commands; the autocomplete list also includes the aliases `/quit` (= `/exit`) and `/?` (a non-functional help alias — see [`/help`](#help)). To discover commands at runtime, type `/help`, which lists the built-ins plus any custom `/run` commands defined in your config.

## Command Summary

| Command | Purpose |
|---------|---------|
| `/help` | List all commands (built-ins plus custom `/run` commands) |
| `/exit` (`/quit`) | Exit the session |
| `/clear` | Clear the terminal screen |
| `/list [PAGE]` | List saved sessions |
| `/session [NAME]` | Create a new session, or switch to/create one by name |
| `/info` | Show session statistics (tokens, cost, cache, compression) |
| `/report` | Detailed per-request usage report |
| `/share` | Upload the session log and print a permanent share URL |
| `/analyze` | Open the session in the web viewer locally, without uploading |
| `/copy` | Copy the last assistant response to the clipboard |
| `/model [MODEL]` | Show or switch the model (runtime + session file) |
| `/role [ROLE]` | Show or switch the role |
| `/effort [LEVEL]` | Show or set reasoning effort (runtime + session file) |
| `/loglevel [LEVEL]` | Set the log level (runtime only) |
| `/context [FILTER]` | Inspect the conversation context |
| `/done` | Force-compress context and extract lessons |
| `/image [PATH]` | Attach an image (from path or clipboard) |
| `/video [PATH]` | Attach a video |
| `/mcp [ACTION]` | Inspect MCP servers and tools |
| `/run [COMMAND]` | Run a custom command from `[[commands]]` config |
| `/prompt [NAME]` | Inject a prompt template from `[[prompts]]` config |
| `/plan [show]` | Show the current structured plan |
| `/skill [NAME\|PAGE\|PATTERN]` | List or toggle skills |
| `/schedule [SUBCOMMAND]` | Schedule a future/recurring injected message |
| `/learning [ACTION]` | Manage cross-session lessons |

## Session Management

### `/help`
Show all available commands with descriptions, including any custom `/run` commands from your config.

> **Note:** `/?` appears in autocomplete but is **not wired into the command dispatcher** — typing it sends `?` to the model as user input. Only `/help` shows help.

### `/exit` / `/quit`
Exit the current session. `/quit` is an alias of `/exit`.

### `/list [PAGE]`
List all saved sessions. Optional page number for pagination.

### `/session [NAME]`
- `/session` (no argument) creates a **new** session named `session_<unix_timestamp>`.
- `/session <name...>` switches to a session with the given name, creating it if it does not exist. The name may contain spaces (all arguments are joined).

This command does **not** display current session info — use [`/info`](#info) for that.

### `/clear`
Clear the terminal screen.

## Information & Monitoring

### `/info`
Display comprehensive session statistics:
- Token usage (input, output, cached, reasoning)
- Cost breakdown (per-request and cumulative)
- Cache savings (tokens and USD)
- Compression statistics (if compression has occurred)
- Model information

### `/report`
Generate a detailed usage report for the session with per-request breakdown.

### `/share`
Upload the current session's JSONL log to the share endpoint and print a permanent URL pointing at the web viewer (`octomind.run/r/<id>`). The full forensic trace — every user/assistant turn, every tool call with args and results, every cost update, every compression/truncation marker — renders in the browser exactly as it occurred on disk.

The CLI **does not** open the URL automatically — clicking it is your choice.

```
/share
```

Output:
```
url    https://octomind.run/r/<8-char id>
id     <8-char id>
```

Environment overrides:
- `OCTOMIND_SHARE_URL` — point `/share` and `/analyze` at a different host (defaults to `https://octomind.run`). Use this only when pointing at a self-hosted instance or a local dev server.

### `/analyze`
Open the current session in the web viewer **without uploading anything**. A tiny HTTP server is bound to `127.0.0.1` on a random port; the printed URL points at `octomind.run/analyze?b=127.0.0.1:<port>&t=<token>` so the browser fetches the JSONL directly from your machine.

The bridge:
- listens on loopback only — unreachable from other machines,
- gates every request with a single-use 24-char token sent in the `X-Bridge-Token` header,
- aborts the previous bridge when `/analyze` is re-invoked (fresh port + fresh token each time),
- shuts down with the `octomind` process — there is no persistent state and no upload.

```
/analyze
```

Output:
```
url    https://octomind.run/analyze?b=127.0.0.1:<port>&t=<token>
port   127.0.0.1:<port> (loopback only)
```

Use `/analyze` for ephemeral, private review of an in-flight session; use `/share` when you want a permanent link to send to someone else.

### `/model [MODEL]`
Show or change the current model. Without argument, displays the current model. With argument, switches to the specified model in `provider:model` format. The change is **runtime + saved to the session file** — it does not modify your global config.

```
/model openai:gpt-4o
/model anthropic:claude-sonnet-4
```

### `/role [ROLE]`
Show or change the current role. Without argument, displays the current role.

The argument is either:
- a **plain role name** defined in your config's `[[roles]]` (validated up front; an unknown name is rejected with `Invalid role`), or
- a **tap agent tag** in `domain:spec` form (e.g. `developer:general`), which resolves the manifest, INPUT/ENV placeholders, and dependency scripts.

On success the session is saved; on failure the previous role/model/temperature are reverted.

```
/role assistant
/role developer:general
```

> The default config ships the roles `assistant`, `task_refiner`, `task_researcher`, and `reduce`. There is no built-in `developer` role — `developer:general` above is a tap agent tag.

### `/effort [LEVEL]`
Show or change the reasoning effort level. Without argument, displays the current level. With argument, sets the effort to one of: `low`, `medium`, `high`, `xhigh`, `max`. The change is **saved to the session file** (not global config) and is ignored by non-thinking models.

```
/effort high
/effort max
```

### `/loglevel [LEVEL]`
Change the log level. Options: `none`, `info`, `debug`. This is **runtime-only** — it is never written to disk.

```
/loglevel debug
```

## Context Management

### `/context [FILTER]`
View session context (message history). Filters:
- `all` -- Show all messages
- `assistant` -- Only assistant messages
- `user` -- Only user messages
- `tool` -- Only tool calls and results
- `system` -- Only system messages
- `large` -- Only messages whose content exceeds 1000 bytes

An unrecognized filter value silently falls back to `all`.

```
/context
/context tool
/context large
```

## Lifecycle

### `/done`
Force-compress the conversation context **bypassing all automatic threshold, cooldown, and cost guards**, then (when `[supervisor.learning].enabled`) spawn fire-and-forget lesson extraction. Use it to manually reclaim context after finishing a unit of work.

- The forced compression preserves only env-loaded skills, dropping manually activated ones.
- Lesson extraction runs in the background and stores lessons for the current role + project — see [Learning Guide](../usage/13-learning.md).
- It does **not** touch any active plan, write to memory, or auto-commit.

## Media

### `/image [PATH]`
Attach an image for AI analysis. With a path, attaches the image file at that path; without a path, attaches an image from the clipboard (no-op if the clipboard holds no image). Requires a vision-capable model.

```
/image screenshot.png
/image /path/to/diagram.jpg
/image            # attach from clipboard
```

### `/video [PATH]`
Attach a video for AI analysis. A path is required — invoking `/video` with no argument is a no-op.

```
/video demo.mp4
```

### `/copy`
Copy the last assistant response to the clipboard.

## MCP & Tools

### `/mcp [ACTION]`
Inspect MCP servers and their tools. The session `/mcp` command is **read-only**; it has exactly these six subcommands:

| Action | Description |
|--------|-------------|
| `/mcp` or `/mcp info` | Default: server status plus tools with short descriptions |
| `/mcp list` | Tool names grouped by server |
| `/mcp full` | Full tool details, including parameters |
| `/mcp health` | Force a health check on all servers |
| `/mcp dump` | Dump all tools with name, description, and parameter schemas |
| `/mcp validate` | Validate tool parameter schemas |

Any other subcommand returns `Invalid MCP subcommand`.

> Runtime server management — adding, enabling, disabling, or removing servers — is done by the `mcp` **MCP tool** (which the model can call), not by this slash command.

## Commands

### `/run [COMMAND]`
Execute a custom command defined in the `[[commands]]` config section. Without argument, lists available commands.

Before executing, `/run` checks both the **session** and **request** spending thresholds; if either is breached (or the check itself errors), execution is declined.

```
/run reduce
/run estimate
```

> **Multi-step workflows** are no longer a session command. Use the external CLI instead: `octomind workflow <file.toml>` — see [Workflows](../usage/09-workflows.md).
### `/prompt [NAME]`
Inject a prompt template defined in the `[[prompts]]` config section into the session inbox; it is delivered **verbatim** as a user message on the next loop iteration. Without argument, lists available prompts. There is currently no template variable substitution.

```
/prompt review
/prompt explain
```

### `/plan [ACTION]`

Manage the structured task plan.

| Usage | Description |
|-------|-------------|
| `/plan` or `/plan show` | Show current plan with progress |

**Note**: The `/plan` slash command only displays the current plan. To create, modify, or clear a plan, use the `plan` MCP tool directly with these commands:

- `plan(command="start", content="<plan goal/title>", tasks=[{title, description}, ...])` — Create a new plan. The plan title comes from `content` (defaults to `Active Plan` if omitted); each task object requires non-empty `title` and `description`.
- `plan(command="next", content="...")` — Mark current task complete, advance to next
- `plan(command="step", content="...")` — Add progress note to current task
- `plan(command="reset")` — **Clear/reset the current plan**
- `plan(command="done", content="...")` — Complete the plan with final summary
### `/skill [NAME|PAGE|PATTERN]`
Manage skills from taps. Skills are reusable instruction packs that inject domain knowledge into context.

| Usage | Description |
|-------|-------------|
| `/skill` | List all skills (active first, then alphabetical), 15 per page |
| `/skill <name>` | Toggle the skill: enable it if inactive (`use`), disable it if active (`forget`). Unknown names return `Skill not found`. |
| `/skill <page>` | Show page N of the skill list |
| `/skill *pattern*` | Filter skills by glob pattern |

### `/schedule [SUBCOMMAND] [ARGS]`
Direct control over the built-in `schedule` MCP tool — schedule a message to be injected as a user message at a future time or on the next idle. Same operations as the MCP tool, but driven from chat input. See [Scheduled Tasks](../use-cases/07-scheduled-tasks.md) for the broader use case.

| Usage | Description |
|-------|-------------|
| `/schedule` or `/schedule list` | List all pending entries with IDs, trigger times, and countdown |
| `/schedule remove <id>` | Cancel a scheduled entry (aliases: `rm`, `delete`, `del`) |
| `/schedule add message="<text>"` | Schedule a one-shot for the next idle (default `when="idle"`) |
| `/schedule add when="<when>" message="<text>" [every="<interval>"] [description="<label>"]` | Schedule a new entry |
| `/schedule edit <id> [when="..."] [message="..."] [every="..."] [description="..."]` | Update an existing entry (use `every="none"` to clear a repeat) |
| `/schedule help` | Show inline usage |

Key=value tokens accept shell-style quoting so multi-word values work: `when="in 1h 30m"`, `message='hello world'`. Supported `when` formats: `idle` (fires on next idle — no running taps or background jobs), `now` (fires immediately), relative (`in 5m`, `in 1h30m`, `in 90s`), time-of-day (`15:30`, `3:30pm`, `9am` — tomorrow if past), or absolute (`2026-03-30 15:30`). `every` accepts `idle` (fires on every idle) or the same duration syntax as relative `when` (`10m`, `1h`, `1h30m`). When both `when` and `every` are omitted on `add`, `when` defaults to `idle`.

Examples:
```
/schedule add message="summarize what we just did"             # default: when="idle"
/schedule add when="idle" message="run lint and report results"
/schedule add every="idle" message="remind me to commit"        # fires every idle
/schedule add when="now" message="say the date" every="5m"
/schedule add when="in 5m" message="check the build"
/schedule add when="9am" message="standup reminder" every="1h" description="daily"
/schedule edit abc12345 when="in 1h"
/schedule remove abc12345
```

### `/learning [ACTION]`

Browse and manage the lessons stored for the current role + project by the cross-session learning system. See [Learning Guide](../usage/13-learning.md) for full details.

| Usage | Description |
|-------|-------------|
| `/learning` or `/learning list` | List lessons for the current role + project, 15 per page |
| `/learning list <page>` | Show page N of the lesson list |
| `/learning list *pattern*` | Glob-filter lessons by content, title, or tags (combinable with a page number) |
| `/learning delete <index>` | Delete the lesson at the 1-based `<index>` from the last list (aliases: `rm`, `remove`) |
| `/learning clear` | Delete **all** lessons for the current role + project |

Any other subcommand returns `unknown subcommand — use: list, delete, clear`.

```
/learning
/learning list 2
/learning list *commit*
/learning delete 3
/learning clear
```
