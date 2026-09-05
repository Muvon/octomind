# Session Commands Reference

Use this reference while operating a CLI session or implementing an ACP/WebSocket client. It lists every registered
slash command, its arguments and output, and the transport differences.

## Get started

```bash
octomind run
```

Type one command per input. Most commands split on whitespace; `/schedule` separately supports quoted `key=value`
arguments. Do not paste shell comments after slash commands: they become arguments.

```text
/help
/model
/role
/info
```

## Command Summary

| Command | Purpose |
|---------|---------|
| `/?` | Autocomplete-only entry; not dispatched as help (see transport notes) |
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
| `/share` | Upload the session log and print the returned share URL |
| `/analyze` | Open the session in the web viewer locally, without uploading |
| `/copy` | Copy the last assistant response to the clipboard |
| `/model [MODEL]` | Show or switch the model (runtime + session file) |
| `/role [ROLE]` | Show or switch the role |
| `/effort [LEVEL]` | Show or set reasoning effort (runtime + session file) |
| `/loglevel [LEVEL]` | Set the log level (runtime only) |
| `/context [FILTER]` | Inspect the conversation context |
| `/done [INSTRUCTIONS]` | Force-compress context and extract lessons |
| `/image [PATH]` | Attach an image (from path or clipboard) |
| `/video [PATH]` | Attach a video |
| `/mcp [ACTION]` | Inspect MCP servers and tools |
| `/run [COMMAND] [INPUT...]` | Run a custom command from `[[commands]]` config |
| `/prompt [NAME]` | Inject a prompt template from `[[prompts]]` config |
| `/plan [show]` | Show the current structured plan |
| `/skill [NAME\|PAGE\|PATTERN]` | List or toggle skills |
| `/schedule [SUBCOMMAND]` | Schedule a future/recurring injected message |
| `/learning [ACTION]` | Manage cross-session lessons |

## Transport support

Every command in the summary except `/?` reaches the shared dispatcher through CLI input, ACP prompts or
`_octomind/command`, and WebSocket `type: "command"` messages. The table below lists exceptions to identical behavior;
all remaining commands execute their shared handler on the server and return its typed output. The CLI renders that
output as terminal text. ACP prompts send it as JSON text in an agent message; ACP extensions return `success`,
`output`, and `error`. WebSocket returns `status.data` with `command_type`. A handled output may itself contain an error
or `success: false`; inspect it, not just the transport envelope.

| Commands | CLI | ACP | WebSocket command |
|---|---|---|---|
| `/?` | Falls through as model input, not help. | Unsupported-command reply / extension error. | Unknown-command error. |
| `/exit`, `/quit` | Exit loop. | Exit request text / `{ "action": "exit" }`; session remains registered. | Save and remove session; connection stays open. |
| `/new [TITLE]` | Reinitialize a fresh session. | Sets a new name and returns an exit request; use ACP session creation for a fresh conversation. | Sets a new name, saves, and removes session; create another session to continue. |
| `/clear` | Renderer clears terminal. | Returns success; client decides how to clear UI. | Returns success; client decides how to clear UI. |
| `/copy`, `/image`, `/video` | Use local clipboard/files. | Use the agent host clipboard/files, not client files. | Use the server host clipboard/files, not browser files. |
| `/done [INSTRUCTIONS]` | Compress, then process instructions if supplied. | Prompt path continues instructions; extension only compresses and returns no output. | Compress status, then process joined args as a message. |
| `/analyze` | Prints loopback bridge URL. | URL points to agent host loopback. | URL points to server host loopback. |

ACP discovery currently advertises removed `session`, `workflow`, and `agents` entries and omits some working commands.
Use this inventory and the dispatcher, not discovery descriptions, as the command contract.

For an existing session whose ID is `demo`, these are complete command envelopes:

```json
{"jsonrpc":"2.0","id":1,"method":"_octomind/command","params":{"session_id":"demo","command":"/context","args":["tool"]}}
```

```json
{"type":"command","session_id":"demo","command":"context","args":["tool"]}
```

Replace `demo` with the ID returned by session creation. ACP includes the slash; WebSocket adds it. See [ACP
Protocol](../integration/02-acp-protocol.md) and [WebSocket Server](../integration/01-websocket-server.md) for
connection and session creation.

## Configure the current session

### `/model [MODEL]`

Show or change the current model. Without argument, displays the current model. With argument, switches to the specified
model in `provider:model` format. The change is **runtime + saved to the session file** — it does not modify your global
config.

```text
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

```text
/role developer:general
/role assistant:concierge
/role assistant
```

> The default config ships the roles `assistant`, `task_refiner`, `task_researcher`, and `reduce`. There is no built-in `developer` role — `developer:general` above is a tap agent tag.

### `/effort [LEVEL]`

Show or change the reasoning effort level. Without argument, displays the current level. With argument, sets the effort
to one of: `low`, `medium`, `high`, `xhigh`, `max`. The change is **saved to the session file** (not global config) and
is ignored by non-thinking models.

```text
/effort high
/effort max
```

### `/loglevel [LEVEL]`

Without an argument, show the current level and valid options. With an argument, change the log level. Options: `none`,
`info`, `debug`. This is **runtime-only** — it is never written to disk.

```text
/loglevel debug
```

Debug output favors one compact event per provider response, usage update, and tool dispatch. Tool parameters are
serialized on one line and capped at 200 tokens; routine animation transitions and full raw provider responses are not
printed. Learning recall is the deliberate exception: its bounded final Active Memory Pack is printed exactly so
injection correctness can be inspected.

## Session Management

### `/help`

Show 27 built-ins with descriptions plus custom `/run` commands. `/usage` and `/login` are valid commands but are absent
from the terminal-rendered list.

> **Note:** `/?` appears in autocomplete but is **not wired into the command dispatcher** — typing it falls through as user input in the CLI. Only `/help` shows help.

```text
/help
```

### `/exit` / `/quit`

Exit the current session. `/quit` is an alias of `/exit`.

```text
/quit
```

### `/list [PAGE]`

List saved session names, titles, creation times, models, token totals, costs, and current-session markers. Pages
contain 15 sessions; the default is 1. Zero, invalid, or out-of-range pages return an error.

```text
/list 1
```

### `/new [TITLE]`

- `/new` (no argument) creates a **new** session with a generated name in the same format as `octomind run`: `YYMMDD-basename-HHMM-uuid4short`.
- `/new <title...>` creates a new session and sets the given title (same as `/rename`). The title may contain spaces (all arguments are joined).

This command does **not** display current session info — use [`/info`](#info) for that.

```text
/new Authentication review
```

### `/rename [TITLE]`

Set the current session's display title. Arguments are joined with spaces. Running `/rename` with no title clears it;
the underlying session name and log filename do not change.

```text
/rename Authentication review
/rename
```

### `/clear`

Clear the terminal screen.

```text
/clear
```

## Information & Monitoring

### `/status [agents [ID]|monitors|jobs]`

The single process-activity surface for the current session. The old `/agents` and `/monitor` commands have been
removed.

| Usage | Description |
|-------|-------------|
| `/status` | Concise active-only view across agents, MCP background jobs, and command monitors |
| `/status agents` | Full agent view: running work plus recent completed, failed, and cancelled tap runs; preserves model, token, cache, and cost data when available |
| `/status agents <id>` | Detailed card for one tap or async `agent_*` run |
| `/status monitors` | Full configuration and elapsed time for active command monitors |
| `/status jobs` | Full live status and bounded output for active MCP resource-backed jobs |

`agents` merges tap specialists with asynchronous `agent_*` calls. Tap runs carry live or persisted usage accounting;
async `agent_*` cost is explicitly shown as not tracked rather than guessed. `jobs` is generic across MCP servers: a
tool must return a standard `ResourceLink`; Octomind retains the originating server and treats the URI as opaque. The
full jobs view performs one bounded `resources/read` call per active resource. Completion remains event-driven via
`resources/updated` and is injected automatically.

All status state is process-local and session-scoped. A resumed process cannot reattach to work owned by the prior
process.

```text
/status
/status agents
/status monitors
/status jobs
```

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

```text
/info
```

### `/report`

Generate a detailed usage report for the session with per-request breakdown.

```text
/report
```

### `/usage`

Show spend windows, storage, and network usage for the signed-in Octomind account. This is account-level information;
`/info` is the current session's local accounting. When not signed in, the command returns a normal unsigned state
rather than failing.

```text
/usage
```

### `/login`

Start the Octomind browser-confirmed sign-in flow. In ACP-style clients it returns the verification URL and code
immediately while polling in the background; an already signed-in process reports the account without starting another
flow. Completion updates the stored OctoHub credential used by `octohub:auto`.

```text
/login
```

### `/share`

Upload the current session's JSONL log to the share endpoint and print the returned share URL pointing at the web viewer
(`octomind.run/r/<id>`). The uploaded content is the saved JSONL log, including recorded messages, tool activity, and
accounting.

The CLI **does not** open the URL automatically — clicking it is your choice.

```text
/share
```

Output contains the server-returned `url` and share `id`; their values are assigned by the share service.

Environment overrides:

- `OCTOMIND_SHARE_URL` — point `/share` and `/analyze` at a different host (defaults to `https://octomind.run`). Use this only when pointing at a self-hosted instance or a local dev server.

### `/analyze`

Open the current session in the web viewer **without uploading anything**. A tiny HTTP server is bound to `127.0.0.1` on
a random port; the printed URL points at `octomind.run/analyze?b=127.0.0.1:<port>&t=<token>` so the browser fetches the
JSONL directly from your machine.

The bridge:

- listens on loopback only — unreachable from other machines,
- gates session-data requests with a per-invocation 24-character token (reusable while that bridge lives) sent in the `X-Bridge-Token` header,
- aborts the previous bridge when `/analyze` is re-invoked (fresh port + fresh token each time),
- shuts down with the `octomind` process — there is no persistent state and no upload.

```text
/analyze
```

Output includes the viewer `url`, the random loopback `port`, and a token in structured output. Open the printed URL on
the same machine as Octomind.

Use `/analyze` for ephemeral, private review of an in-flight session; use `/share` when you want a share link to send to
someone else.

## Context Management

### `/context [FILTER]`

View session context (message history). Filters:

- `all` — Show all messages
- `assistant` — Only assistant messages
- `user` — Only user messages
- `tool` — Only messages with role `tool`; assistant tool-call messages require `assistant` or `all`
- `system` — Only system messages
- `large` — Only messages whose content exceeds 1000 bytes

An unrecognized filter value silently falls back to `all`.

```text
/context
/context tool
/context large
```

### `/done [INSTRUCTIONS]`

Force-compress the conversation context **bypassing all automatic threshold, cooldown, and cost guards**, then (when
`[supervisor.learning].enabled`) spawn fire-and-forget lesson extraction. Use it to manually reclaim context after
finishing a unit of work.

- The forced compression preserves no injected skills, including env-loaded ones.
- Lesson extraction runs in the background and stores lessons for the current role + project — see [Learning Guide](../usage/13-learning.md).
- It does **not** touch the active plan or auto-commit; enabled lesson extraction may write grounded learning records asynchronously.

```text
/done
/done Now review the error handling.
```

## Media

### `/image [PATH]`

Attach an image for AI analysis. With a path, attaches the image file at that path; without a path, attaches an image
from the clipboard (no-op if the clipboard holds no image). Requires a vision-capable model.

```text
/image screenshot.png
/image /path/to/diagram.jpg
/image
```

### `/video [PATH]`

Attach a video for AI analysis. Requires a video-capable model. With no path, it returns unattached status after
checking model support. Both media commands report attachment status, the path, and any error; attachment is sent with
your next message.

```text
/video demo.mp4
```

### `/copy`

Copy the last assistant response to the clipboard. Returns success and byte length, or `copied: false` when there is no
response or the host clipboard is unavailable.

```text
/copy
```

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

```text
/mcp full
/mcp health
/mcp validate
```

## Commands

### `/run [COMMAND] [INPUT...]`

Execute a custom command defined in the `[[commands]]` config section. Without argument, lists commands available to the
current role. Optional trailing text is the input; otherwise it uses the most recent real user task message. Returns the
command name, execution status, and layer result.

Before executing, `/run` runs the session and request spending checks. An interactive session-limit check can ask
whether to continue; a declined check, exceeded request limit, or check error prevents execution.

```text
/run reduce
/run reduce Summarize the current implementation decisions
```

> **Multi-step workflows** are no longer a session command. Use the external CLI instead: `octomind workflow <file.toml>` — see [Workflows](../usage/09-workflows.md).

### `/prompt [NAME]`

Inject a prompt template defined in the `[[prompts]]` config section into the session inbox; it is delivered
**verbatim** as a user message on the next loop iteration. Without argument, lists available prompts. There is currently
no template variable substitution.

```text
/prompt review
/prompt explain
```

### `/plan [ACTION]`

Display the runtime-owned structured task plan.

| Usage | Description |
|-------|-------------|
| `/plan` or `/plan show` | Show current plan with progress and critical knowledge retained from compression |

**Note**: `/plan` is display-only. The specialist has no plan mutation tool. For complex work it emits sparse hidden
signals alongside normal work; the external planner creates, advances, revises, and finalizes runtime plan state.
Focused work stays plan-free.

```text
/plan
```

### `/skill [NAME|PAGE|PATTERN]`

Manage skills from taps. Skills are reusable instruction packs that inject domain knowledge into context.

| Usage | Description |
|-------|-------------|
| `/skill` | List all skills (active first, then alphabetical), 15 per page |
| `/skill <name>` | Toggle the skill: enable it if inactive (`use`), disable it if active (`forget`). Unknown names return `Skill not found`. |
| `/skill <page>` | Show page N of the skill list |
| `/skill *pattern*` | Filter skills by glob pattern |

```text
/skill
/skill 1
/skill *review*
```

Use an exact listed skill name to toggle it, for example:

```text
/skill code-review
```

This example requires `code-review` to appear in your skill list.

### `/schedule [SUBCOMMAND] [ARGS]`

Direct control over the built-in `schedule` MCP tool — schedule a message to be injected as a user message at a future
time or on the next idle. Same operations as the MCP tool, but driven from chat input. See [Scheduled
Tasks](../use-cases/07-scheduled-tasks.md) for the broader use case.

| Usage | Description |
|-------|-------------|
| `/schedule` or `/schedule list` | List all pending entries with IDs, trigger times, and countdown |
| `/schedule remove <id>` | Cancel a scheduled entry (aliases: `rm`, `delete`, `del`) |
| `/schedule add message="<text>"` | Schedule a one-shot for the next idle (default `when="idle"`) |
| `/schedule add when="<when>" message="<text>" [every="<interval>"] [description="<label>"]` | Schedule a new entry |
| `/schedule edit <id> [when="..."] [message="..."] [every="..."] [description="..."]` | Update an existing entry (use `every="none"` to clear a repeat) |
| `/schedule help` or `/schedule ?` | Show inline usage |
| `/schedule message="Check progress"` | Shorthand for `add`; initial `key=value` implies addition. |

Key=value tokens accept shell-style quoting so multi-word values work: `when="in 1h 30m"`, `message='hello world'`.
Supported `when` formats: `idle` (fires on next idle — no running taps or background jobs), `now` (fires immediately),
relative (`in 5m`, `in 1h30m`, `in 90s`), time-of-day (`15:30`, `3:30pm`, `9am` — tomorrow if past), or absolute
(`2030-03-30 15:30`). `every` accepts `idle` (fires on every idle) or the same duration syntax as relative `when`
(`10m`, `1h`, `1h30m`). When both `when` and `every` are omitted on `add`, `when` defaults to `idle`.

Examples:

```text
/schedule add message="summarize what we just did"
/schedule add when="idle" message="run lint and report results"
/schedule add every="idle" message="remind me to commit"
/schedule add when="now" message="say the date" every="5m"
/schedule add when="in 5m" message="check the build"
/schedule add when="9am" message="standup reminder" every="1h" description="daily"
/schedule edit abc12345 when="in 1h"
/schedule remove abc12345
```

Replace `abc12345` with an ID returned by `/schedule list`. The schedule response includes an ID or error message for
each operation; entries live in the current process/session.

### `/learning [ACTION]`

Browse and manage the lessons stored for the current role + project by the cross-session learning system. See [Learning
Guide](../usage/13-learning.md) for full details.

| Usage | Description |
|-------|-------------|
| `/learning` or `/learning list` | List project/domain and global hot memories plus hot/cold retention totals, 15 per page |
| `/learning list <page>` | Show page N of the lesson list |
| `/learning list *pattern*` | Glob-filter lessons by content, title, or tags (combinable with a page number) |
| `/learning show <index>` | Show full content, provenance, relationships, outcome, use metadata, and storage path (alias: `get`) |
| `/learning delete <index>` | Delete the lesson at the 1-based `<index>` in the current unfiltered combined list, not the last filtered page (aliases: `rm`, `remove`) |
| `/learning clear` | Delete current project/domain hot memories and its cold archive; preserve global rules |
| `/learning evolution [list]` | List generated behavior records matching the current project/domain |
| `/learning evolution show <id>` | Inspect one record and its native artifact |
| `/learning evolution approve <id>` | Authorize a shadow candidate for a bounded live trial. |
| `/learning evolution reject <id>` | Mark a generated record rejected. |
| `/learning evolution rollback <id>` | Return a trial or active behavior to shadow and reset counters. |

Any other subcommand returns an error listing `list`, `show`, `delete`, `clear`, and `evolution`.

```text
/learning
/learning list 2
/learning list *commit*
/learning delete 3
/learning clear
```

Use the ID returned by `/learning evolution list` for lifecycle actions:

```text
/learning list
/learning show 1
/learning evolution list
/learning evolution show EVOLUTION_ID
/learning evolution approve EVOLUTION_ID
/learning evolution reject EVOLUTION_ID
/learning evolution rollback EVOLUTION_ID
```

`EVOLUTION_ID` is a user-supplied existing record ID; choose the lifecycle action appropriate to that record.

## Common questions

- **Why is the wrong learning item selected?** `show` and `delete` rebuild the unfiltered list. Run
  `/learning list` without a pattern immediately before selecting an index.
- **Why is media not sent yet?** Attachments are queued for the next user message; send text after attaching.
- **Why does copy or analyze fail remotely?** Clipboard and loopback access belong to the Octomind host. Use
  your client attachment/clipboard UI, or `/share` when uploading the log is intended.
- **Why does `/plan show` not change a task?** Plan arguments are ignored: this command only displays runtime state.

## Source map

Inventory: [commands.rs](../../src/session/chat/commands.rs). Dispatch and output variants:
[commands/mod.rs](../../src/session/chat/session/commands/mod.rs); individual handlers live beside it. Transport
adapters: [main_loop.rs](../../src/session/chat/session/main_loop.rs), [acp/commands.rs](../../src/acp/commands.rs),
[acp/agent.rs](../../src/acp/agent.rs), [websocket/server.rs](../../src/websocket/server.rs).

## See also

- [CLI Reference](01-cli-reference.md)
- [Config Reference](03-config-reference.md)
- [Environment Variables](04-environment-variables.md)
- [Learning Guide](../usage/13-learning.md)
