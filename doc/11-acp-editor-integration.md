# ACP Editor Integration

Octomind implements the [Agent Client Protocol (ACP)](https://agentclientprotocol.com/) — an open standard for connecting AI agents to code editors over JSON-RPC via stdio. This lets you use Octomind directly inside your editor with full session management, tool streaming, and MCP server injection.

## How It Works

When launched as an ACP agent, Octomind reads JSON-RPC messages from stdin and writes responses to stdout. Your editor manages the process lifecycle — it starts Octomind, sends prompts, receives streamed tool calls and assistant responses, and stops the process when done.

```
Editor (ACP client)  ←──JSON-RPC/stdio──→  octomind acp
```

The editor passes its working directory and optionally its own MCP servers to Octomind on session creation. Octomind runs your configured role (default: `developer`) with all its tools, layers, and workflows.

## Starting Octomind as an ACP Agent

```bash
octomind acp
# or with a specific role:
octomind acp --role developer
octomind acp --role assistant
```

The `developer` role includes all MCP tools (shell, file editing, search, etc.). The `assistant` role is a simpler chat without tools — useful for editors that want to manage tools themselves.

> **Note:** Never run this manually in a terminal — it speaks JSON-RPC over stdio. Configure it in your editor as shown below.

---

## Neovim

Two plugins support ACP in Neovim. **CodeCompanion** is the more mature option; **avante.nvim** is an alternative with a Cursor-like UI.

### Option A — CodeCompanion.nvim (recommended)

**1. Install the plugin** (lazy.nvim example):

```lua
{
  "olimorris/codecompanion.nvim",
  dependencies = {
    "nvim-lua/plenary.nvim",
    "nvim-treesitter/nvim-treesitter",
  },
  opts = {},
}
```

**2. Add Octomind as a custom ACP adapter:**

```lua
require("codecompanion").setup({
  adapters = {
    acp = {
      octomind = function()
        return require("codecompanion.adapters").extend("acp", {
          name = "octomind",
          commands = {
            default = { "octomind", "acp", "--role", "developer" },
          },
        })
      end,
    },
  },
  interactions = {
    chat = {
      adapter = "octomind",
    },
  },
})
```

**3. Open a chat buffer:**

```
:CodeCompanionChat
```

Or use the action palette: `:CodeCompanionActions` → select **New Chat**.

**Key bindings** (defaults):
| Key | Action |
|-----|--------|
| `<C-s>` | Send message |
| `ga` | Add file to context |
| `gq` | Close chat |

---

### Option B — avante.nvim

**1. Install:**

```lua
{
  "yetone/avante.nvim",
  event = "VeryLazy",
  build = "make",
  dependencies = {
    "nvim-lua/plenary.nvim",
    "MunifTanjim/nui.nvim",
  },
  opts = {
    provider = "octomind",
    acp_providers = {
      ["octomind"] = {
        command = "octomind",
        args = { "acp", "--role", "developer" },
      },
    },
  },
}
```

**2. Open the panel:** `<Leader>aa` (default) or `:AvanteAsk`

---

## Zed

Zed has native ACP support built in.

**1. Open settings:** `Cmd+,` → edit `settings.json`

**2. Add Octomind under `agent_servers`:**

```json
{
  "agent_servers": {
    "Octomind": {
      "type": "custom",
      "command": "octomind",
      "args": ["acp", "--role", "developer"]
    }
  }
}
```

**3. Open the agent panel:** `Cmd+?`

**4. Click `+`** → select **Octomind** from the list.

Zed streams tool calls and assistant responses in real time with diff previews and multi-buffer editing.

---

## JetBrains IDEs

ACP support is available in IntelliJ IDEA, PyCharm, WebStorm, GoLand, and all other JetBrains IDEs via the **AI Assistant** plugin (version 2025.3.2+).

**1. Open AI Chat:** `View → Tool Windows → AI Assistant`

**2. Open ACP settings:** click the agent selector button → **Edit acp.json**

**3. Add Octomind:**

```json
{
  "agent_servers": {
    "Octomind": {
      "command": "octomind",
      "args": ["acp", "--role", "developer"]
    }
  }
}
```

**4. Select the agent:** click the agent selector in the AI Chat toolbar → choose **Octomind**.

> **Tip:** JetBrains IDEs don't inherit your shell PATH. If `octomind` isn't found, use the full path: `"/Users/you/.cargo/bin/octomind"` (macOS/Linux) or the output of `which octomind`.

---

## Passing MCP Servers from the Editor

ACP clients can inject their own MCP servers into Octomind sessions. Octomind merges them with its own configured servers — the editor's servers become available as tools for that session.

**Supported transports:** `stdio` and `http`. SSE servers are silently skipped (not supported by Octomind's MCP stack).

This is handled automatically by editors that support MCP server forwarding (e.g. CodeCompanion with `mcpServers = "inherit_from_config"`):

```lua
-- CodeCompanion: forward editor-configured MCP servers to Octomind
require("codecompanion").setup({
  adapters = {
    acp = {
      octomind = function()
        return require("codecompanion.adapters").extend("acp", {
          name = "octomind",
          commands = {
            default = { "octomind", "acp" },
          },
          defaults = {
            mcpServers = "inherit_from_config",
          },
        })
      end,
    },
  },
})
```

---

## Roles

| Role | Tools | Use case |
|------|-------|----------|
| `developer` | Shell, file editing, search, web, agents | Default — full coding assistant |
| `assistant` | None | Simple chat, editor manages tools |

Switch roles by changing the `--role` argument in your editor config.

---

## Troubleshooting

**Agent not found / process fails to start**
- Run `octomind acp` in your terminal to confirm it's installed and on PATH.
- Use the absolute path in editor config if needed: `which octomind`.

**No response / hangs**
- Octomind writes JSON-RPC to stdout. Make sure nothing else in your shell config (`~/.zshrc`, `~/.bashrc`) prints to stdout — that corrupts the protocol.
- Check that your Octomind config is valid: `octomind config show`.

**Tools not available**
- Confirm the role has MCP servers enabled in your config (`config-templates/default.toml` or `~/.config/octomind/config.toml`).
- Use `--role developer` explicitly.

**Editor-injected MCP server not working**
- Only `stdio` and `http` transports are supported. SSE servers are skipped automatically.
- Check the Octomind log for `ACP: skipping SSE MCP server` messages to confirm.

**JetBrains: agent not shown after editing acp.json**
- Save the file — agents appear immediately without restart.
- Verify JSON syntax is valid.
