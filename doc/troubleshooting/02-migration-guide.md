# Migration Guide

## Provider Format

**Old format:**
```toml
model = "anthropic/claude-sonnet-4"
```

**Current format:**
```toml
model = "openrouter:anthropic/claude-sonnet-4"
```

All models require `provider:model` format. The provider prefix tells Octomind which API to use.

## MCP Configuration

**Old format:**
```toml
[mcp]
enabled = true
providers = ["core"]
```

**Current format:**
```toml
[mcp]
allowed_tools = []

[[mcp.servers]]
name = "core"
type = "builtin"
timeout_seconds = 30
tools = []
```

Each server is now an explicit entry in `[[mcp.servers]]` with type, timeout, and tool filtering.

## Role Configuration

**Old format:**
```toml
[developer]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

[developer.mcp]
enabled = true
server_refs = ["core"]
```

**Current format:**
```toml
[[roles]]
name = "developer"
model = "openrouter:anthropic/claude-sonnet-4"

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

Key changes:
- Roles use `[[roles]]` array format (not `[role_name]` sections)
- `enabled` field removed (roles are always available if defined)
- `enable_layers` removed (layers are controlled by workflows)
- Tool permissions use `allowed_tools` patterns
- `runtime` builtin server is new — see "Runtime Namespace Move" below

## Layer Configuration

**Old format:**
```toml
[[layers]]
name = "task_refiner"
builtin = true
enabled = true
enable_tools = true
```

**Current format:**
```toml
[[layers]]
name = "task_refiner"
description = "Refines and clarifies user requests"
model = "openrouter:openai/gpt-4.1-mini"
max_tokens = 2048
```

The main AI uses these agents as tools:
- `builtin`, `enabled`, `enable_tools` fields removed
- `description` is now required
- Layer is active when referenced by a workflow or command

## Config File Location

**Old location:** `~/.config/octomind/config.toml` or `~/.octomind/config.toml`

**Current location:** `~/.local/share/octomind/config/config.toml`

Override: `OCTOMIND_CONFIG_PATH` environment variable.

## Automatic Upgrade

```bash
octomind config --upgrade
```

This attempts to migrate your config to the latest version. Review the result and adjust manually if needed.

## Runtime Namespace Move

The `core` builtin server was split into two: high-level tools stay in `core`, low-level harness-control tools moved to a new `runtime` server.

| Tool | Old server | New server |
|------|------------|------------|
| `plan` | `core` | `core` |
| `schedule` | `core` | `core` |
| `capability` | `core` | `core` |
| `tap` *(new)* | -- | `core` |
| `mcp` | `core` | **`runtime`** |
| `agent` | `core` | **`runtime`** |
| `skill` | `core` | **`runtime`** |

If your config or tap manifest has `server_refs = ["core", ...]` and the role calls any of `mcp`, `agent`, or `skill`, add `"runtime"` to the list:

```diff
 [roles.mcp]
-server_refs = ["core", "filesystem", "agent"]
-allowed_tools = ["core:*", "filesystem:*", "agent:*"]
+server_refs = ["core", "runtime", "filesystem", "agent"]
+allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

The `runtime` server is registered automatically in the default config:

```toml
[[mcp.servers]]
name = "runtime"
type = "builtin"
timeout_seconds = 30
tools = []
```

If you have a hand-rolled config without it, add the block.

Roles that don't call `mcp`/`agent`/`skill` (most roles) don't need `"runtime"` at all — drop it from `server_refs` to keep the tool surface tighter.

## Filesystem Is Now External

`filesystem` is no longer a builtin server. It's served by an external `octofs` process configured as a `stdio` server. The default config already wires this:

```toml
[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
args = ["mcp", "--path={{CWD}}"]
timeout_seconds = 30
tools = []
```

If your config still declares `filesystem` as `type = "builtin"`, switch it to the stdio block above. `octomind config --upgrade` does this automatically.

## MCP Server Type

**Old (incorrect in some docs):** `type = "stdin"`

**Correct:** `type = "stdio"`

The server type for local process-based MCP servers is `"stdio"`, not `"stdin"`.

## Session Commands

**Removed:**
- `/save` -- Session persistence is automatic on exit

**Added:**
- `/skill` -- Manage skills (list, use, forget)

Use `/help` to see current command list.
