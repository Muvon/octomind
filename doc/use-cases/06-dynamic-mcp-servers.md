# Use Case: AI Self-Configuring MCP Servers at Runtime

Let the AI discover what tools it needs and connect new MCP servers on the fly -- no config changes, no restarts.

## The Problem

You don't always know upfront which tools a session will need. A developer asks "help me with the GitHub issues" but the GitHub MCP server isn't configured. Or the AI realizes mid-task it needs a database tool. Traditionally, you'd stop, edit config, restart. Octomind lets the AI handle this itself.

## Solution

The built-in `mcp` tool lets the AI add, enable, disable, remove, and persist MCP servers during a live session. This tool lives in the `runtime` builtin MCP server, so it is only available when the AI's role grants access to `runtime` (the default `assistant` role does — `server_refs` includes `runtime` with `runtime:*` allowed). A custom role that drops `runtime` cannot self-configure.

> The conversational snippets below are simplified illustrations of an agent's reasoning and the resulting tool calls. Real tool result strings differ in wording (for example, `enable` returns `Server '<name>' enabled with N tools: ...`).

### How It Works

The AI can manage its own tool ecosystem at runtime:

```
User: "Check the open GitHub issues for this repo"

AI (thinking): "I don't have GitHub tools. Let me add the GitHub MCP server."

AI calls: mcp(action="add", name="github", server_type="http",
          url="https://api.github.com/mcp")

AI calls: mcp(action="enable", name="github")

AI: "Connected to GitHub MCP server. Found 12 open issues..."
```

No config changes. No restart. The AI recognized a gap and filled it.

> The `url` above is an illustrative endpoint, not a real GitHub API. Substitute the actual MCP server URL you want to connect to.

### Example: Full Dynamic Tool Discovery

```
User: "I need to analyze our database schema and cross-reference with GitHub issues"

AI:
  1. mcp(action="list")
     -> Configured servers:

          core [builtin] ✓ active → (all tools)
          runtime [builtin] ✓ active → (all tools)
          agent [builtin] ✓ active → (all tools)

        (no GitHub, no DB tools yet)

  2. mcp(action="add", name="github", server_type="http",
         url="https://api.github.com/mcp")
     -> "Server 'github' registered"

  3. mcp(action="enable", name="github")
     -> "Server 'github' enabled with 12 tools:
         github_list_issues, github_search_code, ..."

  4. mcp(action="add", name="db", server_type="stdio",
         command="mcp-postgres", args=["--connection", "postgresql://..."])
     -> "Server 'db' registered"

  5. mcp(action="enable", name="db")
     -> "Server 'db' enabled with 3 tools:
         query, list_tables, describe_table"

  6. Now uses both servers to cross-reference issues with schema

  7. When done:
     mcp(action="disable", name="db")  # Clean up sensitive connection
```

The `list` action groups servers under a `Configured servers:` header (those declared in config — by default `core`, `runtime`, and `agent`) and a separate `Dynamic servers:` header for anything added at runtime. Each line shows `name [type] status → tools`, a 💾 marker if the server is persisted, and `(all tools)` when no tool filter is set. The default configuration declares only the three builtin servers; a `filesystem` server appears only if a tap or your own config adds one.

### Persisting Dynamic Servers

If a dynamically added server proves useful, persist it to config:

```
AI calls: mcp(action="persist", name="github")
-> Saved to ~/.local/share/octomind/config/mcp-github.toml
```

Whether the persisted server auto-loads next session depends on its state at persist time:

- **Enabled when persisted** -> `auto_bind` is set to the current role, so it loads automatically in future sessions for that role.
- **Disabled when persisted** -> the config is still written, but `auto_bind` is cleared (the tool reports "Auto-bind cleared (server disabled)"), so it won't auto-load until you enable and persist it again.

Remove persisted config:
```
AI calls: mcp(action="unpersist", name="github")
-> Removed from config directory
```

### Stdio Servers (Local Tools)

The AI can spin up local tool servers too:

```
AI calls: mcp(action="add", name="octocode", server_type="stdio",
          command="octocode", args=["mcp", "--path=."], timeout_seconds=240)

AI calls: mcp(action="enable", name="octocode")
-> "Server 'octocode' enabled with 3 tools:
    semantic_search, view_signatures, graphrag"
```

`timeout_seconds` is optional and defaults to **60**. Raise it (as above) only for servers that take a long time to respond.

### Tool Filtering

Only expose specific tools from a server:

```
AI calls: mcp(action="add", name="github", server_type="http",
          url="https://api.github.com/mcp",
          tools=["github_list_issues", "github_create_issue"])
```

Or use wildcards:
```
tools=["github_*"]
```

The `enable` action also accepts a `tools` filter, applied when the server actually connects. It overrides whatever filter was registered at `add` time, so you can register a server broadly and narrow its exposed tools later:

```
AI calls: mcp(action="enable", name="github", tools=["github_list_issues"])
```

### Dynamic Agents Too

The same pattern works for agents. The AI can create specialized agents at runtime:

```
AI calls: agent(action="add", name="db_expert",
           description="Analyzes database schemas and queries",
           system="You are a database expert...",
           server_refs=["filesystem", "db"],
           allowed_tools=["filesystem:view", "db:*"])

AI calls: agent(action="enable", name="db_expert")

AI calls: agent_db_expert(task="Analyze the schema for N+1 query risks")
```

Once enabled, the agent becomes callable as a tool named `agent_<name>` (here `agent_db_expert`). A few details worth knowing:

- **`server_refs` is validated at add time.** Any name you list explicitly must already exist as a configured server or a dynamic server you have added; an unknown reference fails the call with `Server '<name>' not found. Available servers: ...`. (In the example, `filesystem` must be available — for instance provided by a tap — and `db` must have been added first.)
- **`server_refs` can be inferred.** If you omit `server_refs` but provide `allowed_tools`, Octomind auto-populates `server_refs` from the server prefixes in the tool names (so `allowed_tools=["db:*"]` implies `server_refs=["db"]`).

## Practical Scenario: Self-Assembling Toolkit

Prompt the AI to assess what it needs:

```
User: "You have access to the filesystem. Before you start working on this project,
       check what tools would be useful and set them up."

AI:
  1. Reads project files: package.json, Cargo.toml, docker-compose.yml
  2. Discovers: Rust project with PostgreSQL, uses GitHub for issues
  3. Sets up:
     - octocode (code search)
     - postgres MCP (database)
     - github MCP (issues/PRs)
  4. Reports: "I've connected 3 additional tool servers. Ready to work."
```

## MCP Management Commands

| Action | Description |
|--------|-------------|
| `list` | Show all servers with status |
| `add` | Register new server (doesn't connect; `timeout_seconds` defaults to 60) |
| `enable` | Connect and activate tools (optional `tools` filter applied at connect time) |
| `disable` | Deactivate (keep config) |
| `remove` | Unregister a dynamically added server (does not remove config-defined servers) |
| `persist` | Save to config dir; auto-binds to the current role only if enabled |
| `unpersist` | Remove from saved config |

### AI tool vs human command — two separate surfaces

It is important not to confuse two things that look similar:

- **The `mcp` tool (AI-driven).** This is what the AI calls to *mutate* its tool ecosystem: `add`, `enable`, `disable`, `remove`, `persist`, `unpersist`, and `list`. Everything in this document is about this tool.
- **The `/mcp` interactive command (human-driven, read-only).** When *you* type `/mcp` in a session, it only inspects state. Its subcommands are `info` (default), `list`, `full`, `health`, `dump`, and `validate`. There is no `/mcp add`, `/mcp enable`, `/mcp remove`, or `/mcp persist` — typing those returns an unknown-subcommand error. Runtime server mutation is exclusively the AI's job via the `mcp` tool.

## Key Points

- The `mcp` tool lives in the `runtime` builtin server; the AI's role must grant `runtime` access (the default `assistant` role does) for self-configuration to be possible
- The AI can add/remove MCP servers without human intervention
- Supports both HTTP (remote) and stdio (local process) servers
- `timeout_seconds` is optional and defaults to 60
- `persist` saves a dynamic server to the config dir; it auto-binds to the current role only when the server is enabled at persist time (disabled servers persist with `auto_bind` cleared)
- `remove` only unregisters dynamic (runtime-added) servers — it cannot delete servers defined in config
- Tool filtering prevents exposing unnecessary capabilities, and a filter can be applied at `add` time or refined at `enable` time
- Dynamic agents follow the same add/enable/disable/remove pattern, are callable as `agent_<name>`, and validate their `server_refs` at add time
- The `mcp` tool (AI-driven, mutating) and the `/mcp` command (human-driven, read-only) are distinct surfaces with non-overlapping actions
- This is unique to Octomind -- most AI tools require static tool configuration
- The AI can assess a project and self-configure its toolkit
