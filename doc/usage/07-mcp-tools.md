# MCP Tools Reference

MCP unifies Octomind's built-in controls, tap capabilities, external servers, and project-local tools through one runtime surface.

## Architecture

Octomind ships four builtin MCP servers (`core`, `orchestration`, `runtime`, `agent`), plus an auto-discovered `local` server for project scripts:

| Server | Type | Description |
|--------|------|-------------|
| `core` | builtin | Session-memory retrieval (`recall` when attention or governance is enabled; governance defaults on) |
| `orchestration` | builtin | Delegation (`tap`), scheduled messages (`schedule`), and event streams (`monitor`) |
| `runtime` | builtin | Harness reconfiguration: register MCP servers, manage dynamic agents, load skills, capability |
| `agent` | builtin | Delegates tasks to configured ACP sub-agents (each `[[agents]]` entry exposes an `agent_<name>` tool) |
| `local` | builtin | Project-local shebang-script tools auto-discovered from `<workdir>/.agents/tools/`. See [Local Tools](17-local-tools.md). |

The filesystem tools (`view`, `text_editor`, `shell`, …) are **not** a builtin server. They are served by a separate `octofs` MCP server (a stdio subprocess: command `octofs`, args `["mcp"]`) that is **not declared in the default config**. It is delivered through the built-in default tap [`muvon/tap`](../integration/04-tap-system.md)'s capabilities `filesystem-read` and `filesystem-write`, and roles reach it via `server_refs`/capabilities under the `filesystem` capability name — never a hardcoded `[[mcp.servers]]` block named `filesystem`. See [Filesystem Server Tools (octofs)](#filesystem-server-tools-octofs) below for the prerequisites.

Planning is supervisor-internal rather than an MCP tool. The specialist sees runtime-owned plan state and emits sparse hidden signals alongside normal work; the external planner owns transitions. `/plan` only displays that state.

Additional servers can be added via `[[mcp.servers]]` config as `http` or `stdio` types.

## Core Server Tools

### Adaptive external planning

There is no model-callable `plan` MCP tool. Focused tasks execute directly. When work has meaningful dependent phases or context-loss risk, the specialist emits a hidden plan signal with a real work response and a separate supervisor call updates runtime-owned state from bounded trajectory and evidence. Use `/plan` to inspect the current checklist.

### `recall` — Retrieve Archived Compression Blocks

`recall` is advertised only when compression attention or its governance layer is enabled. It accepts `ids`, an array of one or two `b:<hex>` block IDs cited by compressed `<folded_state>` units, verifies them against the current session's sidecar registry, and returns the original archived messages verbatim. Unknown IDs and sessions without an archive return errors; larger recalls require another call.

## Orchestration Server Tools

### `tap` — Run Specialist Roles from Taps

Delegate work to a specialist role installed via a tap (e.g. `developer:general`, `lawyer:us`, `security:owasp`). Each role brings its own system prompt, model preferences, and MCP tool kit. Use `tap` to hand off a focused task, monitor what's running, stop a run, or browse the catalog.

**Parameters:**
- `action` (string, required): `"run"`, `"list"`, `"stop"`, `"discover"`, `"capability"`
- `role` (string): Role tag in `category:variant` form. Required for `run` when `session` is not given.
- `prompt` (string): User message for `run`, or capability intent for `capability`. Required for those actions.
- `session` (string): Run id (e.g. `tap-developer-general-a3f1c2`). Required for `stop`. For `run`, supply this to resume an existing run instead of starting a new one.
- `workdir` (string): Working directory the role operates in. Optional — defaults to the parent session's current cwd.
- `intent` (string): Free-text intent for `discover`.

| Action | Description |
|--------|-------------|
| `run` | Launch a role (or resume one via `session`) in the background. Returns the run id immediately and injects the reply later. Resuming a run that is still executing a prior turn is rejected with a busy error — wait for it to finish or `stop` it first. |
| `list` | Show every run in this session: id, role, workdir, status (`running` / `done` / `failed` / `cancelled`), start time. |
| `stop` | Cancel a running role by id. Sends a watch-channel signal; the run aborts at its next checkpoint. |
| `discover` | Semantic match free-text intent against installed roles' titles/descriptions. Requires the local embedding model (errors if not ready). Returns roles scoring above 0.2 cosine, top 5. |
| `capability` | Run the prompt through the same skill/capability auto-activation path used for user messages. |

```jsonl
{"action": "discover", "intent": "review a Singapore employment contract"}
{"action": "run", "role": "lawyer:sg", "prompt": "What are the notice period rules for termination?"}
{"action": "run", "role": "security:owasp", "prompt": "Audit this auth module"}
{"action": "list"}
{"action": "stop", "session": "tap-security-owasp-a3f1c2"}
{"action": "run", "session": "tap-lawyer-sg-9b2c1d", "prompt": "What about probationary periods?"}
```

**Lifecycle.** Tap-runs live for the duration of the parent session. When the parent session exits, all in-flight runs are cancelled. The on-disk role manifest is unaffected.

**Non-interactive.** Tap-runs run in non-interactive mode, so `{{INPUT:KEY}}` / `{{ENV:KEY}}` placeholders that would normally prompt stdin instead return a structured error. Pre-populate inputs once via an interactive `octomind run developer:general`, then tap-run picks up the stored values.

The other orchestration tools are [`schedule`](#schedule----scheduled-message-injection) and `monitor`. `monitor` runs an event-stream command, bounds and coalesces output injections, and is inspected through `/status monitors`.

## Runtime and Orchestration Tool Details

Low-level tools for reconfiguring the harness mid-session. Most agents won't need these — they're for tasks like adding a one-off MCP server, prototyping a dynamic agent, or activating a skill.

### `mcp` — Dynamic MCP Server Management

Manage MCP servers at runtime without editing config.

**Parameters:**
- `action` (string, required): `"list"`, `"add"`, `"enable"`, `"disable"`, `"remove"`, `"persist"`, `"unpersist"`

| Action | Description |
|--------|-------------|
| `list` | Show all servers with status and persistence info |
| `add` | Register a new server (does not connect yet) |
| `enable` | Connect and activate a registered server's tools. Accepts an optional `tools` array to apply a per-enable filter (overrides the registered filter; empty/omitted = all registered tools). |
| `disable` | Deactivate server tools (config stays) |
| `remove` | Unregister entirely |
| `persist` | Save server config to config dir. If the server is enabled, auto-binds it to the current role (`auto_bind = [role]`); if disabled, clears `auto_bind` (file persists but won't auto-load). |
| `unpersist` | Remove persisted config file |

**Add parameters:**
- `name` (string): Unique server name
- `server_type` (string): `"stdio"` or `"http"`
- `command` (string): Executable (for stdio)
- `args` (array): Arguments (for stdio)
- `url` (string): Endpoint (for http)
- `timeout_seconds` (number): Per-operation timeout; tool-call progress resets this idle deadline (default: 30)
- `tools` (array): Tool filter (empty = all, supports wildcards like `"github_*"`). Also accepted by `enable` for a per-enable filter.

### `agent` — Dynamic Agent Management

Manage in-process AI agents at runtime. Each registered agent becomes a tool prefixed with `agent_`. Distinct from the `agent` server (which exposes config-defined ACP sub-agents) and from `tap run` (which launches tap-distributed roles).

**Parameters:**
- `action` (string, required): `"list"`, `"add"`, `"enable"`, `"disable"`, `"remove"`

**Add parameters:**
- `name` (string): Unique agent name (tool becomes `agent_<name>`)
- `description` (string): MCP tool description
- `system` (string): System prompt (required for add)
- `welcome` (string): Optional welcome message
- `model` (string): Optional model-name override
- `temperature`, `top_p`, `top_k`: Existing optional sampling overrides
- `server_refs` (array): MCP server references — validated at add-time against config-defined and dynamic servers. When left empty, the needed servers are auto-derived from the `allowed_tools` patterns.
- `allowed_tools` (array): Tool filter (supports wildcards)
- `workdir` (string): Working directory (default: `"."`)

### `skill` — Skill Management from Taps

Manage skills (reusable instruction packs) from taps.

**Parameters:**
- `action` (string, required): `"list"`, `"use"`, `"forget"`
- `name` (string): Skill name (required for `use` and `forget`)
- `pattern` (string): Substring filter (for `list`)
- `offset` (integer): Pagination offset (default: 0)
- `limit` (integer): Max results (default: 20)

**Workflow:**
1. `skill(action="list")` — discover available skills
2. `skill(action="use", name="skill-name")` — activate (injects instructions into context)
3. `skill(action="forget", name="skill-name")` — deactivate (removes from active skills, content cleaned up at next automatic compression)

**Skill resources:** Skills can include `scripts/`, `references/`, and `assets/` subdirectories. When activated, a resource catalog with absolute paths is provided.

> **Internal note:** the dispatcher also accepts a `use_silent` action used for silent / auto-activation (env-loaded skills, `/skill` activation). It is not part of the JSON schema enum — the user/AI-facing actions are only `list`, `use`, and `forget`.

### `schedule` — Scheduled Message Injection

Schedule messages for future injection into the session — fire at a specific time, or the next time the session becomes idle. Also exposed as the [`/schedule`](../reference/02-session-commands.md#schedule-subcommand-args) slash command for direct user control.

**Parameters:**
- `command` (string, required): `"add"`, `"list"`, `"remove"`, `"edit"`
- `message` (string, required for `add`): exact text injected as a user message when the entry fires
- `when` (string, optional for `add`): when to fire. Defaults to `"idle"` when both `when` and `every` are omitted.
- `every` (string, optional): repeat interval — entry re-schedules itself after each firing until removed

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

One-shot entries fire once and are removed; repeating entries (`every` set) re-schedule automatically after each firing. Idle entries fire only when the response loop is idle AND no tap-runs or background-agent jobs are running, so messages cannot interrupt in-flight work. Jobs cancelled on session exit.

### `monitor` — Long-Lived Event Streams

The orchestration `monitor` tool has `start`, `list`, and `stop` actions. `start` requires an inline `command` and optionally accepts `description`, `working_directory`, `flush_interval_seconds`, `max_batch_bytes`, `timeout_ms`, and `persistent`. The command runs once through `sh -c`; stdout is delivered to the session inbox in bounded coalesced batches, stderr is diagnostic, and unexpected exit is injected once. Monitors are session-owned, are never auto-restarted, and stop on explicit `stop` or session cleanup.

### `capability` — Discover and Activate Domain Bundles

Activate MCP server bundles ("capabilities") on demand. Capabilities are TOML-defined groups of MCP servers and tool filters distributed via taps (`<tap>/capabilities/<name>/<provider>.toml`).

**Parameters:**
- `action` (string, required): `"list"`, `"discover"`, `"enable"`, `"disable"`
- `name` (string): Capability name (required for `enable` and `disable`)
- `intent` (string): Free-text intent for `discover` (e.g., `"I need to query a database"`)

| Action | Description |
|--------|-------------|
| `list` | Show every installed capability with active marker |
| `discover` | Semantic search by intent — capabilities scoring above 0.2 cosine, top 5 returned |
| `enable` | Register and connect a capability's MCP servers (domain-gated — see below) |
| `disable` | Disconnect a capability's tools (refcount-aware — see below) |

```jsonl
{"action": "list"}
{"action": "discover", "intent": "I need to query a Postgres database"}
{"action": "enable", "name": "database-postgres"}
{"action": "disable", "name": "database-postgres"}
```

**`discover` requires the embedding model.** Semantic discovery embeds your intent with the local embedding model (muvon/octomind-embed). If that model is not yet initialized, `discover` returns an error rather than degrading — wait a moment after startup and retry. Results are filtered to cosine score > 0.2 and capped at the top 5.

**`enable` is domain-gated.** A capability whose manifest binds it to specific domains can only be enabled when the session's current domain matches; enabling a cap bound to other domains is refused with an error. Capabilities with no `domains` list are universal and enable anywhere.

**`disable` is refcount-aware.** When multiple active capabilities (or a role's static config) reference the same underlying MCP server, disabling one capability only strips *that* capability's tools — the server keeps running for its other consumers. The server process is fully shut down only when this was the last referencer and no static role config owns it.

**Auto-activation.** Capabilities also auto-activate before each API call when the user's message strongly matches a capability's hand-authored triggers (semantic match via local embedding, no LLM in the loop). Activation uses a similarity threshold of 0.45 with a 0.08 abstain-on-tie margin and considers the top 3 trigger scores; the active set is bounded by an LRU eviction policy (soft cap of 4). See [Token Efficiency](16-token-efficiency.md#deterministic-auto-activation) for the full algorithm.

**Boot-time forcing.** Set `OCTOMIND_CAPABILITIES=cap1,cap2` to force-enable specific capabilities at startup. Every comma-delimited value must be the exact installed capability directory/name; this path does not perform semantic discovery, alias expansion, or fuzzy matching. Forced capabilities are still domain- and environment-gated.

## Filesystem Server Tools (octofs)

These tools are provided by the external `octofs` MCP server (command `octofs mcp`) running as a stdio subprocess. They are **not** a builtin — to have them you need:

1. The `octofs` binary on your `PATH`.
2. The built-in default tap [`muvon/tap`](../integration/04-tap-system.md) present (auto-cloned on first use), which ships the `filesystem-read` / `filesystem-write` capabilities that declare the `octofs` server.

A role or tap agent references these tools through its `server_refs` / capabilities under the `filesystem` server name — there is no hardcoded `[[mcp.servers]]` entry named `filesystem`. Without the tap and binary, these tools will not be present.

The octofs server provides six tools: `view`, `workdir`, `text_editor`, `batch_edit`, `extract_lines`, and `shell`. The parameter schemas are advertised by the external octofs process, so `/mcp full` is authoritative for the installed version.

### `view` — Read Files and Directories

Read files, view directories, and search file content.

```jsonl
{"path": "src/main.rs"}
{"path": "src/main.rs", "lines": [10, 20]}
{"path": "src/", "pattern": "TODO"}
{"content": "function_name", "path": "src/"}
```

**Parameters:**
- `path` (string): File or directory path
- `lines` (array): `[start, end]` line range
- `pattern` (string): Search pattern within file/directory
- `content` (string): Content search query

### `text_editor` — File Editing

Comprehensive file manipulation with multiple commands.

**Commands:**

| Command | Key Params | Description |
|---------|-----------|-------------|
| `create` | `path`, `file_text` | Create new file |
| `str_replace` | `path`, `old_str`, `new_str` | Replace specific string |
| `insert` | `path`, `insert_line`, `new_str` | Insert at line position |
| `line_replace` | `path`, `view_range`, `new_str` | Replace line range (empty `new_str` = delete) |
| `undo_edit` | `path` | Revert last edit |
| `view` | `path`, `view_range` (optional) | View file or range |
| `view_many` | `paths` (array) | View multiple files |

```jsonl
{"command": "create", "path": "src/new.rs", "file_text": "pub fn hello() {}"}
{"command": "str_replace", "path": "src/main.rs", "old_str": "fn old()", "new_str": "fn new()"}
{"command": "insert", "path": "src/main.rs", "insert_line": 5, "new_str": "// Comment"}
{"command": "line_replace", "path": "src/main.rs", "view_range": [5, 8], "new_str": "fn updated() {}"}
{"command": "undo_edit", "path": "src/main.rs"}
```

### `batch_edit` — Atomic Multi-Line Editing

Multiple insert/replace operations on a single file atomically. All operations reference original line numbers (before any changes).

**Parameters:**
- `path` (string, required): File path
- `operations` (array, required):
  - `operation`: `"insert"` (after line) or `"replace"` (line range)
  - `line_range`: Single number (insert) or `[start, end]` (replace)
  - `content`: Content to insert/replace

```json
{
  "path": "src/main.rs",
  "operations": [
    {"operation": "replace", "line_range": [10, 12], "content": "fn new_function() {}"},
    {"operation": "insert", "line_range": 20, "content": "// New comment"}
  ]
}
```

Returns a standard diff showing changes.

### `extract_lines` — Extract and Move Code

Extract lines from a source file and append to a target file without modifying the source.

**Parameters:**
- `from_path` (string): Source file
- `from_range` (array): `[start, end]` line numbers (1-indexed, inclusive)
- `append_path` (string): Target file (auto-created if needed)
- `append_line` (integer): Insert position (0=beginning, -1=end, N=after line N)

```jsonl
{"from_path": "src/utils.rs", "from_range": [10, 25], "append_path": "src/extracted.rs", "append_line": -1}
```

### `shell` — Shell Command Execution

Execute shell commands with output capture.

**Parameters:**
- `command` (string, required): Shell command

```json
{"command": "cargo test"}
```

Octomind does not define a `background` boolean for this companion tool. Current octofs long-running work is surfaced through MCP resource links and appears under `/status jobs`; do not rely on the removed PID-style examples.

### `workdir` — Working Directory Management

Get or set the working directory for file and shell operations.

**Parameters:**
- `path` (string): Set new working directory (absolute or relative)
- `reset` (boolean): Reset to original project directory

```jsonl
{}
{"path": "/path/to/directory"}
{"reset": true}
```

Thread-local: changes only affect the current session.

## Agent Server Tools

Each agent configured in `[[agents]]` becomes a separate tool: `agent_<name>`.

**Parameters:**
- `task` (string, required): Task description for the agent
- `async` (boolean, default: false): Run asynchronously

**Sync (default):** Blocks until complete. Use when you need the result immediately.

**Async:** Returns immediately. Result appears as a user message when done. Use for tasks taking 30+ seconds when you can continue other work.

```jsonl
{"task": "Analyze the authentication system architecture"}
{"task": "Review this function for performance", "async": true}
```

Max concurrent async jobs equals the detected CPU count, with a fallback of 4 when detection fails. Jobs are cancelled on session exit.

## External MCP Servers

### Adding HTTP Servers

```toml
[[mcp.servers]]
name = "custom_api"
type = "http"
url = "https://api.example.com/mcp"
headers = { Authorization = "Bearer {{ENV:CUSTOM_API_TOKEN}}" }  # optional
timeout_seconds = 30
tools = []
```

`headers` is sent on every request. Values may use `{{ENV:KEY}}` placeholders; a server whose placeholders reference unset env vars is skipped at startup with an error log. When an `Authorization` header is configured it is used as-is and OAuth discovery is disabled for that server; without one, Octomind runs MCP Authorization Discovery (RFC 9728) and authenticates via PKCE automatically.

### Adding Stdio Servers

```toml
[[mcp.servers]]
name = "custom_tools"
type = "stdio"
command = "python"
args = ["-m", "my_mcp_server"]
timeout_seconds = 30
tools = []
```

For tool calls, `timeout_seconds` is an idle deadline rather than a total runtime limit: every MCP progress notification resets it. Calls are still bounded by an absolute cap of 20 times this value. After a timeout, completion and side effects may be unknown, so inspect state before retrying; prefer a narrower operation, a background/monitor workflow, or an MCP task for inherently long-running work.

### Auto-Bind to Roles

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

```toml
# Only expose specific tools
[[mcp.servers]]
name = "github_mcp"
type = "http"
url = "https://api.github.com/mcp"
tools = ["github_create_issue", "github_list_repos"]

# Alternative wildcard filter: tools = ["github_*"]
```

### Override Files (mcp-*.toml)

Files named `mcp-*.toml` have special load order behavior — they are loaded **after** all other `*.toml` files, regardless of alphabetical order. This ensures they can reliably override same-named servers.

**Use Case: Persisting Dynamic Servers**

When you use `mcp(action="persist", name="my_server")`, Octomind writes:
- File: `<config_dir>/mcp-my_server.toml`
- Content: Full server config plus `auto_bind = ["<current_role>"]`

On next startup, this file is loaded after all other config files, so it:
1. Overwrites any existing server named `my_server` (last wins for same-name entries)
2. Auto-binds to the role that persisted it

**Example persisted override file:**

```toml
[[mcp.servers]]
name = "github"
type = "http"
url = "https://api.github.com/mcp"
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

This server will automatically be available for the `developer:general` tap agent on next startup.

## Health Monitoring

MCP servers are monitored automatically:
- Health checks every 120 seconds for external servers (HTTP + stdio)
- Builtin servers are always considered healthy
- Only restartable local processes auto-restart: stdio servers and HTTP entries that have a launch command. Remote HTTP endpoints are checked but cannot be restarted by Octomind.
- Three consecutive restart failures mark a server failed; attempts are separated by a 30-second cooldown
- A terminal `Failed` state is left alone by the monitor; it is not automatically probed or restarted again
- Use `/mcp health` to force a health check

## Design Notes: Builtin Server Boundaries

The builtin split provides clear ownership while keeping each role's tool schema small.

**The taxonomy.** `runtime` changes the available harness and tool surface. `orchestration` delegates or schedules work. `core` holds small session-native primitives such as conditional `recall`. Planning is external supervisor state rather than a tool category.

**The token cost.** Every always-on tool is schema text the model receives every turn, even when irrelevant. Splitting `runtime` out lets roles omit its four control tools (`mcp`, `agent`, `skill`, `capability`) when they are not needed.

**Where new tools go.** Harness or tool-surface mutation belongs in `runtime`; delegation, schedules, and monitors belong in `orchestration`; small universally session-native primitives belong in `core`. Domain work is usually a [capability](#capability----discover-and-activate-domain-bundles) activated on demand rather than a built-in.

**Token direction.** Keep model-callable built-ins sparse. Plan bookkeeping moved out of the specialist surface entirely; role filters and capabilities should similarly avoid exposing runtime tools that the role does not need.
