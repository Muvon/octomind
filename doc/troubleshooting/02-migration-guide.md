# Migration Guide

> **Read this first:** the current schema is version `12`. `octomind config --upgrade` performs the declared version-by-version migrations, including the v12 nested model-profile migration, and writes a backup before replacement. Very old pre-version structural formats may still need the manual changes documented below.

## Model Profile and Provider Format

**Old format:**
```toml
model = "anthropic/claude-sonnet-4"
```

**Current format:**
```toml
[model]
name = "openrouter:anthropic/claude-sonnet-4"
reasoning_effort = "medium"
max_tokens = 32768
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

All model names require `provider:model` format. `[model]` is the required complete baseline. Optional `[roles.model]`, `[supervisor.model]`, and `[compression.model]` profiles may override any subset; omitted fields inherit from main. Tap and workflow mappings remain name-only.

When migrating a bare model name, pick the provider prefix that matches where you actually call the model. Common prefixes: `openrouter`, `openai`, `anthropic`, `google` (Vertex), `amazon` (Bedrock), `cloudflare`, `deepseek`, `ollama`, `local`, and the special `cli` meta-provider for locally CLI-backed models. There are 20 network providers plus `cli` in total — see [doc/usage/04-providers.md](../usage/04-providers.md) for the full list and which prefix to choose.

### API keys are environment-only

API keys are **no longer** read from config. If your legacy config has `[providers]`, `[openrouter]`, or similar blocks carrying an `api_key`, **delete them** — keys come from environment variables only (for example `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`). `octomind config --api-key` is always rejected; set the environment variable instead. See [doc/usage/04-providers.md](../usage/04-providers.md) for the per-provider variable names.

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

[roles.model]
name = "openrouter:anthropic/claude-sonnet-4"

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

Key changes:
- Roles use `[[roles]]` array format (not `[role_name]` sections); every role is a top-level `[[roles]]` entry with a `name` field
- `enabled` field removed (roles are always available if defined)
- `enable_layers` removed (legacy in-session workflow system is gone — use external `octomind workflow` instead)
- Every role model setting now lives in `[roles.model]`; `max_tokens`, reasoning, sampling, retries, and timeouts are all valid partial overrides
- Tool permissions use `allowed_tools` patterns
- `runtime` builtin server is new — see [Runtime Namespace Move](#runtime-namespace-move) below

The default tag used when no `TAG` is passed to `octomind run` (or `acp`/`server`) is `assistant:concierge` (the `default` field in the root config). It can be a role name (e.g. `developer`) or a tap agent (e.g. `octomind:assistant`).

## Layer Configuration

> The old-format fields below (`builtin`, `enabled`, `enable_tools`) belong to the pre-v1 in-session layer system. If your config still has them, it predates the current schema and needs the manual reshaping shown here.

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
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

The example above is the `reduce` command that ships in the default config (declared as a `[[commands]]` entry — layers and commands share the same fields). The layer's `name` is arbitrary; the `command` is what references an actual role via `octomind acp <role>`.

Key changes:
- `builtin`, `enabled`, `enable_tools`, `model`, `max_tokens` fields removed
- `description` and `command` are required
- Model/system/MCP config lives in the `[[roles]]` entry that `command` references
- Layer is active when referenced by a `/run` command

## In-Session Workflows Removed

The `[[workflows]]` config section and the `/workflow` session command have been removed. Multi-step AI orchestration is now an external CLI: `octomind workflow <file.toml>`.

If you previously had `workflow = "..."` on a role, drop the field. To port an existing in-session workflow, rewrite it as an external workflow TOML — see [doc/usage/09-workflows.md](../usage/09-workflows.md).

## Config File Location

**Current location (the only path the code reads):** `~/.local/share/octomind/config/config.toml` (macOS and Linux; `%LOCALAPPDATA%\octomind\config\config.toml` on Windows).

Override: `OCTOMIND_CONFIG_PATH` environment variable. There are no other legacy fallback paths — if your config lives somewhere else from an older install, move it here or point `OCTOMIND_CONFIG_PATH` at it.

### Splitting a monolithic config

The config directory merges **all** `*.toml` files it contains, not just `config.toml`. Files named `mcp-*.toml` are loaded **last** as overrides. This is handy when migrating a large config: you can split MCP server definitions into a separate `mcp-servers.toml` (loaded after, so it wins on conflicts) instead of keeping everything in one file.

## Automatic Upgrade

```bash
octomind config --upgrade
```

This upgrades the config to the current schema version (`5`) through the registered migration chain and creates a backup before the atomic replacement. Versioned migrations add or transform the fields they explicitly own while preserving user values; unrelated historical layout changes described below may still require manual edits.

## Runtime Namespace Move

The `core` builtin server was split into two: high-level tools stay in `core`, low-level harness-control tools moved to a new `runtime` server.

| Tool | Old server | New server |
|------|------------|------------|
| `plan` | `core` | removed from tool surface; supervisor-internal |
| `tap` *(new)* | -- | **`orchestration`** |
| `mcp` | `core` | **`runtime`** |
| `agent` | `core` | **`runtime`** |
| `skill` | `core` | **`runtime`** |
| `schedule` | `core` | **`orchestration`** |
| `monitor` *(new)* | -- | **`orchestration`** |
| `capability` | `core` | **`runtime`** |

If your config or tap manifest has `server_refs = ["core", ...]` and the role calls any of `mcp`, `agent`, `skill`, `schedule`, or `capability`, add `"runtime"` to the list:

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

Roles that don't call `mcp`/`agent`/`skill`/`schedule`/`capability` (most roles) don't need `"runtime"` at all — drop it from `server_refs` to keep the tool surface tighter.

> **`agent` appears in two places.** The `runtime` server hosts the `agent` **management** tool (shown in the table above). A separate `agent` **builtin** server (a third builtin in the default config, alongside `core` and `runtime`) hosts the dynamically generated `agent_<name>` **execution** tools. If you use dynamic agents, keep both `"runtime"` and `"agent"` in `server_refs`.

## Filesystem Is Now External

`filesystem` is no longer a builtin server. It is provided as an external `octofs` stdio server — **but you do not declare it yourself**. The built-in default tap (`muvon/tap`) and the runtime overlay supply it. Your local `config.toml` only declares three builtin servers (`core`, `runtime`, `agent`); it does **not** contain a `filesystem`/`octofs` entry, and the default roles simply reference it:

```toml
[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
```

> **Do not paste an octofs stdio block.** Because the tap/overlay already provides `filesystem`, hand-rolling a `[[mcp.servers]]` block for it is unnecessary and can cause conflicts. Only add one if you intentionally self-host `octofs`.

If you have a hand-rolled config that declares `filesystem` as `type = "builtin"`, **remove that block** — the tap/overlay supplies the server; just keep `"filesystem"` in your roles' `server_refs`. This is a manual edit: `octomind config --upgrade` will **not** do it for you.

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
