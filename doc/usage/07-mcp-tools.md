# MCP Tools Reference

Use this reference to inspect and configure the tools available to your Octomind session. It covers built-in controls,
tap capabilities, external MCP servers, and project-local tools.

## Get Started

Start a session, then inspect its actual tool surface:

```bash
octomind run
```

```text
/mcp list
/mcp full
```

`/mcp full` shows the schemas advertised by your enabled servers. JSON examples below are arguments for the named MCP
tool, not slash commands; where several objects are shown, make one tool call per object.

## Architecture

Octomind ships four builtin MCP servers (`core`, `orchestration`, `runtime`, `agent`), plus an auto-discovered `local`
server for project scripts:

| Server | Type | Description |
|--------|------|-------------|
| `core` | builtin | Session-memory retrieval (`recall` when attention or governance is enabled; governance defaults on) |
| `orchestration` | builtin | Delegation (`tap`), scheduled messages (`schedule`), and event streams (`monitor`) |
| `runtime` | builtin | Harness reconfiguration: register MCP servers, manage dynamic agents, load skills, capability |
| `agent` | builtin | Delegates tasks to configured ACP sub-agents (each `[[agents]]` entry exposes an `agent_<name>` tool) |
| `local` | builtin | Project-local shebang-script tools auto-discovered from `<workdir>/.agents/tools/`. See [Local Tools](17-local-tools.md). |

Filesystem tools come from external servers such as `octofs`; they are not implemented by Octomind. Use your installed
capability manifests or an explicit server configuration to enable them.

Planning is supervisor-internal rather than an MCP tool. The specialist sees runtime-owned plan state and emits sparse
hidden signals alongside normal work; the external planner owns transitions. `/plan` only displays that state.

Additional servers can be added via `[[mcp.servers]]` config as `http` or `stdio` types.

## Configure External MCP Servers

### Adding HTTP Servers

Replace the endpoint with your server URL and set `CUSTOM_API_TOKEN` to its issued token before starting Octomind. Add
`auto_bind` or reference the server from your role so it becomes visible.

```toml
[[mcp.servers]]
name = "custom_api"
type = "http"
url = "https://api.example.com/mcp"
headers = { Authorization = "Bearer {{ENV:CUSTOM_API_TOKEN}}" }  # optional
timeout_seconds = 30
tools = []
```

`headers` is sent on every request. Values may use `{{ENV:KEY}}` placeholders; a server whose placeholders reference
unset or empty env vars cannot connect and is logged as a startup error. When an `Authorization` header is configured it is used as-is
and OAuth discovery is disabled for that server; without one, Octomind runs MCP Authorization Discovery (RFC 9728) and
attempts OAuth authentication when the server requires it.

Set the token issued by your server in the environment before launch:

```bash
export CUSTOM_API_TOKEN='token-issued-by-your-server'
octomind run
```

### Adding Stdio Servers

This example requires your Python MCP module `my_mcp_server` to be installed. `env` passes values to the child, and
`cwd` sets its working directory. Replace these deployment-specific values with your server settings.

```toml
[[mcp.servers]]
name = "custom_tools"
type = "stdio"
command = "python"
args = ["-m", "my_mcp_server"]
env = { API_TOKEN = "{{ENV:CUSTOM_API_TOKEN}}" }
cwd = "."
timeout_seconds = 30
tools = []
```

For tool calls, `timeout_seconds` is an idle deadline rather than a total runtime limit: every MCP progress notification
resets it. Calls are still bounded by an absolute cap of 20 times this value. After a timeout, completion and side
effects may be unknown, so inspect state before retrying; prefer a narrower operation, a background/monitor workflow, or
an MCP task for inherently long-running work.

### Auto-Bind to Roles

Use exact role tags. This example activates the server for either listed role:

```toml
[[mcp.servers]]
name = "my_server"
type = "http"
url = "http://localhost:3000/mcp"
timeout_seconds = 30
tools = []
auto_bind = ["developer:general", "assistant:concierge"]
```

### Tool Filtering

Use names advertised by your server; `search` and `get_record` below are example server-specific names.

```toml
# Only expose specific tools
[[mcp.servers]]
name = "local_api"
type = "http"
url = "http://localhost:3000/mcp"
timeout_seconds = 30
tools = ["search", "get_record"]

# Alternative wildcard filter: tools = ["get_*"]
```

### Override Files (mcp-*.toml)

Files named `mcp-*.toml` have special load order behavior — they are loaded **after** all other `*.toml` files,
regardless of alphabetical order. This ensures they can reliably override same-named servers.

**Use Case: Persisting Dynamic Servers**

When you use `mcp(action="persist", name="my_server")`, Octomind writes:

- File: `<config_dir>/mcp-my_server.toml`
- Content: Full server config; enabled servers get `auto_bind = ["<current_role>"]`, disabled servers clear it

On next startup, this file is loaded after all other config files, so it:

1. Overwrites any existing server named `my_server` (last wins for same-name entries)
2. Auto-binds to the role that persisted it if the server was enabled when persisted

**Example persisted override file:**

```toml
[[mcp.servers]]
name = "my_server"
type = "http"
url = "http://localhost:3000/mcp"
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

This server will automatically be available for the `developer:general` tap agent on next startup.

## Core Server Tools

### Adaptive external planning

There is no model-callable `plan` MCP tool. Focused tasks execute directly. When work has meaningful dependent phases or
context-loss risk, the specialist emits a hidden plan signal with a real work response and a separate supervisor call
updates runtime-owned state from bounded trajectory and evidence. Use this session command to inspect the current
checklist:

```text
/plan
```

### `recall` — Retrieve Archived Compression Blocks

`recall` is advertised only when compression attention or its governance layer is enabled. It accepts `ids`, an array of
one or two `b:<hex>` block IDs cited by compressed `<folded_state>` units, verifies them against the current session's
sidecar registry, and returns their role labels and original text (with surrounding whitespace trimmed). Unknown IDs and
sessions without an archive return errors; larger recalls require another call. Copy an actual ID from your summary into
this payload:

```json
{"ids": ["b:1a2b3c4d"]}
```

The ID shown illustrates the format; it must be replaced with a block ID cited in your own session.

## Orchestration Server Tools

### `tap` — Run Specialist Roles from Taps

Delegate work to a specialist role installed via a tap (e.g. `developer:general`). Each role brings its own system
prompt, model preferences, and MCP tool kit. Use `tap` to hand off a focused task, monitor what's running, stop a run,
or browse the catalog.

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `action` | string, required | `"run"`, `"list"`, `"stop"`, `"discover"`, `"capability"` |
| `role` | string | Role tag in `category:variant` form. Required for `run` when `session` is not given. |
| `prompt` | string | User message for `run`, or capability intent for `capability`. Required for those actions. |
| `session` | string | Run id (e.g. `tap-developer-general-a3f1c2`). Required for `stop`. For `run`, supply this to resume an existing run instead of starting a new one. |
| `workdir` | string | Working directory the role operates in. Optional — defaults to the parent session's current cwd. |
| `intent` | string | Free-text intent for `discover`. |

| Action | Description |
|--------|-------------|
| `run` | Launch a role (or resume one via `session`) in the background. Returns the run id immediately and injects the reply later. Resuming a run that is still executing a prior turn is rejected with a busy error — wait for it to finish or `stop` it first. |
| `list` | Show every run in this session: id, role, workdir, status (`running` / `done` / `failed` / `cancelled`), start time. |
| `stop` | Cancel a running role by id. Sends a watch-channel signal; the run aborts at its next checkpoint. |
| `discover` | Semantic match free-text intent against installed roles' titles/descriptions. Requires the local embedding model (errors if not ready). Returns roles scoring above 0.2 cosine, top 5. |
| `capability` | Run the prompt through the same skill/capability auto-activation path used for user messages. |

```jsonl
{"action": "discover", "intent": "review this codebase"}
{"action": "run", "role": "developer:general", "prompt": "Review src/main.rs and report concrete bugs."}
{"action": "run", "role": "developer:general", "prompt": "Audit this auth module"}
{"action": "list"}
{"action": "stop", "session": "tap-developer-general-a3f1c2"}
{"action": "run", "session": "tap-developer-general-a3f1c2", "prompt": "Check the error handling too."}
```

Use a role returned by `discover` and replace the sample session IDs with IDs returned by `run` or `list`.

**Lifecycle.** Tap-runs live for the duration of the parent session. When the parent session exits, all in-flight runs
are cancelled. The on-disk role manifest is unaffected.

**Non-interactive.** Tap-runs run in non-interactive mode, so `{{INPUT:KEY}}` / `{{ENV:KEY}}` placeholders that would
normally prompt stdin instead return a structured error. Resolve interactive placeholders and provide required
environment variables before delegation:

```bash
octomind run developer:general
```

The other orchestration tools are [`schedule`](#schedule--scheduled-message-injection) and `monitor`. `monitor` runs an
event-stream command, bounds and coalesces output injections, and is inspected through `/status monitors`.

### `schedule` — Scheduled Message Injection

Schedule messages for future injection into the session — fire at a specific time, or the next time the session becomes
idle. Also exposed as the [`/schedule`](../reference/02-session-commands.md#schedule-subcommand-args) slash command for
direct user control.

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `command` | string, required | `"add"`, `"list"`, `"remove"`, `"edit"` |
| `message` | string, required for `add` | exact text injected as a user message when the entry fires |
| `when` | string, optional for `add` | when to fire. Defaults to `"idle"` when both `when` and `every` are omitted. |
| `every` | string, optional | repeat interval — entry re-schedules itself after each firing until removed |

**`when` formats** (local timezone):

- `"idle"` — fires the next time the session becomes idle (no running taps, no running background jobs)
- `"now"` (fires immediately on the next scheduler tick)
- Relative: `"in 5m"`, `"in 2h"`, `"in 1h30m"`, `"in 90s"`
- Time today: `"15:30"`, `"3:30pm"`, `"9am"` (past times fire tomorrow)
- Exact: `"2030-03-22 15:30"`

**`every` format** (omit for one-shot):

- `"idle"` — fires on every idle transition (pairs with `when="idle"` or omitted)
- Same syntax as relative `when` without the `in` prefix — `"10m"`, `"1h"`, `"1h30m"`
- Pass `"none"` (or `"off"`) in `edit` to clear an existing interval

| Command | Required Params | Description |
|---------|----------------|-------------|
| `add` | `message` | Schedule a message. `when` defaults to `"idle"`. `description` and `every` optional. |
| `list` | -- | Show pending entries with countdown |
| `remove` | `id` | Cancel entry by ID |
| `edit` | `id` | Update `trigger_at` (via `when`), `message`, `description`, or interval (via `every`). Cannot switch an entry between idle and time modes — editing `when` on an idle entry has no firing effect (idle entries ignore `trigger_at`). Recreate the entry (remove + add) to change modes. |

One-shot entries fire once and are removed; repeating entries (`every` set) re-schedule automatically after each firing.
Idle entries fire only when the response loop is idle AND no tap-runs, detached shell jobs, or background-agent jobs are
running, so messages cannot interrupt in-flight work. Schedules are saved in the session log and restored when that
session resumes; they do not fire while the process is stopped.

For a repeating timer, supply both `when` and `every`; a time interval alone is rejected. Give repeating messages a stop
condition. Call `schedule`:

```jsonl
{"command": "add", "when": "in 10m", "every": "10m", "message": "Check the deployment status. If complete, remove this schedule using its ID from schedule list."}
{"command": "list"}
```

### `monitor` — Long-Lived Event Streams

The orchestration `monitor` tool has `start`, `list`, and `stop` actions. `start` requires an inline `command` and
optionally accepts `description`, `working_directory`, `flush_interval_seconds`, `max_batch_bytes`, `timeout_ms`, and
`persistent`. The command runs once through `sh -c`; stdout is delivered to the session inbox in bounded coalesced
batches, stderr is diagnostic, and unexpected exit is injected once. Monitors are session-owned, are never
auto-restarted, and stop on explicit `stop` or session cleanup.

| Monitor option | Default | Accepted range / meaning |
|----------------|---------|--------------------------|
| `flush_interval_seconds` | `30` | `5`–`3600` seconds between deliveries |
| `max_batch_bytes` | `65536` | `1024`–`1048576` bytes retained per batch |
| `timeout_ms` | `600000` | `1000`–`86400000` ms lifetime |
| `persistent` | `false` | Ignore the deadline until stop or session cleanup |

A monitor defaults to a 600000 ms lifetime. `persistent = true` removes that deadline, but session cleanup still stops
it. For an existing log file, call `monitor` (requires `sh` and `tail`):

```jsonl
{"action": "start", "command": "tail -n 0 -f /tmp/service.log", "flush_interval_seconds": 30, "persistent": true}
{"action": "list"}
```

Use the returned ID to stop it:

```json
{"action": "stop", "id": "monitor-id-from-list"}
```

```text
/status monitors
```

## Runtime Server Tools

Use these tools to add servers, prototype dynamic agents, or activate skills and capabilities during a session.

### `mcp` — Dynamic MCP Server Management

Manage MCP servers at runtime without editing config.

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `action` | string, required | `"list"`, `"add"`, `"enable"`, `"disable"`, `"remove"`, `"persist"`, `"unpersist"` |

| Action | Description |
|--------|-------------|
| `list` | Show all servers with status and persistence info |
| `add` | Register a new server (does not connect yet) |
| `enable` | Connect and activate a registered server's tools. Accepts an optional `tools` array to apply a per-enable filter (overrides the registered filter; empty/omitted = all tools advertised by the server). |
| `disable` | Deactivate server tools (config stays) |
| `remove` | Unregister entirely |
| `persist` | Save server config to config dir. If the server is enabled, auto-binds it to the current role; if disabled, clears `auto_bind` (file persists but won't auto-load). |
| `unpersist` | Remove persisted config file |

**Add parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `name` | string | Unique server name |
| `server_type` | string | `"stdio"` or `"http"` |
| `command` | string | Executable (for stdio) |
| `args` | array | Arguments (for stdio) |
| `url` | string | Endpoint (for http) |
| `timeout_seconds` | number | Per-operation timeout; tool-call progress resets this idle deadline (default: 30) |
| `tools` | array | Tool filter (empty = all, supports wildcards like `"github_*"`). Also accepted by `enable` for a per-enable filter. |

For a local HTTP MCP server already listening at this endpoint, call `mcp` in sequence:

```jsonl
{"action": "add", "name": "local_api", "server_type": "http", "url": "http://localhost:3000/mcp"}
{"action": "enable", "name": "local_api"}
{"action": "persist", "name": "local_api"}
{"action": "disable", "name": "local_api"}
{"action": "unpersist", "name": "local_api"}
{"action": "remove", "name": "local_api"}
```

### `agent` — Dynamic Agent Management

Manage in-process AI agents at runtime. Each registered agent becomes a tool prefixed with `agent_`. Distinct from the
`agent` server (which exposes config-defined ACP sub-agents) and from `tap run` (which launches tap-distributed roles).

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `action` | string, required | `"list"`, `"add"`, `"enable"`, `"disable"`, `"remove"` |

**Add parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `name` | string, required | Tool becomes `agent_<name>` after enable |
| `system` | string, required | System prompt |
| `description` | string, optional | Tool description; defaults to empty |
| `welcome` | string, optional | Welcome message |
| `model` | string, optional | Model-name override |
| `temperature`, `top_p` | number, optional | Sampling overrides |
| `top_k` | integer, optional | Sampling override |
| `server_refs` | array, optional | Validated server names; when empty, inferred from `allowed_tools` |
| `allowed_tools` | array, optional | Tool-name or wildcard filters |
| `workdir` | string, optional | Working directory; defaults to `"."` |

Register and enable a tool-free summarizer with `agent`:

```jsonl
{"action": "add", "name": "summarizer", "description": "Summarize supplied text", "system": "Summarize the supplied text accurately in one sentence.", "server_refs": [], "allowed_tools": []}
{"action": "enable", "name": "summarizer"}
```

Then call `agent_summarizer`:

```json
{"task": "Summarize: The deployment succeeded. Two follow-up checks remain."}
```

### `skill` — Skill Management from Taps

Manage skills (reusable instruction packs) from taps.

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `action` | string, required | `"list"`, `"use"`, `"forget"` |
| `name` | string | Skill name (required for `use` and `forget`) |
| `pattern` | string | Substring filter (for `list`) |
| `offset` | integer | Pagination offset (default: 0) |
| `limit` | integer | Max results (default: 20) |

Call `skill` to list, activate, then forget an installed skill. Replace `code-review` with a name from `list`:

```jsonl
{"action": "list", "limit": 20}
{"action": "use", "name": "code-review"}
{"action": "forget", "name": "code-review"}
```

Activation injects instructions; forgetting removes the active entry and lets the next automatic compression clean up
its old content.

**Skill resources:** Skills can include `scripts/`, `references/`, and `assets/` subdirectories. When activated, a
resource catalog with absolute paths is provided.

> **Internal note:** the dispatcher also accepts a `use_silent` action used for silent / auto-activation (env-loaded skills, `/skill` activation). It is not part of the JSON schema enum — the user/AI-facing actions are only `list`, `use`, and `forget`.

To force-load an installed skill at session startup, use its exact name:

```bash
OCTOMIND_SKILLS=code-review octomind run
```

Replace `code-review` with an installed name; multiple names are comma-separated.

### `capability` — Discover and Activate Domain Bundles

Activate MCP server bundles ("capabilities") on demand. Capabilities are TOML-defined groups of MCP servers and tool
filters distributed via taps (`<tap>/capabilities/<name>/<provider>.toml`).

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `action` | string, required | `"list"`, `"discover"`, `"enable"`, `"disable"` |
| `name` | string | Capability name (required for `enable` and `disable`) |
| `intent` | string | Free-text intent for `discover` (e.g., `"I need to query a database"`) |

| Action | Description |
|--------|-------------|
| `list` | Show every installed capability with active marker |
| `discover` | Semantic search by intent — capabilities scoring above 0.2 cosine, top 5 returned |
| `enable` | Register and connect a capability's MCP servers (domain-gated — see below) |
| `disable` | Disconnect a capability's tools (refcount-aware — see below) |

Use an installed name returned by `list` in place of `database-postgres`:

```jsonl
{"action": "list"}
{"action": "discover", "intent": "I need to query a Postgres database"}
{"action": "enable", "name": "database-postgres"}
{"action": "disable", "name": "database-postgres"}
```

**`discover` requires the embedding model.** Semantic discovery embeds your intent with the local embedding model
(muvon/octomind-embed). If that model is not yet initialized, `discover` returns an error rather than degrading — wait a
moment after startup and retry. Results are filtered to cosine score > 0.2 and capped at the top 5.

**`enable` is domain-gated.** A capability whose manifest binds it to specific domains can only be enabled when the
session's current domain matches; enabling a cap bound to other domains is refused with an error. Capabilities with no
`domains` list are universal and enable anywhere.

**`disable` is refcount-aware.** When multiple active capabilities (or a role's static config) reference the same
underlying MCP server, disabling one capability only strips *that* capability's tools — the server keeps running for its
other consumers. The server process is fully shut down only when this was the last referencer and no static role config
owns it.

**Auto-activation.** Capabilities also auto-activate before each API call when the user's message strongly matches a
capability's hand-authored triggers (semantic match via local embedding, no LLM in the loop). Activation uses a
similarity threshold of 0.45 with a 0.08 abstain-on-tie margin and considers the top 3 trigger scores; the active set is
bounded by an LRU eviction policy (soft cap of 4). See [Token
Efficiency](16-token-efficiency.md#deterministic-auto-activation) for the full algorithm.

**Boot-time forcing.** Use `OCTOMIND_CAPABILITIES` to force-enable a comma-separated list of capabilities at startup.
Every comma-delimited value must be the exact installed capability directory/name; this path does not perform semantic
discovery, alias expansion, or fuzzy matching. Forced capabilities are still domain- and environment-gated. For an
installed capability named `database-postgres`:

```bash
OCTOMIND_CAPABILITIES=database-postgres octomind run
```

## External Filesystem Tools

Octomind loads external tool schemas at connection time. This repository does not define the parameter contracts of
`octofs` or other companion servers; inspect your installed server before calling its tools:

```text
/mcp full
/status jobs
```

`/status jobs` also shows detached shell jobs discovered through MCP resource links. See [Local
Tools](17-local-tools.md) if you want to expose your own project scripts.

## Agent Server Tools

Each agent configured in `[[agents]]` becomes a separate tool: `agent_<name>`.

**Parameters:**

| Parameter | Type / requirement | Meaning |
|-----------|--------------------|---------|
| `task` | string, required | Task description for the agent |
| `async` | boolean, default: false | Run asynchronously |

**Sync (default):** Blocks until complete. Use when you need the result immediately.

**Async:** Returns immediately. Result appears as a user message when done. Use for tasks taking 30+ seconds when you
can continue other work.

```jsonl
{"task": "Analyze the authentication system architecture"}
{"task": "Review this function for performance", "async": true}
```

Max concurrent async jobs equals the detected CPU count, with a fallback of 4 when detection fails. Jobs are cancelled
on session exit.

## Health Monitoring

MCP servers are monitored automatically:

- Health checks every 120 seconds for external servers (HTTP + stdio)
- Builtin servers are always considered healthy
- Only restartable local processes auto-restart: stdio servers. HTTP endpoints are checked but cannot be restarted by
  Octomind (the HTTP config has no launch command).
- Three consecutive restart failures mark a server failed; attempts are separated by a 30-second cooldown
- A terminal `Failed` state is left alone by the monitor; it is not automatically probed or restarted again
- Use `/mcp health` to force a health check

## Common Questions

**Why is a configured tool missing?** Check the active role, exact `auto_bind` tag, `server_refs`, and tool filters. An
installed server is not necessarily enabled. Run:

```text
/role
/mcp list
/mcp full
/mcp health
```

**Why did discovery fail?** Wait for the local embedding model to initialize, then retry `discover`; `list` does not
need semantic matching. Check missing environment variables if capability activation is refused.

**Where are persisted servers?** They are in the data directory's `config/` subdirectory. On macOS/Linux the default is
`~/.local/share/octomind/config/`; on Windows it is `%LOCALAPPDATA%/octomind/config/`. `OCTOMIND_DATA_DIR` relocates
that data tree. Exact role tags matter: `developer` does not match `developer:general`.

## Source Reference

- [Built-in tools and routing](../../src/mcp/tool_map.rs)
- [MCP config and defaults](../../src/config/mcp.rs)
- [Runtime tools](../../src/mcp/runtime/mod.rs)
- [Schedules and monitors](../../src/mcp/orchestration/mod.rs)
- [External server lifecycle](../../src/mcp/health_monitor.rs)

## See also

- [Configuration reference](../reference/03-config-reference.md)
- [Session commands](../reference/02-session-commands.md)
- [Tap system](../integration/04-tap-system.md)
- [Local tools](17-local-tools.md)
