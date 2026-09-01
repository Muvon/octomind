# Configuration Reference

Field reference for Octomind's versioned TOML configuration, including defaults, required sections, and the three model purposes.

## Root-Level Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `version` | u32 | `12` | Config version. Do not modify. Used for automatic upgrades. |
| `log_level` | string | `"info"` | Logging verbosity: `"none"`, `"info"`, `"debug"` |
| `default` | string | `"assistant:concierge"` | Default tag for bare `run`, `acp`, and `server`. See note below. |
| `sandbox` | bool | `false` | Restrict writes for `run`, `acp`, and `server`; those commands also accept `--sandbox`. |
| `telemetry` | bool | `true` | Anonymous usage telemetry. Overridden per-run by `OCTOMIND_TELEMETRY`, and by `DO_NOT_TRACK=1` before either. See [Telemetry](04-environment-variables.md#telemetry) for the exact field list. |
| `auto_capabilities` | bool | `true` | Enable automatic capability activation on user messages. Disable to require manual `capability(action="enable")` calls. |
| `system` | string (optional) | _none_ | Legacy serialized field retained for config compatibility. Session prompts come from required `[[roles]].system`; do not use this as a role-prompt override. |

> **About the `default` value:** `"assistant:concierge"` is a **tap agent** addressed as `category:variant`, shipped by the built-in default tap `muvon/tap` (which resolves to the GitHub repo `github.com/muvon/octomind-tap`) — *not* a role defined in this config file. If you search this file for a `concierge` role you will not find one. A bare tag without a colon (e.g. `"developer"`) resolves against your local `[[roles]]`; a `category:variant` tag resolves against installed taps.

## `[model]`

The complete main model profile and inheritance baseline. Persistent role, supervisor, and compression profiles use the same fields; name-only tap/workflow overrides retain the inherited parameters.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"octohub:auto"` | Provider-qualified model identifier |
| `reasoning_effort` | enum | `"medium"` | `"low"`, `"medium"`, `"high"`, `"xhigh"`, or `"max"` |
| `max_tokens` | u32 | `32768` | Maximum output tokens; `0` uses provider behavior |
| `temperature` | f32 | `0.3` | Sampling temperature, 0.0-2.0 |
| `top_p` | f32 | `0.7` | Nucleus sampling, 0.0-1.0 |
| `top_k` | u32 | `20` | Top-k limit, 0-1000; `0` disables it |
| `max_retries` | u32 | `1` | Provider retry attempts |
| `retry_timeout` | u64 | `30` | Exponential-backoff base in seconds |
| `request_timeout_seconds` | u64 | `300` | Hard timeout for one provider request; `0` is unlimited |

## Performance & Limits

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mcp_response_tokens_threshold` | usize | `20000` | Hard limit on MCP response tokens. Responses truncated when exceeded. `0` = unlimited. |
| `max_session_tokens_threshold` | usize | `200000` | Max tokens per session before truncation. Also acts as the **hard compression ceiling** and the denominator for context-pressure hints (see `[compression]`). `0` = disabled. Validation fails if `> 2,000,000`. |
| `cache_keepalive_enabled` | bool | `false` | Keep prompt cache warm with periodic pings while the session idles. Provider-aware: currently **only Anthropic** is pinged, and the ping interval comes from the provider's cache TTL (1h), not from this config. |
| `cache_keepalive_max_idle_seconds` | u64 | `1800` | Stop pinging this many seconds after last user activity. `0` = ping until session ends. Validation fails if `> 86400` (24h). |
## User Interface

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_markdown_rendering` | bool | `true` | Pretty-print AI responses with markdown rendering. |
| `markdown_theme` | string | `"default"` | Theme: `"default"`, `"dark"`, `"light"`, `"ocean"`, `"solarized"`, `"monokai"` |
| `max_session_spending_threshold` | f64 | `0.0` | Session spending limit. Prompts before continuing when exceeded. `0.0` = no limit. |
| `max_request_spending_threshold` | f64 | `0.0` | Request spending limit. Stops execution when exceeded. `0.0` = no limit. |

## `[capabilities]`

Map of capability name to provider override. Used by tap agents to route specific capabilities to different providers.

```toml
[capabilities]
codesearch = "octocode"  # uses capabilities/codesearch/octocode.toml
```

Empty by default. Each key maps to a provider TOML file within the tap's `capabilities/` directory.

## `[taps]`

Strict map of tap agent tag to model name. It changes only `name`; all other parameters come from the main profile before any independent role override.

```toml
[taps]
"developer:general" = "ollama:glm-5.3"
"assistant:concierge" = "openai:gpt-5.6-luna"
```

**Priority (highest wins):** explicit runtime override > the active role's `[roles.model]` > the tap name mapping > `[model]`.
1. `--model` CLI flag (if provided)
2. The `model` the agent's role/manifest declares (for `developer:general`, the manifest's role model)
3. Main `[model]` profile — its `name` is replaced by the matching tap mapping when present

`[taps]` only applies to tap agents (tags with `:`). Plain roles resolve `[roles.model]` directly against `[model]`.

## `[[roles]]`

Define custom roles that override or extend tap-provided agents.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Role identifier (e.g., `"developer"`, `"assistant"`) |
| `system` | string | yes | System prompt. Supports template variables. |
| `welcome` | string | yes | Welcome message shown on session start. Supports template variables; use `""` for no banner. |

### `[roles.model]`

Optional partial model profile for the role. It accepts every field from `[model]`; unspecified fields inherit from main.

### `[roles.mcp]`

MCP configuration for the role.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server_refs` | string[] | `[]` | MCP server names to enable for this role |
| `allowed_tools` | string[] | `[]` | Tool access patterns. Empty = all tools. Supports wildcards: `"core:*"`, `"filesystem:view"` |

```toml
[[roles]]
name = "assistant"
system = """
You are helpful and knowledgeable assistant.
Working directory: {{CWD}}
"""
welcome = "Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})"

[roles.model]
name = "openai:gpt-5.6-sol"
reasoning_effort = "high"

[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
```

## `[mcp]`

Global MCP (Model Context Protocol) configuration.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_tools` | string[] | `[]` | Global tool restrictions. Empty = no restrictions. Fallback when role doesn't specify. |

### `[[mcp.servers]]`

MCP server definitions. Three types supported: `builtin`, `http`, `stdio`.

**Builtin servers** (always available, no external process):

| Server | Tools | Description |
|--------|-------|-------------|
| `core` | `recall` (when attention or governance is enabled) | Session-memory retrieval; governance defaults on and planning is supervisor-internal |
| `orchestration` | `tap`, `schedule`, `monitor` | Delegation, scheduled messages, and event-stream monitoring |
| `runtime` | `mcp`, `agent`, `skill`, `capability` | Harness and tool-surface reconfiguration |
| `agent` | `agent_<name>` per `[[agents]]` entry | ACP sub-agent dispatch |

> **`filesystem` is not declared here.** It is an external `stdio` server backed by octofs and provided through tap capabilities. The octofs server provides six tools: `view`, `workdir`, `text_editor`, `batch_edit`, `extract_lines`, and `shell`. `/mcp full` shows the installed server's authoritative schemas.

#### Common Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique server identifier |
| `type` | string | yes | `"builtin"`, `"http"`, or `"stdio"` |
| `timeout_seconds` | u64 | yes | Per-operation timeout; tool-call progress resets this idle deadline (template: 30) |
| `tools` | string[] | yes | Tool filter. Empty = all tools. Supports wildcards such as `"github_*"`. |
| `auto_bind` | string[] | no | Role names to auto-include this server for |

#### HTTP-Specific Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | yes | Server endpoint URL |
| `headers` | map | no | Headers sent on every request. Values support `{{ENV:KEY}}` placeholders. A configured `Authorization` header disables OAuth discovery. |

> **Authentication:** Configure a static `Authorization` header for bearer tokens or API keys. Without one, Octomind uses MCP Authorization Discovery (RFC 9728), registers via CIMD/DCR, and authenticates using PKCE.

#### Stdio-Specific Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `command` | string | yes | Executable to run |
| `args` | string[] | yes | Command arguments; use `[]` when none |
| `env` | map | no | Child environment entries; values support `{{ENV:KEY}}` placeholders |
| `cwd` | string | no | Child working directory; omitted inherits Octomind's working directory (plugins may set their root) |


## `[[hooks]]`

Webhook HTTP listeners that pipe payloads through scripts and inject output into sessions.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Unique hook identifier |
| `bind` | string | required | HTTP server address (e.g., `"0.0.0.0:9876"`) |
| `script` | string | required | Path to executable script |
| `timeout` | u64 | `30` | Script timeout in seconds (1-3600) |

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
| `workdir` | string | `"."` | Working directory (relative to session workdir). The only optional field. |
| `input_mode` | string | **required** | How input is fed: `"last"`, `"all"`, `"summary"` |
| `output_mode` | string | **required** | How output affects session: `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | **required** | Role for output messages: `"assistant"`, `"user"` |

> `input_mode`, `output_mode`, and `output_role` have **no default** — config loading fails if any is omitted. Only `workdir` is optional.

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

Custom session commands triggered with `/run <name>`. **Uses the exact same schema as `[[layers]]`** (same `LayerConfig` struct) — see the field table above, including the required `input_mode` / `output_mode` / `output_role` fields. The only difference is invocation: `[[commands]]` entries are run manually from a session via `/run <name>`, while `[[layers]]` are orchestration units invoked over ACP. For `[[commands]]`, `name` is the token you type after `/run`.

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
| `description` | string | no | Optional text shown in `/prompt` list |
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

> **`auto_validation` scope:** this flag gates only the `validate` scripts declared inside `SKILL.md` files. It does **not** gate the separate guardrail `[[validator]]` system in `.agents/guardrails.toml` — those end-of-turn validators run unconditionally regardless of this setting.

## `[compression]`

Automatic context compression system.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `knowledge_retention` | usize | `25` | Max critical knowledge entries retained across compressions |
| `analysis_findings_max_tokens` | usize | `6000` | Hard token budget for retained analysis findings; `0` disables retention |
| `threshold` | usize | `70000` | Single compression trigger in absolute tokens; `0` disables compression |

> **Depth is computed, not configured.** Once context exceeds `threshold`, how deep each compression goes is derived per cycle from the measured session growth rate and the context ceiling — the lower of `max_session_tokens_threshold` (see Performance & Limits) and the session model's usable window. The derived ratio always lands in [2.0, 16.0].

### `[compression.attention]`

Optional PACT-style provenance and archive governance around compression.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable provenance-labelled causal evidence selection and rendering |
| `validator` | bool | `true` | Reject optional compactions whose folded units have invalid attribution |
| `telemetry` | bool | `true` | Persist a content-free compression decision record beside the lossless archive |

`[compression.attention.governance]` defaults to `enabled = true` and `verify_hash = true`, preserving runtime-owned pins/frontier and checking governance hashes before a compaction is committed.

Keep scalar `[compression]` keys before nested `[compression.attention]` and `[compression.model]` headers; TOML assigns later scalars to the most recent nested table.

### `[compression.model]`

Model used for compression decisions and summary generation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"octohub:auto"` | Compression model name |
| `reasoning_effort` | enum | `"medium"` | Thinking effort override |
| `max_tokens` | u32 | `16000` | Max tokens for decision + summary |
| `temperature` | f64 | `0.3` | Lower = more consistent decisions |
| `top_p` | f64 | `1.0` | Nucleus sampling |
| `top_k` | u32 | `0` | Top-k (0 = disabled) |
| `max_retries` | u32 | `1` | Retry attempts |
| `retry_timeout` | u64 | `30` | Retry backoff base (seconds) |
| `request_timeout_seconds` | u64 | `300` | Hard timeout for one request; `0` is unlimited |

```toml
[compression]
knowledge_retention = 25
analysis_findings_max_tokens = 6000
threshold = 70000

[compression.model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 16000
temperature = 0.3
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

## `[supervisor]`

The out-of-band control plane around the agent loop. It hosts learning (distill + recall + orientation memory), deterministic detectors, the verify-gate, the external plan manager, and condense. See the [Supervisor guide](../usage/14-supervisor.md) for how the mechanics fit together. **Strict:** the `[supervisor]` section and its required keys must be present — a missing section or key is a hard parse error, not a silent default.

Deterministic detectors (loop / no-progress / failed-check recovery), goal recitation, and the free check-after-mutation pre-gate are always on with fixed thresholds — they are behavior, not knobs.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for the whole control plane |
### `[supervisor.model]`

Optional partial profile shared by every supervisor mechanic: gate, resolve, plan, condense, extraction, recall, retention, verification, and evolution. It accepts every field from `[model]`; omitted fields inherit main. Omitting the entire block uses `[model]` unchanged.

### `[supervisor.learning]`

Cross-session adaptive learning. Extracts lessons and orientation memory (durable subject understanding, recalled as working assumptions to verify) from sessions and injects them into future sessions. See [Learning Guide](../usage/13-learning.md) for full details.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the learning system (lessons + orientation) |

### `[supervisor.learning.evolution]`

Optional grounded behavior evolution. When enabled, newly stored quote-backed
rules and verified experiences may produce scoped native skill or guardrail
candidates. Synthesis and admission both use the single `[supervisor.model]`
profile, which must support structured output.
Thresholds and trial limits are fixed internal constants.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable detached candidate synthesis and lifecycle-managed trials |

### `[supervisor.gate]`

Verify-gate on self-reported completion. Free deterministic pre-gates run first (no model call); the LLM checklist runs only if those pass.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the verify-gate |

### `[supervisor.plan]`

Adaptive external plan manager. The specialist has no plan mutation tool; a sparse hidden signal emitted alongside real work wakes this manager only when planning or a transition is needed.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable adaptive external planning |

### `[supervisor.condense]`

Task-aware narrowing of oversized plain-text tool outputs. A result whose own output exceeds `tokens_threshold` becomes a candidate; smaller results in the same round are passed through untouched and never shown to the condenser. One shared supervisor-model call per round selects, by original line ranges over a bounded query/diagnostic-aware view, what the current task needs; kept lines are reconstructed verbatim, and irrelevant results get deterministic notices rather than model-authored summaries. Full originals are spilled to session files first when the active role can read them back. The `mcp_response_tokens_threshold` prefix-cut is applied **before** condensation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable condensation |
| `adaptive` | bool | `false` | Adapt a process-local multiplier from realized savings, bounded to `0.5x`–`2.0x` of the configured baseline |
| `tokens_threshold` | usize | `5000` | Per-result trigger (estimated tokens of that single result); `0` = off. Keep well below `mcp_response_tokens_threshold` |

```toml
[supervisor]
enabled = true

[supervisor.model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 8192
temperature = 0.0
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300

[supervisor.learning]
enabled = true

[supervisor.learning.evolution]
enabled = false

[supervisor.gate]
enabled = true

[supervisor.plan]
enabled = true

[supervisor.condense]
enabled = true
adaptive = false
tokens_threshold = 5000
```

## `[registry]`

Controls caching of agent manifests fetched from taps. Registry sources themselves are managed with `octomind tap <user/repo> [path]` and `octomind untap <user/repo>`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cache_ttl_hours` | u64 | `24` | How long a fetched tap manifest is cached before re-checking. |

Fetched manifests are cached at `<data>/agents/<category>/<variant>.toml`. Within the TTL the cached manifest is served immediately; once stale, the cached copy is still served (stale-serve) while a background refresh fetches the latest version. See [Tap System](../integration/04-tap-system.md) for the registry behavior.

```toml
[registry]
cache_ttl_hours = 24
```

## Guardrails (`.agents/guardrails.toml`)

Project-level guardrails are configured in `.agents/guardrails.toml` in the working directory, **not** in the main config file. That file holds four distinct mechanisms — `[[guard]]`, `[[hook]]`, `[[validator]]`, and `[[pipe]]`. Only `[[pipe]]` is detailed below; see [Guardrails](../usage/18-guardrails.md) for the full reference.

> **Do not confuse** the guardrail `[[hook]]` (a post-result script in `.agents/guardrails.toml`) with the top-level `[[hooks]]` config above (webhook HTTP listeners for daemon mode). They are entirely separate concepts.

| Table | Purpose |
|-------|---------|
| `[[guard]]` | Pre-call deny rule — blocks a tool call before it runs. |
| `[[hook]]` | Post-result script run after a tool call (`on = "success"`, `"error"`, or `"any"`). |
| `[[validator]]` | End-of-turn script run on the new call-log slice (cursor-based), with optional role filter. Runs regardless of `[skills].auto_validation`. |
| `[[pipe]]` | Pre-model input transform (detailed below). |

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

These variables are substituted in role `system` and `welcome` fields at prompt-expansion time:

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
| `{{CONTEXT}}` | Project context bundle (README, Git status, tracked tree) |
| `{{SYSTEM}}` | Current system information (shell, OS, working directory, binaries) |

> **`{{HOME}}` is not substituted here.** It is only resolved by the `octomind vars` command listing, not in `system`/`welcome` prompts. Using `{{HOME}}` in a role prompt leaves the literal text in place — use an absolute path or `{{CWD}}` instead.
