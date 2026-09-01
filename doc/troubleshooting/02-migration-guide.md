# Migration Guide

Migrate legacy Octomind configurations to schema version `12` with the automatic upgrade chain and the documented manual changes.

## Model Profile and Provider Format

**Old format:**
```toml
model = "legacy-model-without-provider"
```

**Current format:**
```toml
[model]
name = "openrouter:anthropic/claude-sonnet-4-6"
reasoning_effort = "medium"
max_tokens = 32768
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

All model names require `provider:model` format. There are exactly three model purposes: main, supervisor, and compression. `[model]` is the required complete main-purpose baseline; `[roles.model]` is a partial override of that same main purpose. Optional `[supervisor.model]` and `[compression.model]` profiles own the other two purposes and inherit omitted fields from `[model]`. Tap and workflow mappings remain name-only.

The shipped main, supervisor, and compression profiles use `octohub:auto`, authenticated through `octomind login`. Migration does not require replacing an intentional model choice with that default: pick the configured prefix that matches where the model is served — e.g. `openai:gpt-5.6-sol`, `anthropic:claude-sonnet-4-6`, `openrouter:moonshotai/kimi-k3`, or `ollama:glm-5.3` — and see [Providers](../usage/04-providers.md) for the full prefix and credential table.

### API keys are environment-only

API keys are **no longer** read from config. If your legacy config has `[providers]`, `[openrouter]`, or similar blocks carrying an `api_key`, remove them. `octomind login` stores the OctoHub credential as `OCTOHUB_API_KEY`; alternate providers read their own environment variables. `octomind config --api-key` reports that config-file keys are unsupported and makes no change.

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
model = "legacy-model-without-provider"
enable_layers = true

[developer.mcp]
enabled = true
server_refs = ["core"]
```

**Current format:**
```toml
[[roles]]
name = "developer"
system = "You are the project developer. Work in {{CWD}}."
welcome = ""

[roles.model]
name = "openrouter:anthropic/claude-sonnet-4-6"

[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
```

Key changes:
- Roles use `[[roles]]` array format (not `[role_name]` sections); every role is a top-level `[[roles]]` entry with a `name` field
- `enabled` field removed (roles are always available if defined)
- `enable_layers` removed (legacy in-session workflow system is gone — use external `octomind workflow` instead)
- Every role model setting now lives in `[roles.model]`; `max_tokens`, reasoning, sampling, retries, and timeouts are all valid partial overrides
- Tool permissions use `allowed_tools` patterns
- `runtime` builtin server is new — see [Runtime Namespace Move](#runtime-namespace-move) below

The default tag used when no `TAG` is passed to `octomind run` (or `acp`/`server`) is `assistant:concierge` (the `default` field in the root config). It can be a role name (e.g. `developer`) or a tap agent (e.g. `octomind:assistant`).

## Command and Layer Configuration

> The old-format fields below (`builtin`, `enabled`, `enable_tools`) belong to the pre-v1 in-session layer system. If your config still has them, it predates the current schema and needs the manual reshaping shown here.

**Old format:**
```toml
[[layers]]
name = "task_refiner"
builtin = true
enabled = true
enable_tools = true
```

**Current `/run` command format:**
```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

The example above is the `reduce` command in the default config. `[[layers]]` and `[[commands]]` deserialize to the same `LayerConfig` fields, but `/run <name>` looks up `[[commands]]`; put user-invoked command layers there. The `command` selects the ACP process and role.

Key changes:
- `builtin`, `enabled`, `enable_tools`, `model`, `max_tokens` fields removed
- `description` and `command` are required
- Model/system/MCP config lives in the `[[roles]]` entry that `command` references
- `/run <name>` executes the matching `[[commands]]` entry

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

This upgrades the config to the current schema version (`12`) through the registered migration chain and creates a backup before the atomic replacement. Versioned migrations add, transform, or remove the fields they explicitly own while preserving unrelated user values; historical structural changes outside that chain may still require manual edits.

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

If your config or tap manifest has only `server_refs = ["core", ...]`, add `runtime` for `mcp`, `agent`, `skill`, or `capability`, and add `orchestration` for `tap`, `schedule`, or `monitor`:

```diff
 [roles.mcp]
-server_refs = ["core", "filesystem", "agent"]
-allowed_tools = ["core:*", "filesystem:*", "agent:*"]
+server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
+allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
```

The `runtime` and `orchestration` servers are registered in the default config:

```toml
[[mcp.servers]]
name = "runtime"
type = "builtin"
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "orchestration"
type = "builtin"
timeout_seconds = 30
tools = []
```

If you have a hand-rolled config without either server, add the corresponding block.

Roles that do not use runtime-management tools do not need `runtime`; roles that do not use tap/schedule/monitor do not need `orchestration`.

> **`agent` appears in two places.** The `runtime` server hosts the `agent` management tool. A separate `agent` builtin server (one of the four defaults) hosts generated `agent_<name>` execution tools. Dynamic-agent roles need both `runtime` and `agent`.

## Filesystem Is Now External

`filesystem` is no longer a builtin server. Resolved tap configuration can provide it as an external `octofs` stdio server. The default config declares four builtin servers (`core`, `orchestration`, `runtime`, `agent`) and no `filesystem` entry; tap roles may reference and supply it:

```toml
[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
```

> Do not add a second `filesystem` server when the selected tap agent already supplies one. Add an explicit stdio block only when you intentionally own that server configuration.

If you have a hand-rolled config that declares `filesystem` as `type = "builtin"`, **remove that block** and let the selected tap agent supply the external server, or replace it with an intentional stdio configuration. This is a manual edit: `octomind config --upgrade` does not perform it.

## MCP Server Type

**Old (incorrect in some docs):** `type = "stdin"`

**Correct:** `type = "stdio"`

The server type for local process-based MCP servers is `"stdio"`, not `"stdin"`.

## Session Commands

**Removed:**
- `/save` — Session persistence is automatic on exit

**Added:**
- `/skill` — Manage skills (list, use, forget)

Use `/help` to see current command list.
