# Configuration Reference

Complete field-by-field reference for `~/.local/share/octomind/config/config.toml`.

All values shown match `config-templates/default.toml`. Fields marked **(required)** have no fallback default.

## Root-Level Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `version` | u32 | `1` | Config version. Do not modify. Used for automatic upgrades. |
| `log_level` | string | `"info"` | Logging verbosity: `"none"`, `"info"`, `"debug"` |
| `model` | string | `"openrouter:anthropic/claude-sonnet-4"` | Default model in `provider:model` format |
| `default` | string | `"assistant:concierge"` | Default tag when no TAG passed to `octomind run` |
| `max_tokens` | u32 | `16384` | Global max tokens for all operations |
| `custom_instructions_file_name` | string | `"INSTRUCTIONS.md"` | File auto-loaded as user message in new sessions. Empty string to disable. |
| `custom_constraints_file_name` | string | `"CONSTRAINTS.md"` | File appended to each request in `<constraints>` tags. Empty string to disable. |
| `sandbox` | bool | `false` | Restrict filesystem writes to working directory. Also available as `--sandbox` CLI flag. |
| `auto_capabilities` | bool | `true` | Enable automatic capability activation on user messages. Disable to require manual `capability(action="enable")` calls. |

## Performance & Limits

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mcp_response_tokens_threshold` | u32 | `20000` | Hard limit on MCP response tokens. Responses truncated when exceeded. `0` = unlimited. |
| `max_session_tokens_threshold` | u32 | `200000` | Max tokens per session before truncation. `0` = disabled. |
| `max_retries` | u32 | `1` | Retry attempts for API calls. |
| `retry_timeout` | u32 | `30` | Base timeout in seconds for exponential backoff. |
| `request_timeout_seconds` | u32 | `300` | Per-request HTTP timeout in seconds. Hard limit on LLM provider API calls. `0` = no timeout. |
| `reasoning_effort` | enum | `"medium"` | Thinking model effort: `"low"`, `"medium"`, `"high"`, `"xhigh"`, `"max"`. Non-thinking models ignore it. |
| `cache_keepalive_enabled` | bool | `false` | Keep prompt cache warm with periodic pings while session idles. Provider-aware (only pings providers that support refresh-on-read). |
| `cache_keepalive_max_idle_seconds` | u64 | `1800` | Stop pinging this many seconds after last user activity. `0` = ping until session ends. |
## User Interface

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_markdown_rendering` | bool | `true` | Pretty-print AI responses with markdown rendering. |
| `markdown_theme` | string | `"default"` | Theme: `"default"`, `"dark"`, `"light"`, `"ocean"`, `"solarized"`, `"monokai"` |
| `max_session_spending_threshold` | f64 | `0.0` | USD limit per session. Prompts before continuing when exceeded. `0.0` = no limit. |
| `max_request_spending_threshold` | f64 | `0.0` | USD limit per request. Stops execution when exceeded. `0.0` = no limit. |

## `[capabilities]`

Map of capability name to provider override. Used by tap agents to route specific capabilities to different providers.

```toml
[capabilities]
codesearch = "octocode"  # uses capabilities/codesearch/octocode.toml
```

Empty by default. Each key maps to a provider TOML file within the tap's `capabilities/` directory.

## `[taps]`

Map of tap agent tag to model override. Set a preferred model for specific tap agents.

```toml
[taps]
"developer:general" = "ollama:glm-5"
"octomind:assistant" = "openai:gpt-4o"
```

**Priority:** CLI `--model` > taps override > role.model > config.model
When you run `octomind run developer:general`, the model is resolved in this order:
1. `--model` CLI flag (if provided)
2. `[taps]` override for `"developer:general"` (if configured)
3. Global `model` in config

Empty by default. Only applies to tap agents (tags with `:`). Plain role names use role.model or config.model.

## `[[roles]]`

Define custom roles that override or extend tap-provided agents.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Role identifier (e.g., `"developer"`, `"assistant"`) |
| `model` | string | no | Model override for this role (`provider:model` format) |
| `system` | string | no | System prompt. Supports template variables. |
| `welcome` | string | no | Welcome message shown on session start. Supports template variables. |
| `temperature` | f64 | no | Sampling temperature (0.0-2.0) |
| `top_p` | f64 | no | Nucleus sampling (0.0-1.0) |
| `top_k` | u32 | no | Top-k token limit (1-1000) |

### `[roles.mcp]`

MCP configuration for the role.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server_refs` | string[] | `[]` | MCP server names to enable for this role |
| `allowed_tools` | string[] | `[]` | Tool access patterns. Empty = all tools. Supports wildcards: `"core:*"`, `"filesystem:view"` |

```toml
[[roles]]
name = "assistant"
temperature = 0.3
top_p = 0.7
top_k = 20
system = """
You are helpful and knowledgeable assistant.
Working directory: {{CWD}}
"""
welcome = "Hello! Ready to help. Working in {{CWD}} (Role: {{ROLE}})"

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

## `[mcp]`

Global MCP (Model Context Protocol) configuration.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_tools` | string[] | `[]` | Global tool restrictions. Empty = no restrictions. Fallback when role doesn't specify. |

### `[[mcp.servers]]`

MCP server definitions. Three types supported: `builtin`, `http`, `stdio`.

**Builtin servers** (always available, no external process):

| `core` | `plan`, `tap` | High-level planning and agent management |
| `runtime` | `mcp`, `agent`, `skill`, `schedule`, `capability` | Harness reconfiguration and scheduling |
| `agent` | `agent_<name>` per `[[agents]]` entry | ACP sub-agent dispatch |

`filesystem` is no longer a builtin — it's an external `stdio` server backed by `octofs`. See [MCP Tools](../usage/07-mcp-tools.md) for the tool surface.

#### Common Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique server identifier |
| `type` | string | yes | `"builtin"`, `"http"`, or `"stdio"` |
| `timeout_seconds` | u32 | no | Response timeout (default: 30) |
| `tools` | string[] | no | Tool filter. Empty = all tools. Supports wildcards: `"github_*"` |
| `auto_bind` | string[] | no | Role names to auto-include this server for |

#### HTTP-Specific Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | yes | Server endpoint URL |

> **OAuth Authentication:** HTTP servers requiring authentication are handled automatically via MCP Authorization Discovery (RFC 9728). No manual configuration needed — just provide the URL and Octomind will discover OAuth endpoints, register via CIMD/DCR, and authenticate using PKCE.

#### Stdio-Specific Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `command` | string | yes | Executable to run |
| `args` | string[] | no | Command arguments |


## `[[hooks]]`

Webhook HTTP listeners that pipe payloads through scripts and inject output into sessions.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique hook identifier |
| `bind` | string | required | HTTP server address (e.g., `"0.0.0.0:9876"`) |
| `script` | string | required | Path to executable script |
| `timeout` | u32 | `30` | Script timeout in seconds (1-3600) |

```toml
[[hooks]]
name = "github-push"
bind = "0.0.0.0:9876"
script = "/path/to/process-github-push.sh"
timeout = 30
```


## `[[layers]]`

Reusable ACP-invocable units used by `[[commands]]`. Layers delegate to roles via the ACP protocol — the actual model, system prompt, and MCP configuration live in `[[roles]]`, not here.

> **Multi-step AI workflows** are no longer defined in this config. Use the external CLI: `octomind workflow <file.toml>` — see [doc/usage/09-workflows.md](../usage/09-workflows.md).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Layer identifier |
| `description` | string | required | Human-readable description (used in help, MCP) |
| `command` | string | required | ACP command to execute: `"octomind acp <role_name>"` |
| `workdir` | string | `"."` | Working directory (relative to session workdir) |
| `input_mode` | string | no | How input is fed: `"last"`, `"all"`, `"summary"` |
| `output_mode` | string | no | How output affects session: `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | no | Role for output messages: `"assistant"`, `"user"` |

```toml
[[layers]]
name = "task_refiner"
description = "Refines and clarifies user requests for better processing by subsequent layers"
command = "octomind acp task_refiner"
input_mode = "last"
output_mode = "none"
output_role = "assistant"
```

## `[[commands]]`

Custom session commands triggered with `/run <name>`. Uses the same schema as `[[layers]]`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Command identifier (used as `/run <name>`) |
| `description` | string | required | Human-readable description (shown in help text) |
| `command` | string | required | ACP command to execute: `"octomind acp <role_name>"` |
| `workdir` | string | `"."` | Working directory (relative to session workdir) |
| `input_mode` | string | no | How input is fed: `"last"`, `"all"`, `"summary"` |
| `output_mode` | string | no | How output affects session: `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | no | Role for output messages: `"assistant"`, `"user"` |

```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

## `[[agents]]`

Specialized AI agents using ACP protocol. Each becomes an MCP tool (`agent_<name>`).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Agent identifier. Tool becomes `agent_<name>`. |
| `description` | string | required | MCP tool description shown to the AI |
| `command` | string | required | Shell command starting an ACP server over stdio |
| `workdir` | string | `"."` | Working directory for subprocess |

```toml
[[agents]]
name = "context_gatherer"
description = "Gather detailed context from files and codebase."
command = "octomind acp context_gatherer"
workdir = "."
```

## `[[prompts]]`

Reusable prompt templates accessible via `/prompt <name>`.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Prompt identifier |
| `description` | string | yes | Shown in `/prompt` list |
| `prompt` | string | yes | Prompt text injected into session |

```toml
[[prompts]]
name = "review"
description = "Request code review with focus on best practices"
prompt = """Please review the code above focusing on:
- Code quality and best practices
- Security considerations
- Performance implications"""
```

## `[skills]`

Automatic skill activation and validation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_activation` | bool | `true` | Enable declarative rule-based activation (checks on every user message) |
| `auto_validation` | bool | `false` | Enable validate script execution at end of assistant turns |
| `activation_timeout` | u64 | `3` | Reserved. Rules evaluate in-process (no timeout needed) |
| `validation_timeout` | u64 | `60` | Seconds per validate script. `0` = unlimited |
| `max_retries` | u32 | `3` | Max validation retries per skill before giving up |

```toml
[skills]
auto_activation = true
auto_validation = false
activation_timeout = 3
validation_timeout = 60
max_retries = 3
```

## `[compression]`

Automatic context compression system.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hints_enabled` | bool | `true` | Enable compression system |
| `hints_pressure_threshold` | f64 | `0.7` | Context pressure threshold (0.0-1.0) to start showing hints |
| `hints_min_interval` | u32 | `5` | Minimum tool executions between hints |
| `knowledge_retention` | u32 | `10` | Max critical knowledge entries retained across compressions |

### `[[compression.pressure_levels]]`

| Field | Type | Description |
| `threshold` | u64 | Token count threshold to trigger compression |
| `target_ratio` | f64 | Compression strength (2.0 = 50% reduction, 4.0 = 75%, 8.0 = 87.5%) |

Default pressure levels:

| Threshold | Target Ratio | Effect |
|-----------|-------------|--------|
| `60000` | `2.0` | Light: 50% reduction |
| `120000` | `4.0` | Medium: 75% reduction |
| `160000` | `8.0` | Aggressive: 87.5% reduction |
### `[compression.decision]`

Model used for compression decisions and summary generation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `"openai:gpt-5-mini"` | Fast, cheap model recommended |
| `max_tokens` | u32 | `16000` | Max tokens for decision + summary |
| `temperature` | f64 | `0.3` | Lower = more consistent decisions |
| `top_p` | f64 | `1.0` | Nucleus sampling |
| `top_k` | u32 | `0` | Top-k (0 = disabled) |
| `max_retries` | u32 | `1` | Retry attempts |
| `retry_timeout` | u32 | `30` | Retry backoff base |
| `ignore_cost` | bool | `false` | When true, compression cost is not tracked |

```toml
[compression]
hints_enabled = true
hints_pressure_threshold = 0.7
hints_min_interval = 5
knowledge_retention = 10

[[compression.pressure_levels]]
threshold = 60000
target_ratio = 2.0

[[compression.pressure_levels]]
threshold = 120000
target_ratio = 4.0

[[compression.pressure_levels]]
threshold = 160000
target_ratio = 8.0

[compression.decision]
model = "openai:gpt-5-mini"
max_tokens = 16000
temperature = 0.3
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
ignore_cost = false
```

## `[learning]`

Cross-session adaptive learning. Extracts lessons from sessions and injects them into future sessions. See [Learning Guide](../usage/13-learning.md) for full details.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the learning system |
| `model` | string | `"anthropic:claude-haiku-4-5"` | Model for extraction and retrieval LLM calls |
| `backend` | string | `"file"` | Backend: `"file"` or `"mcp"` |
| `min_messages_for_intermediate` | u32 | `3` | Min user messages before intermediate learning triggers |
| `max_inject` | u32 | `5` | Max lessons injected into system prompt |

### `[learning.store]` (MCP backend only)

| Field | Type | Description |
|-------|------|-------------|
| `tool` | string | MCP tool name for storing lessons (e.g. `"memorize"`) |
| `field_map` | table | Maps canonical fields to MCP argument names. Empty string = omit. |

### `[learning.retrieve]` (MCP backend only)

| Field | Type | Description |
|-------|------|-------------|
| `tool` | string | MCP tool name for retrieving lessons (e.g. `"remember"`) |
| `field_map` | table | Maps canonical fields to MCP argument names. Empty string = omit. |

```toml
[learning]
enabled = true
model = "anthropic:claude-haiku-4-5"
backend = "file"
min_messages_for_intermediate = 3
max_inject = 5
```

## Guardrails (`.agents/guardrails.toml`)

Project-level guardrails are configured in `.agents/guardrails.toml` in the working directory, not in the main config file. See [Guardrails](../usage/18-guardrails.md) for full documentation.

### `[[pipe]]` — Pre-Model Input Transform

Preprocesses user input through an external script before the model sees it. At most one `[[pipe]]` may match per message.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Pipe identifier (used in errors and `PIPE_NAME` env var) |
| `command` | string | yes | — | Script path (relative to workdir or absolute) |
| `when` | string | no | `"any"` | `"first"` = first message only; `"any"` = every message |
| `match` | string | no | — | Regex on user message text. Empty = match all. |
| `roles` | string[] | no | — | Restrict to roles (exact or domain-prefix match). Empty = all roles. |

```toml
# .agents/guardrails.toml
[[pipe]]
name = "prepare"
command = "./prepare.sh"
when = "first"
match = "^/deploy"
roles = ["developer:general"]
```

Environment variables set when spawning: `OCTOMIND_ROLE`, `OCTOMIND_WORKDIR`, `PIPE_NAME`, `PIPE_RUN_COUNT`, `SESSION_MESSAGE_COUNT`. Timeout: 300 seconds.

## Multi-File Configuration

Octomind supports split-file configuration. All `*.toml` files in the config directory are merged:

1. `config.toml` is loaded first
2. Other `*.toml` files are loaded alphabetically
3. Arrays of tables (e.g., `[[mcp.servers]]`) are concatenated
4. Same-name entries are deduplicated (last wins)
5. Scalar values are overridden by later files

This allows organizing config by concern (e.g., `mcp-github.toml`, `layers-custom.toml`).

**Special Case: `mcp-*.toml` Override Files**

Files matching the pattern `mcp-*.toml` are loaded **AFTER** all other `*.toml` files, regardless of their alphabetical position. This ensures they can reliably override same-named MCP servers defined in earlier files like `mcp.toml`.

Without this special handling, `mcp.toml` would lexicographically sort after `mcp-github.toml` and silently overwrite any server overrides.

This mechanism is used by the `mcp persist` command, which writes to `<config_dir>/mcp-<name>.toml` with `auto_bind = ["<role>"]`. These persisted servers are automatically available on the next startup without manual `server_refs` edits.

## Template Variables

Available in `system` and `welcome` fields:

| Variable | Description |
|----------|-------------|
| `{{CWD}}` | Current working directory |
| `{{ROLE}}` | Active role name |
| `{{DATE}}` | Current date |
| `{{SHELL}}` | User's shell |
| `{{OS}}` | Operating system |
| `{{BINARIES}}` | Available binary tools |
| `{{GIT_STATUS}}` | Git repository status |
| `{{GIT_TREE}}` | Project file tree |
| `{{README}}` | Contents of README.md in project root |
| `{{HOME}}` | User's home directory path |
| `{{CONTEXT}}` | Session context (for layers) |
| `{{SYSTEM}}` | Parent system prompt (for layers) |
