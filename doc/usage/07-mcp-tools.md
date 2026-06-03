# MCP Tools Reference

Octomind uses the Model Context Protocol (MCP) to provide AI models with external tools. This is the single reference for all built-in tools.

## Architecture

Octomind ships **three builtin MCP servers** declared in the default config (`core`, `runtime`, `agent`), plus an auto-discovered `local` server for project scripts:

| Server | Type | Description |
|--------|------|-------------|
| `core` | builtin | High-level day-to-day tools: planning, tap-role launch |
| `runtime` | builtin | Low-level harness reconfiguration: register MCP servers, manage dynamic agents, load skills, schedule, capability |
| `agent` | builtin | Delegates tasks to configured ACP sub-agents (each `[[agents]]` entry exposes an `agent_<name>` tool) |
| `local` | builtin | Project-local shebang-script tools auto-discovered from `<workdir>/.agents/tools/`. See [Local Tools](17-local-tools.md). |

The filesystem tools (`view`, `text_editor`, `shell`, `ast_grep`, …) are **not** a builtin server. They are served by a separate `octofs` MCP server (a stdio subprocess: command `octofs`, args `["mcp"]`) that is **not declared in the default config**. It is delivered through the built-in default tap [`muvon/tap`](../integration/04-tap-system.md)'s capabilities `filesystem-read` and `filesystem-write`, and roles reach it via `server_refs`/capabilities under the `filesystem` capability name — never a hardcoded `[[mcp.servers]]` block named `filesystem`. See [Filesystem Server Tools (octofs)](#filesystem-server-tools-octofs) below for the prerequisites.

`core` and `runtime` are the two split halves of what used to be a single `core` server. The split separates "what the agent uses to do work" (`core`) from "what reconfigures the harness mid-session" (`runtime`).

Additional servers can be added via `[[mcp.servers]]` config as `http` or `stdio` types.

## Core Server Tools

### `plan` -- Structured Task Management

Break down large objectives into steps with progress tracking.

**Parameters:**
- `command` (string, required): `"start"`, `"step"`, `"next"`, `"list"`, `"done"`, `"reset"`

| Command | Required Params | Description |
|---------|----------------|-------------|
| `start` | `content` (plan goal/title), `tasks` (array of `{title, description}`) | Begin a new plan (errors if a plan already exists — `done` or `reset` first) |
| `step` | `content` | Add progress notes to current task (does not advance it) |
| `next` | `content` | Mark current task done, advance |
| `list` | -- | Show all tasks with status |
| `done` | `content` | Complete plan, trigger cleanup |
| `reset` | -- | Abort and clear plan |

The plan title comes from the `content` parameter on `start` — there is **no** `title` property on the tool itself (only inside each `tasks` entry). The schema sets `additionalProperties: false`, so a stray top-level `title` key is rejected.

```json
{"command": "start", "content": "Implement Auth", "tasks": [
  {"title": "Design API", "description": "Create endpoints"},
  {"title": "Write tests", "description": "Unit and integration"}
]}
{"command": "next", "content": "API designed, moving to tests"}
{"command": "done", "content": "Feature complete"}
```

### `tap` -- Run Specialist Roles from Taps

Delegate work to a specialist role installed via a tap (e.g. `developer:general`, `lawyer:us`, `security:owasp`). Each role brings its own system prompt, model preferences, and MCP tool kit. Use `tap` to hand off a focused task, monitor what's running, stop a run, or browse the catalog.

**Parameters:**
- `action` (string, required): `"run"`, `"list"`, `"stop"`, `"discover"`
- `role` (string): Role tag in `category:variant` form. Required for `run` when `session` is not given.
- `prompt` (string): User message to send. Required for `run`.
- `session` (string): Run id (e.g. `tap-developer-general-a3f1c2`). Required for `stop`. For `run`, supply this to resume an existing run instead of starting a new one.
- `workdir` (string): Working directory the role operates in. Optional -- defaults to the parent session's current cwd.
- `background` (boolean, default: false): When true, return immediately and inject the reply as a user message when ready.
- `intent` (string): Free-text intent for `discover`.

| Action | Description |
|--------|-------------|
| `run` | Launch a role (or resume one via `session`). Foreground blocks for the reply; background returns the run id and injects the reply later. Resuming a run that is still executing a prior turn is rejected with a busy error — wait for it to finish or `stop` it first. |
| `list` | Show every run in this session: id, role, workdir, status (`running` / `done` / `failed` / `cancelled`), start time. |
| `stop` | Cancel a running role by id. Sends a watch-channel signal; the run aborts at its next checkpoint. |
| `discover` | Semantic match free-text intent against installed roles' titles/descriptions. Requires the local embedding model (errors if not ready). Returns roles scoring above 0.2 cosine, top 5. |

```json
{"action": "discover", "intent": "review a Singapore employment contract"}
{"action": "run", "role": "lawyer:sg", "prompt": "What are the notice period rules for termination?"}
{"action": "run", "role": "security:owasp", "prompt": "Audit this auth module", "background": true}
{"action": "list"}
{"action": "stop", "session": "tap-security-owasp-a3f1c2"}
{"action": "run", "session": "tap-lawyer-sg-9b2c1d", "prompt": "What about probationary periods?"}
```

**Lifecycle.** Tap-runs live for the duration of the parent session. When the parent session exits, all in-flight runs are cancelled. The on-disk role manifest is unaffected.

**Non-interactive.** Tap-runs run in non-interactive mode, so `{{INPUT:KEY}}` / `{{ENV:KEY}}` placeholders that would normally prompt stdin instead return a structured error. Pre-populate inputs once via `octomind run <role>` (interactive), then tap-run picks up the stored values.

## Runtime Server Tools

Low-level tools for reconfiguring the harness mid-session. Most agents won't need these — they're for tasks like adding a one-off MCP server, prototyping a dynamic agent, or activating a skill.

### `mcp` -- Dynamic MCP Server Management

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
- `timeout_seconds` (number): Timeout (default: 60)
- `tools` (array): Tool filter (empty = all, supports wildcards like `"github_*"`). Also accepted by `enable` for a per-enable filter.

### `agent` -- Dynamic Agent Management

Manage in-process AI agents at runtime. Each registered agent becomes a tool prefixed with `agent_`. Distinct from the `agent` server (which exposes config-defined ACP sub-agents) and from `tap run` (which launches tap-distributed roles).

**Parameters:**
- `action` (string, required): `"list"`, `"add"`, `"enable"`, `"disable"`, `"remove"`

**Add parameters:**
- `name` (string): Unique agent name (tool becomes `agent_<name>`)
- `description` (string): MCP tool description
- `system` (string): System prompt (required for add)
- `welcome` (string): Optional welcome message
- `model` (string): Model override
- `temperature`, `top_p`, `top_k`: Sampling parameters
- `server_refs` (array): MCP server references — validated at add-time against config-defined and dynamic servers. When left empty, the needed servers are auto-derived from the `allowed_tools` patterns.
- `allowed_tools` (array): Tool filter (supports wildcards)
- `workdir` (string): Working directory (default: `"."`)

### `skill` -- Skill Management from Taps

Manage skills (reusable instruction packs) from taps.

**Parameters:**
- `action` (string, required): `"list"`, `"use"`, `"forget"`
- `name` (string): Skill name (required for `use` and `forget`)
- `pattern` (string): Substring filter (for `list`)
- `offset` (integer): Pagination offset (default: 0)
- `limit` (integer): Max results (default: 20)

**Workflow:**
1. `skill(action="list")` -- discover available skills
2. `skill(action="use", name="skill-name")` -- activate (injects instructions into context)
3. `skill(action="forget", name="skill-name")` -- deactivate (removes from active skills, content cleaned up at next automatic compression)

**Skill resources:** Skills can include `scripts/`, `references/`, and `assets/` subdirectories. When activated, a resource catalog with absolute paths is provided.

> **Internal note:** the dispatcher also accepts a `use_silent` action used for silent / auto-activation (env-loaded skills, `/skill` activation). It is not part of the JSON schema enum — the user/AI-facing actions are only `list`, `use`, and `forget`.

### `schedule` -- Scheduled Message Injection

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
- Exact: `"2026-03-22 15:30"`

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

### `capability` -- Discover and Activate Domain Bundles

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

```json
{"action": "list"}
{"action": "discover", "intent": "I need to query a Postgres database"}
{"action": "enable", "name": "database-postgres"}
{"action": "disable", "name": "database-postgres"}
```

**`discover` requires the embedding model.** Semantic discovery embeds your intent with the local embedding model (muvon/octomind-embed). If that model is not yet initialized, `discover` returns an error rather than degrading — wait a moment after startup and retry. Results are filtered to cosine score > 0.2 and capped at the top 5.

**`enable` is domain-gated.** A capability whose manifest binds it to specific domains can only be enabled when the session's current domain matches; enabling a cap bound to other domains is refused with an error. Capabilities with no `domains` list are universal and enable anywhere.

**`disable` is refcount-aware.** When multiple active capabilities (or a role's static config) reference the same underlying MCP server, disabling one capability only strips *that* capability's tools — the server keeps running for its other consumers. The server process is fully shut down only when this was the last referencer and no static role config owns it.

**Auto-activation.** Capabilities also auto-activate before each API call when the user's message strongly matches a capability's hand-authored triggers (semantic match via local embedding, no LLM in the loop). Activation uses a similarity threshold of 0.45 with a 0.08 abstain-on-tie margin and considers the top 3 trigger scores; the active set is bounded by an LRU eviction policy (soft cap of 4). See [Token Efficiency](16-token-efficiency.md#deterministic-auto-activation) for the full algorithm.

**Boot-time forcing.** Set `OCTOMIND_CAPABILITIES=cap1,cap2` to force-enable specific capabilities at startup — useful for non-interactive runs that need a deterministic tool surface (e.g. `OCTOMIND_CAPABILITIES=cron,docker octomind run -r ...`). Forced capabilities are still domain-gated.

## Filesystem Server Tools (octofs)

These tools are provided by the external `octofs` MCP server (command `octofs mcp`) running as a stdio subprocess. They are **not** a builtin — to have them you need:

1. The `octofs` binary on your `PATH`.
2. The built-in default tap [`muvon/tap`](../integration/04-tap-system.md) present (auto-cloned on first use), which ships the `filesystem-read` / `filesystem-write` capabilities that declare the `octofs` server.

A role references these tools through its `server_refs` / capabilities under the `filesystem` capability name (the default `assistant` role does this) — there is no hardcoded `[[mcp.servers]]` entry named `filesystem`. Without the tap and binary, these tools will not be present.

### `view` -- Read Files and Directories

Read files, view directories, and search file content.

```json
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

### `list_files` -- List Directory Entries

List files and directories at a path. Complements `view` (which reads content) when you only need the file listing.

**Parameters:**
- `path` (string): Directory to list (defaults to the current working directory)

```json
{"path": "src/"}
```

### `text_editor` -- File Editing

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

```json
{"command": "create", "path": "src/new.rs", "file_text": "pub fn hello() {}"}
{"command": "str_replace", "path": "src/main.rs", "old_str": "fn old()", "new_str": "fn new()"}
{"command": "insert", "path": "src/main.rs", "insert_line": 5, "new_str": "// Comment"}
{"command": "line_replace", "path": "src/main.rs", "view_range": [5, 8], "new_str": "fn updated() {}"}
{"command": "undo_edit", "path": "src/main.rs"}
```

### `batch_edit` -- Atomic Multi-Line Editing

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

### `extract_lines` -- Extract and Move Code

Extract lines from a source file and append to a target file without modifying the source.

**Parameters:**
- `from_path` (string): Source file
- `from_range` (array): `[start, end]` line numbers (1-indexed, inclusive)
- `append_path` (string): Target file (auto-created if needed)
- `append_line` (integer): Insert position (0=beginning, -1=end, N=after line N)

```json
{"from_path": "src/utils.rs", "from_range": [10, 25], "append_path": "src/extracted.rs", "append_line": -1}
```

### `shell` -- Shell Command Execution

Execute shell commands with output capture.

**Parameters:**
- `command` (string, required): Shell command
- `background` (boolean, default: false): Run in background, return PID

```json
{"command": "cargo test"}
{"command": "python -m http.server 8000", "background": true}
```

Background mode returns PID. Use `{"command": "kill <pid>"}` to terminate.

### `workdir` -- Working Directory Management

Get or set the working directory for file and shell operations.

**Parameters:**
- `path` (string): Set new working directory (absolute or relative)
- `reset` (boolean): Reset to original project directory

```json
{}
{"path": "/path/to/directory"}
{"reset": true}
```

Thread-local: changes only affect the current session.

### `ast_grep` -- AST-Based Code Search

Search and refactor code using AST patterns with ast-grep (sg).

**Parameters:**
- `pattern` (string, required): AST pattern using ast-grep syntax
- `language` (string): `"rust"`, `"javascript"`, `"typescript"`, `"python"`, `"go"`, `"java"`, `"c"`, `"cpp"`, `"php"`
- `paths` (array): File paths or glob patterns (default: current directory)
- `rewrite` (string): Rewrite pattern for refactoring
- `json_output` (boolean, default: false): JSON format output
- `context` (integer, default: 0): Context lines around matches
- `update_all` (boolean, default: false): Apply rewrites to all matches

```json
{"pattern": "console.log($$$)", "language": "javascript"}
{"pattern": "oldFunc($ARGS)", "rewrite": "newFunc($ARGS)", "language": "javascript"}
{"pattern": "class $NAME", "language": "php", "paths": ["src/**/*.php"], "context": 2}
```

## Agent Server Tools

Each agent configured in `[[agents]]` becomes a separate tool: `agent_<name>`.

**Parameters:**
- `task` (string, required): Task description for the agent
- `async` (boolean, default: false): Run asynchronously

**Sync (default):** Blocks until complete. Use when you need the result immediately.

**Async:** Returns immediately. Result appears as a user message when done. Use for tasks taking 30+ seconds when you can continue other work.

```json
{"task": "Analyze the authentication system architecture"}
{"task": "Review this function for performance", "async": true}
```

Max concurrent async jobs is configurable. Jobs cancelled on session exit.

## External MCP Servers

### Adding HTTP Servers

```toml
[[mcp.servers]]
name = "custom_api"
type = "http"
url = "https://api.example.com/mcp"
timeout_seconds = 30
tools = []
```

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

### Auto-Bind to Roles

```toml
[[mcp.servers]]
name = "my_server"
type = "http"
url = "http://localhost:3000/mcp"
auto_bind = ["developer", "assistant"]
```

### Tool Filtering

```toml
# Only expose specific tools
[[mcp.servers]]
name = "github_mcp"
type = "http"
url = "https://api.github.com/mcp"
tools = ["github_create_issue", "github_list_repos"]

# Wildcard filtering
tools = ["github_*"]
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
auto_bind = ["developer"]
```

This server will automatically be available for the `developer` role on next startup.

## Health Monitoring

MCP servers are monitored automatically:
- Health checks every 120 seconds for external servers (HTTP + stdio)
- Builtin servers are always considered healthy
- A failed server auto-restarts up to 3 times, waiting 30 seconds between restart attempts
- The failed-state flag is cleared after a 5-minute cooldown, allowing the server to be retried again (distinct from the 30-second between-attempt wait)
- Use `/mcp health` to force a health check

## Design Notes: Why Two Builtin Servers

The `core`/`runtime` split exists for two reasons: a clearer mental model, and a lower default token tax.

**The taxonomy.** `runtime` answers *"what am I?"* — its tools mutate the agent's identity: register a new MCP server, define a dynamic-agent class, load a skill instruction-pack. `core` answers *"how do I get this done?"* — planning a multi-step task, deferring work, growing the toolset on intent, delegating to a specialist. Most roles always want the second. Only specialized harness-authoring roles want the first.

**The token cost.** Every always-on tool is schema text the model stares at every turn, even when irrelevant. Splitting `runtime` out lets a typical role (`lawyer:sg`, `doctor:blood`, `developer:general`) drop those three tools from its surface entirely — they're never reached for during normal work, and exposing them just adds noise.

**Where new tools go.** When adding a tool, ask: *does it change what the agent **is**, or help the agent **work**?* Identity-change → `runtime`. Work-help → `core`. If neither fits cleanly, it's probably a [capability](#capability----discover-and-activate-domain-bundles) — a domain bundle activated on demand rather than a built-in.

**Direction of travel.** Even `core` is moving toward capability-gated rather than always-on. Today the essentials (`capability`, `tap`, `plan`) are always exposed because they're the bootstrap layer — `capability` loads everything else, `tap` is foundational delegation, `plan` is meta-cognition every role benefits from. Auxiliaries like `schedule`, and the entirety of `runtime`, are good candidates to migrate behind opt-in capability bundles authored in taps. The auto-activation pipeline already handles this for external capabilities; extending it to builtin tools is a tap-side authoring task.
