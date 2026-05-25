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
- `enable_layers` removed (legacy in-session workflow system is gone — use external `octomind workflow` instead)
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
command = "octomind acp task_refiner"
input_mode = "last"
output_mode = "none"
output_role = "assistant"
```

Key changes:
- `builtin`, `enabled`, `enable_tools`, `model`, `max_tokens` fields removed
- `description` and `command` are required
- Model/system/MCP config lives in the `[[roles]]` entry that `command` references
- Layer is active when referenced by a `/run` command

## In-Session Workflows Removed

The `[[workflows]]` config section and the `/workflow` session command have been removed. Multi-step AI orchestration is now an external CLI: `octomind workflow <file.toml>`.

If you previously had `workflow = "..."` on a role, drop the field. To port an existing in-session workflow, rewrite it as an external workflow TOML — see [doc/usage/09-workflows.md](../usage/09-workflows.md).

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
