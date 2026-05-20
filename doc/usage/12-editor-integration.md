# Editor Integration

Octomind integrates with code editors via the ACP (Agent Client Protocol), providing AI assistance directly in your IDE.

## Features

- Full session management with tool access
- Streaming tool execution with real-time feedback
- Slash commands available in editor
- MCP server injection from editor config
- Role-based access control

## How It Works

Octomind runs as an ACP agent over stdio using JSON-RPC:

```bash
octomind acp [TAG]
```

The editor launches this as a subprocess and communicates via JSON-RPC messages on stdin/stdout.

## Neovim

### CodeCompanion.nvim (recommended)

```lua
require("codecompanion").setup({
  adapters = {
    octomind = function()
      return require("codecompanion.adapters").extend("octomind", {
        command = "octomind",
        args = { "acp", "developer" },
      })
    end,
  },
  strategies = {
    chat = { adapter = "octomind" },
    inline = { adapter = "octomind" },
  },
})
```

### avante.nvim

```lua
require("avante").setup({
  provider = "octomind",
  vendors = {
    octomind = {
      command = "octomind",
      args = { "acp", "developer" },
    },
  },
})
```

## Zed

Zed has native ACP support. Add to your Zed settings:

```json
{
  "language_models": {
    "octomind": {
      "binary": "octomind",
      "args": ["acp", "developer"]
    }
  }
}
```

## JetBrains IDEs

Supported via the AI Assistant plugin. Configure an external ACP agent:

1. Open **Settings > Tools > AI Assistant**
2. Add external agent
3. Set command: `octomind acp developer`

## MCP Server Injection

Editors can inject additional MCP servers into the Octomind session. The servers are passed via the ACP initialization handshake.

## Available Slash Commands

All 28 session commands are available via ACP:

`/help`, `/?`, `/exit`, `/quit`, `/copy`, `/clear`, `/list`, `/session`, `/info`, `/done`, `/loglevel`, `/model`, `/run`, `/mcp`, `/report`, `/image`, `/video`, `/context`, `/role`, `/prompt`, `/plan`, `/workflow`, `/skill`, `/effort`, `/schedule`, `/learning`, `/share`, `/analyze`

## Roles

- **developer** -- full tool access (file editing, shell, code analysis)
- **assistant** -- chat-only, limited tools
- Custom roles work the same as in CLI sessions

## Troubleshooting

**Agent not found:**
Ensure `octomind` is on your PATH. Try running `octomind acp developer` in terminal first.

**No response / hangs:**
- Check that the API key is set in your shell environment
- Editor may need to inherit shell environment variables
- Check `~/.local/share/octomind/logs/acp-debug.log` for errors

**Tools not available:**
- Verify the role has correct `server_refs` and `allowed_tools`
- Check `~/.local/share/octomind/logs/acp-errors.jsonl` for details

**JetBrains issues:**
- Ensure AI Assistant plugin is up to date
- The plugin must support external ACP agents
