# Session Commands Reference

Reference for interactive session commands covering lifecycle, models, tools, automation, learning, sharing, and account access.

## Command Summary

| Command | Purpose |
|---------|---------|
| `/help` | Show the terminal help list plus custom `/run` commands |
| `/exit` (`/quit`) | Exit the session |
| `/clear` | Clear the terminal screen |
| `/list [PAGE]` | List saved sessions |
| `/new [TITLE]` | Start a fresh session with unified naming (optional title) |
| `/rename [TITLE]` | Set or clear the current session title |
| `/info` | Show session statistics (tokens, cost, cache, compression) |
| `/status [agents [ID]\|monitors\|jobs]` | Show current process activity for this session |
| `/report` | Detailed per-request usage report |
| `/usage` | Show Octomind account usage, quotas, and balance |
| `/login` | Sign in to an Octomind account |
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
Show 27 built-ins with descriptions plus custom `/run` commands. `/usage` and `/login` are valid commands but are absent from the terminal-rendered list.

> **Note:** `/?` appears in autocomplete but is **not wired into the command dispatcher** — typing it sends `?` to the model as user input. Only `/help` shows help.

### `/exit` / `/quit`
Exit the current session. `/quit` is an alias of `/exit`.

### `/list [PAGE]`
List all saved sessions. Optional page number for pagination.

### `/new [TITLE]`
- `/new` (no argument) creates a **new** session with a generated name in the same format as `octomind run`: `YYMMDD-basename-HHMM-uuid4short`.
- `/new <title...>` creates a new session and sets the given title (same as `/rename`). The title may contain spaces (all arguments are joined).

This command does **not** display current session info — use [`/info`](#info) for that.

### `/rename [TITLE]`

Set the current session's display title. Arguments are joined with spaces. Running `/rename` with no title clears it; the underlying session name and log filename do not change.

### `/clear`
Clear the terminal screen.

## Information & Monitoring

### `/status [agents [ID]|monitors|jobs]`

The single process-activity surface for the current session. The old `/agents`
and `/monitor` commands have been removed.

| Usage | Description |
|-------|-------------|
| `/status` | Concise active-only view across agents, MCP background jobs, and command monitors |
| `/status agents` | Full agent view: running work plus recent completed, failed, and cancelled tap runs; preserves model, token, cache, and cost data when available |
| `/status agents <id>` | Detailed card for one tap or async `agent_*` run |
| `/status monitors` | Full configuration and elapsed time for active command monitors |
| `/status jobs` | Full live status and bounded output for active MCP resource-backed jobs |

`agents` merges tap specialists with asynchronous `agent_*` calls. Tap runs
carry live or persisted usage accounting; async `agent_*` cost is explicitly
shown as not tracked rather than guessed. `jobs` is generic across MCP servers: a tool
must return a standard `ResourceLink`; Octomind retains the originating server
and treats the URI as opaque. The full jobs view performs one bounded
`resources/read` call per active resource. Completion remains event-driven via
`resources/updated` and is injected automatically.

All status state is process-local and session-scoped. A resumed process cannot
reattach to work owned by the prior process.

### `/info`
Display comprehensive session statistics:
- Token usage (input, output, cached, reasoning)
- Cost breakdown (per-request and cumulative)
- Cache savings (tokens and accounting estimate)
- Compression statistics (if compression has occurred)
- Learning packs, items/tokens shown, materially used memories, outcome credit,
  active-pack state, and maintenance activity. Cumulative learning usage is
  persisted with the named session and survives resume.
- Model information

### `/report`
Generate a detailed usage report for the session with per-request breakdown.

### `/usage`

Show spend windows, storage, and network usage for the signed-in Octomind account. This is account-level information; `/info` is the current session's local accounting. When not signed in, the command returns a normal unsigned state rather than failing.

### `/login`

Start the Octomind browser-confirmed sign-in flow. In ACP-style clients it returns the verification URL and code immediately while polling in the background; an already signed-in process reports the account without starting another flow. Completion updates the stored OctoHub credential used by `octohub:auto`.

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
/model openai:gpt-5.6-sol
/model anthropic:claude-sonnet-4-6
/model octohub:auto
```

### `/role [ROLE]`
Show or change the current role. Without argument, displays the current role.

The argument is either:
- a **plain role name** defined in your config's `[[roles]]` (validated up front; an unknown name is rejected with `Invalid role`), or
- a **tap agent tag** in `domain:spec` form (e.g. `developer:general`), which resolves the manifest, INPUT/ENV placeholders, and dependency scripts.

On success the session is saved; on failure the previous role and complete resolved model profile are restored.

```
/role developer:general
/role assistant:concierge
/role assistant            # explicit local [[roles]] entry
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

Debug output favors one compact event per provider response, usage update, and
tool dispatch. Tool parameters are serialized on one line and capped at 200
tokens; routine animation transitions and full raw provider responses are not
printed. Learning recall is the deliberate exception: its bounded final Active
Memory Pack is printed exactly so injection correctness can be inspected.

## Context Management

### `/context [FILTER]`
View session context (message history). Filters:
- `all` — Show all messages
- `assistant` — Only assistant messages
- `user` — Only user messages
- `tool` — Only tool calls and results
- `system` — Only system messages
- `large` — Only messages whose content exceeds 1000 bytes

An unrecognized filter value silently falls back to `all`.

```
/context
/context tool
/context large
```

## Lifecycle

### `/done`
Force-compress the conversation context **bypassing all automatic threshold, cooldown, and cost guards**, then (when `[supervisor.learning].enabled`) spawn fire-and-forget lesson extraction. Use it to manually reclaim context after finishing a unit of work.

- The forced compression preserves no injected skills, including env-loaded ones.
- Lesson extraction runs in the background and stores lessons for the current role + project — see [Learning Guide](../usage/13-learning.md).
- It does **not** touch the active plan or auto-commit; enabled lesson extraction may write grounded learning records asynchronously.

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

Display the runtime-owned structured task plan.

| Usage | Description |
|-------|-------------|
| `/plan` or `/plan show` | Show current plan with progress |

**Note**: `/plan` is display-only. The specialist has no plan mutation tool. For complex work it emits sparse hidden signals alongside normal work; the external planner creates, advances, revises, and finalizes runtime plan state. Focused work stays plan-free.
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

Key=value tokens accept shell-style quoting so multi-word values work: `when="in 1h 30m"`, `message='hello world'`. Supported `when` formats: `idle` (fires on next idle — no running taps or background jobs), `now` (fires immediately), relative (`in 5m`, `in 1h30m`, `in 90s`), time-of-day (`15:30`, `3:30pm`, `9am` — tomorrow if past), or absolute (`2030-03-30 15:30`). `every` accepts `idle` (fires on every idle) or the same duration syntax as relative `when` (`10m`, `1h`, `1h30m`). When both `when` and `every` are omitted on `add`, `when` defaults to `idle`.

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
| `/learning` or `/learning list` | List lessons plus hot/cold retention totals for the current role + project, 15 per page |
| `/learning list <page>` | Show page N of the lesson list |
| `/learning list *pattern*` | Glob-filter lessons by content, title, or tags (combinable with a page number) |
| `/learning show <index>` | Show full content, provenance, relationships, outcome, use metadata, and storage path (alias: `get`) |
| `/learning delete <index>` | Delete the lesson at the 1-based `<index>` from the last list (aliases: `rm`, `remove`) |
| `/learning clear` | Delete **all** lessons for the current role + project |
| `/learning evolution [list]` | List generated behavior records matching the current project/domain |
| `/learning evolution show <id>` | Inspect one record and its native artifact |
| `/learning evolution approve\|reject\|rollback <id>` | Control a generated behavior lifecycle |

Any other subcommand returns an error listing `list`, `show`, `delete`, `clear`, and `evolution`.

```
/learning
/learning list 2
/learning list *commit*
/learning delete 3
/learning clear
```
