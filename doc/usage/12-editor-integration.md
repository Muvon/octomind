# Editor Integration

Octomind integrates with code editors via the ACP (Agent Client Protocol), providing AI assistance directly in your IDE.

## Features

- Full session management with tool access
- Streaming tool execution with real-time feedback
- Slash commands available in the editor (advertised over ACP)
- Image and video attachments (clients can attach images inline; video arrives as embedded blob resources)
- MCP server injection from editor config (stdio and HTTP transports)
- Cost and token-usage reporting via an ACP `_meta` side-channel
- Background inbox monitor: scheduled messages, webhook injections, and async agent results appear mid-session
- Role-based access control

## How It Works

Octomind runs as an ACP agent over stdio using JSON-RPC:

```bash
octomind acp [TAG]
```

The editor launches this as a subprocess and communicates via JSON-RPC messages on stdin/stdout. Stderr is reserved for logging (it never carries protocol traffic).

`TAG` is optional. When omitted, the agent uses the default role from your config (the shipped default is `assistant:concierge`). `TAG` can be:

- A **local role name** from your config (e.g. `assistant`), or
- A **tap agent** addressed as `category:variant` (e.g. `developer:general`).

> `developer:general` is a tap-registry agent provided by the built-in default tap `muvon/tap`, not a local config role. The stock config ships the roles `assistant`, `task_refiner`, `task_researcher`, and `reduce`. If you point an editor at `developer:general`, make sure the tap is installed (it is the default tap), otherwise the agent will fail to resolve the tag. To stay fully local, omit the tag or use `assistant`.

Each ACP session also spawns a background inbox monitor. It processes scheduled messages (`/schedule`), webhook injections, and background-agent results without waiting for a user prompt; these arrive in the editor as user-side message chunks. See the [ACP Protocol reference](../integration/02-acp-protocol.md) for the full handshake and session lifecycle.

### `octomind acp` flags

| Flag | Description |
|------|-------------|
| `TAG` | Agent tag (e.g. `developer:general`) or local role name. Omit for the config default. |
| `--name`, `-n` | Preferred session name for the next `new_session` request |
| `--resume`, `-r` | Resume a specific session by name on the next `new_session` |
| `--resume-recent` | Resume the most recent session for the current working directory |
| `--model`, `-m` | Override the model for all sessions started by this agent (priority: `--model` > `role.model` > `config.model`; a `[taps]` entry overrides `config.model` for tap agents) |
| `--sandbox` | Restrict all filesystem writes to the current working directory |
| `--hook` | Activate a webhook hook by name (defined in `[[hooks]]` config); repeatable |

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

To stay fully local without the tap registry, use `args = { "acp", "assistant" }` (or `{ "acp" }` to use the config default).

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

The ACP agent advertises **23 commands** during the session. Names are sent **without the leading `/`** — the client prepends it when displaying:

`help`, `role`, `model`, `done`, `info`, `clear`, `copy`, `context`, `list`, `session`, `run`, `workflow`, `mcp`, `plan`, `prompt`, `image`, `video`, `loglevel`, `report`, `skill`, `effort`, `schedule`, `exit`

Notes:

- This advertised set is a subset of the full CLI session commands. Commands like `/learning`, `/share`, and `/analyze` are not advertised over ACP.
- `/done` is handled specially in ACP: it compresses the conversation and reports the result. If you pass trailing instructions (`/done <instructions>`), the agent compresses first, sends the compression status, then processes the instructions as a normal prompt.
- The advertised `workflow` command is a **legacy no-op** — `/workflow` was removed; run multi-step workflows via the `octomind workflow <file.toml>` CLI.
- `/effort` accepts `low`, `medium`, `high`, `xhigh`, or `max` (the advertised input hint only shows the first three).
- Editors that support arbitrary slash input may still send other commands as prompts; only the 23 above are surfaced in the client command menu.

### Programmatic command execution

Beyond the slash-command menu, clients can invoke commands programmatically through the ACP extension method namespace `octomind/command`. The request carries `{ session_id, command, args }` and the response returns `{ success, output, error }` with structured JSON output. This lets editor integrations run session commands without routing them through the prompt stream.

## Cost and Usage Reporting

As a session runs, the agent emits a `SessionInfoUpdate` notification carrying a `_meta["octomind.usage"]` payload with `session_tokens`, `session_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, and `reasoning_tokens`. Clients that pass `_meta` through can display live cost and token usage.

## Roles

The role you pass to `octomind acp` determines which tools the session can use.

- **`assistant`** (shipped default, full access) -- file editing, shell, and code analysis via the `core`, `runtime`, `filesystem`, and `agent` MCP servers (`allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]`).
- **`task_refiner`** -- lightweight query refinement; no MCP servers.
- **`task_researcher`** -- read-only reconnaissance; `filesystem` server with only the `view` tool allowed.
- **`reduce`** -- session-history compression; special-purpose.
- **Tap agents** like `developer:general` provide richer development presets and come from the built-in default tap `muvon/tap`.
- Custom roles work the same as in CLI sessions.

## Troubleshooting

**Agent not found:**
Ensure `octomind` is on your PATH. Try running `octomind acp assistant` in a terminal first. If you use a tap agent like `developer:general`, confirm the tap is installed.

**No response / hangs:**
- Check that the API key is set in your shell environment
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

- [ACP Protocol](../integration/02-acp-protocol.md) -- full handshake, capabilities, and session lifecycle
- [WebSocket Server](../integration/01-websocket-server.md) -- alternative integration transport
- [CLI Reference](../reference/01-cli-reference.md) -- complete `octomind` command and flag reference
- [Session Commands](../reference/02-session-commands.md) -- all interactive session commands
