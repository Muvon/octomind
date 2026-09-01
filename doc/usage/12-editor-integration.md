# Editor Integration

Editor integration exposes Octomind's ACP runtime, sessions, tools, commands, and streaming events through ACP-capable clients.

## Features

- Full session management with tool access
- Streaming tool execution with real-time feedback
- Slash commands available in the editor (advertised over ACP)
- Image and video attachments (clients can attach images inline; video arrives as embedded blob resources)
- MCP server injection from editor config (stdio and HTTP transports)
- Cost and token-usage reporting via an ACP `_meta` side-channel
- Background inbox monitor: schedules, monitors, tap runs, detached jobs, skills, guardrails, and async agent results can appear mid-session
- Role-based access control

## How It Works

Octomind runs as an ACP agent over stdio using JSON-RPC:

```bash
octomind acp [TAG]
```

The editor launches this as a subprocess and communicates via JSON-RPC on stdio. Protocol diagnostics go to files under `~/.local/share/octomind/logs/` so stdout/stderr are not polluted by normal logging.

`TAG` is optional. When omitted, the agent uses the default role from your config (the shipped default is `assistant:concierge`). `TAG` can be:

- A **local role name** from your config (e.g. `assistant`), or
- A **tap agent** addressed as `category:variant` (e.g. `developer:general`).

> `developer:general` is a tap-registry agent provided by the built-in default tap `muvon/tap`, not a local config role. The stock config ships the roles `assistant`, `task_refiner`, `task_researcher`, and `reduce`. If you point an editor at `developer:general`, make sure the tap is installed (it is the default tap), otherwise the agent will fail to resolve the tag. To select a local `[[roles]]` entry, pass `assistant` explicitly; omitting `TAG` uses the `assistant:concierge` tap default.

Each ACP session also spawns a background inbox monitor. It processes internally queued schedules, monitors, tap runs, detached jobs, skills, guardrail feedback, and background-agent results without waiting for a user prompt; these arrive in the editor as user-side message chunks. ACP does not start the `octomind send` or webhook listeners owned by `octomind run`.

### `octomind acp` flags

| Flag | Description |
|------|-------------|
| `TAG` | Agent tag (e.g. `developer:general`) or local role name. Omit for the config default. |
| `--name`, `-n` | Preferred session name for the next `new_session` request |
| `--resume`, `-r` | Resume a specific session by name on the next `new_session` |
| `--resume-recent` | Resume the most recent session for the current working directory |
| `--model`, `-m` | Override the model name for sessions started by this agent (runtime > role > tap > main `[model]`) |
| `--sandbox` | Restrict all filesystem writes to the current working directory |
| `--hook` | Parsed and carried into ACP session options, but ACP does not currently start webhook listeners |

## Neovim

> The editor-side snippets below are illustrative. Plugin configuration shapes change over time; confirm against each plugin's current docs.

### CodeCompanion.nvim

CodeCompanion does not ship a built-in `octomind` adapter, so you configure Octomind as a custom ACP adapter. Adjust to match the version of CodeCompanion you have installed.

```lua
require("codecompanion").setup({
  adapters = {
    octomind = function()
      return require("codecompanion.adapters").extend("octomind", {
        command = "octomind",
        args = { "acp", "developer:general" },
      })
    end,
  },
  strategies = {
    chat = { adapter = "octomind" },
    inline = { adapter = "octomind" },
  },
})
```

To select the explicit local `[[roles]]` entry instead of a tap agent, use `args = { "acp", "assistant" }`; `{ "acp" }` uses the `assistant:concierge` tap default.

### avante.nvim

```lua
require("avante").setup({
  provider = "octomind",
  vendors = {
    octomind = {
      command = "octomind",
      args = { "acp", "developer:general" },
    },
  },
})
```

## Zed

Zed has native ACP support and configures external ACP agents under `agent_servers` with a `command` and `args`. Add to your Zed `settings.json`:

```json
{
  "agent_servers": {
    "Octomind": {
      "command": "octomind",
      "args": ["acp", "developer:general"]
    }
  }
}
```

Replace `developer:general` with `assistant` (or drop the second arg) to use a local role instead of the tap agent. See Zed's external-agent configuration docs for the authoritative schema.

## JetBrains IDEs

Supported via the AI Assistant plugin. Configure an external ACP agent:

1. Open **Settings > Tools > AI Assistant**
2. Add external agent
3. Set command: `octomind acp developer:general` (or `octomind acp assistant` for a local role)

## MCP Server Injection

Editors can inject additional MCP servers into the Octomind session through the ACP `initialize` / `new_session` handshake. Behavior:

- **Per-session scope**: injected servers are merged into a per-session config snapshot and added to the role's `server_refs` for that session only. Your base config is never mutated.
- **Supported transports**: `stdio` and `HTTP` only. The agent advertises HTTP MCP support (`mcp_capabilities.http = true`) during initialization, so clients offer HTTP servers.
- **Unsupported transports**: `SSE` and any unknown transport are skipped (logged, not connected).
- **Timeout**: injected servers use a hardcoded 30-second timeout.

The agent also advertises `load_session` support, so clients can resume sessions by ID.

## Available Slash Commands

The ACP agent currently advertises **26 command names** during the session. Names are sent **without the leading `/`** — the client prepends it when displaying:

`help`, `role`, `model`, `done`, `info`, `clear`, `copy`, `context`, `list`, `session`, `run`, `workflow`, `mcp`, `plan`, `prompt`, `image`, `video`, `loglevel`, `report`, `skill`, `effort`, `schedule`, `agents`, `usage`, `login`, `exit`

Notes:

- This advertised set is a subset of the full session registry. Commands such as `/learning`, `/share`, `/analyze`, `/rename`, and `/status` are not advertised over ACP.
- `/done` is handled specially in ACP: it compresses the conversation and reports the result. If you pass trailing instructions (`/done <instructions>`), the agent compresses first, sends the compression status, then processes the instructions as a normal prompt.
- Three advertised names are not wired into the shared slash-command dispatcher: `session`, `workflow`, and `agents`. ACP reports them as unsupported if invoked. Use `/new`/`/list` for session management, `octomind workflow [NAME|FILE]` externally, and `/status agents` for agent activity.
- `/effort` accepts `low`, `medium`, `high`, `xhigh`, or `max` (the advertised input hint only shows the first three).
- Editors that support arbitrary slash input may send other registered commands even when they are absent from the menu; unknown slash commands receive an unsupported-command response rather than reaching the model.

### Programmatic command execution

Beyond the slash-command menu, clients can invoke commands programmatically through the ACP extension method namespace `octomind/command`. The request carries `{ session_id, command, args }` and the response returns `{ success, output, error }` with structured JSON output. This lets editor integrations run session commands without routing them through the prompt stream.

## Cost and Usage Reporting

As a session runs, the agent emits a `SessionInfoUpdate` notification carrying a `_meta["octomind.usage"]` payload with `session_tokens`, `session_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, and `reasoning_tokens`. Clients that pass `_meta` through can display live cost and token usage.

## Roles

The role you pass to `octomind acp` determines which tools the session can use.

- **`assistant`** (shipped config role, full access) — `core`, `orchestration`, `runtime`, external `filesystem`, and `agent`, with one `server:*` allow pattern for each.
- **`task_refiner`** — lightweight query refinement; no MCP servers.
- **`task_researcher`** — read-only reconnaissance; it requests the external `filesystem` server with only `view`, so that tool is present only when the companion server resolves.
- **`reduce`** — session-history compression; special-purpose.
- **Tap agents** like `developer:general` provide richer development presets and come from the built-in default tap `muvon/tap`.
- Custom roles work the same as in CLI sessions.

## Troubleshooting

**Agent not found:**
Ensure `octomind` is on your PATH. Try `octomind acp` for the default `assistant:concierge` tap agent, or `octomind acp developer:general`; confirm the default tap is installed.

**No response / hangs:**
- For the shipped `octohub:auto` profile, run `octomind login`; for a direct provider model, ensure its credential variable reaches the editor process
- Editor may need to inherit shell environment variables
- Check `~/.local/share/octomind/logs/acp-debug.log` for runtime errors

**Tools not available:**
- Verify the role has correct `server_refs` and `allowed_tools`
- Check `~/.local/share/octomind/logs/acp-errors.jsonl` for structured error details

**Agent fails to start at all:**
- In ACP mode stdout/stderr are reserved for JSON-RPC, so startup failures are written to `~/.local/share/octomind/logs/acp-init-errors.log`

**JetBrains issues:**
- Ensure AI Assistant plugin is up to date
- The plugin must support external ACP agents

## See Also

- [ACP Protocol](../integration/02-acp-protocol.md) — full handshake, capabilities, and session lifecycle
- [WebSocket Server](../integration/01-websocket-server.md) — alternative integration transport
- [CLI Reference](../reference/01-cli-reference.md) — complete `octomind` command and flag reference
- [Session Commands](../reference/02-session-commands.md) — all interactive session commands
