# Configuration Reference

Use this reference when editing Octomind’s TOML configuration or diagnosing a load error. It covers the complete shipped
template, optional parsed fields, and model, role, tool, and supervisor settings.

## Get started

```bash
octomind config
octomind config --show
octomind config --validate
```

The default path is `~/.local/share/octomind/config/config.toml` on macOS/Linux and
`%LOCALAPPDATA%\octomind\config\config.toml` on Windows. See [Environment Variables](04-environment-variables.md) for
path and credential overrides. `--show` displays selected settings, not a complete TOML dump. Startup can create or
migrate configuration even for inspection commands. Use [default.toml](../../config-templates/default.toml) for the
complete starting file.

The tables below follow template order. “Default” means the value shipped in that template, not permission to omit a
required key. Examples are fragments to merge into that complete file; replace an existing table instead of declaring it
twice. Arrays of named entries require complete replacements when overriding a name.

## Multi-File Configuration

Octomind supports split-file configuration. All `*.toml` files in the config directory are merged:

1. `config.toml` is loaded first
2. Other `*.toml` files are loaded alphabetically
3. Tables deep-merge; arrays of tables concatenate; scalar arrays such as `allowed_tools` replace
4. Array entries with the same `name` are deduplicated (the entire last entry wins; it is not a field patch)
5. Scalar values are overridden by later files

This allows organizing config by concern (e.g., `mcp-github.toml`, `layers-custom.toml`).

**Special Case: `mcp-*.toml` Override Files**

Files matching the pattern `mcp-*.toml` are loaded **AFTER** all other `*.toml` files, regardless of their alphabetical
position. This ensures they can reliably override same-named MCP servers defined in earlier files like `mcp.toml`.

Without this special handling, `mcp.toml` would lexicographically sort after `mcp-github.toml` and silently overwrite
any server overrides.

This mechanism is used by the model-callable `mcp` tool’s `persist` action, which writes to
`<config_dir>/mcp-<name>.toml` with `auto_bind = ["<role>"]` when enabled. Persisting a disabled server omits `auto_bind`;
it stays defined but is not automatically enabled on the next startup.

The selected `OCTOMIND_CONFIG_PATH` determines the directory to merge and the primary migration/save path; it does not
restrict loading to one file. The file literally named `config.toml` still sorts first. Ordinary mutations save to the
loaded path; `config --upgrade` and displayed path labels use the standard `<data>/config/config.toml`. If no TOML
exists, loading creates the embedded default config. Existing files must collectively supply required fields; they are
not automatically overlaid on the template.

For a split config, put the following in `model-local.toml` beside `config.toml`:

```toml
[model]
reasoning_effort = "high"
```

```bash
octomind config --validate
```

## Root-Level Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `version` | u32 | `12` | Config version. Do not modify. Used for automatic upgrades. |
| `log_level` | string | `"info"` | Logging verbosity: `"none"`, `"info"`, `"debug"` |
| `default` | string | `"assistant:concierge"` | Default tag for bare `run`, `acp`, and `server`. See note below. |
| `sandbox` | bool | `false` | Restrict writes for `run`, `acp`, and `server`; those commands also accept `--sandbox`. |
| `telemetry` | bool | `true` | Anonymous usage telemetry. Overridden per-run by `OCTOMIND_TELEMETRY`, and by `DO_NOT_TRACK=1` before either. See [Telemetry](04-environment-variables.md#telemetry) for the exact field list. |

> **About the `default` value:** `"assistant:concierge"` is a **tap agent** addressed as `category:variant`, shipped by the built-in default tap `muvon/tap` (which resolves to the GitHub repo `github.com/muvon/octomind-tap`) — *not* a role defined in this config file. If you search this file for a `concierge` role you will not find one. A bare tag without a colon (e.g. `"developer"`) resolves against your local `[[roles]]`; a `category:variant` tag resolves against installed taps.

```toml
version = 13
log_level = "info"
default = "assistant:concierge"
sandbox = false
telemetry = true
```

## Performance & Limits

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mcp_response_tokens_threshold` | usize | `20000` | Hard limit on MCP response tokens. Responses truncated when exceeded. `0` = unlimited. |
| `max_session_tokens_threshold` | usize | `200000` | Full-context safety limit, capped further by model window minus output reservation. `0` removes this configured limit, not the model ceiling. Maximum `2,000,000`; unrecoverable overflow errors before a provider request. |
| `cache_keepalive_enabled` | bool | `false` | Keep prompt cache warm with periodic pings while the session idles. Provider-aware: currently **only Anthropic** is pinged, and the ping interval comes from the provider's cache TTL (1h), not from this config. |
| `cache_keepalive_max_idle_seconds` | u64 | `1800` | Stop pinging this many seconds after last user activity. `0` = ping until session ends. Validation fails if `> 86400` (24h). |

```toml
mcp_response_tokens_threshold = 20000
max_session_tokens_threshold = 200000
cache_keepalive_enabled = false
cache_keepalive_max_idle_seconds = 1800
```

## User Interface

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_markdown_rendering` | bool | `true` | Pretty-print AI responses with markdown rendering. |
| `markdown_theme` | string | `"default"` | Theme: `"default"`, `"dark"`, `"light"`, `"ocean"`, `"solarized"`, `"monokai"` |
| `max_session_spending_threshold` | f64 | `0.0` | USD spent since last accepted checkpoint; interactive CLI asks to continue, piped/ACP/WebSocket declines. `0.0` = no limit. |
| `max_request_spending_threshold` | f64 | `0.0` | Request spending limit. Stops execution when exceeded. `0.0` = no limit. |

```toml
enable_markdown_rendering = true
markdown_theme = "default"
max_session_spending_threshold = 0.0
max_request_spending_threshold = 0.0
```

## Capability auto-activation

| Field | Type | Default | Description |
|---|---|---|---|
| `auto_capabilities` | bool | `true` | Enable automatic capability activation on user messages. Disable to require manual `capability(action="enable")` calls. |

```toml
# Root keys must precede the first table header.
auto_capabilities = true
```

## `[model]`

The complete main model profile and inheritance baseline. Persistent role, supervisor, and compression profiles use the
same fields; name-only tap/workflow overrides retain the inherited parameters.

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

```toml
[model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 32768
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

## `[capabilities]`

Map of capability name to provider override. Used by tap agents to route specific capabilities to different providers.

```toml
[capabilities]
codesearch = "octocode"  # uses capabilities/codesearch/octocode.toml
```

Type: map of strings to strings; default `{}`. Each key maps to a provider TOML file within the tap's `capabilities/`
directory.

## `[taps]`

String-to-string map, default `{}`, of tap agent tag to model name. It changes only `name`; all other parameters come
from the main profile before any independent role override.

```toml
[taps]
"developer:general" = "ollama:glm-5.3"
"assistant:concierge" = "openai:gpt-5.6-luna"
```

**Priority (highest wins):** explicit runtime override > the active role's `[roles.model]` > the tap name mapping >
`[model]`.

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

The shipped role entries are below; the full default `system` strings are in the linked template. None of these four
entries declares a model override. `system` and `welcome` are required strings, with no generic default for a new role.

| Name | Default system purpose / welcome | Default `server_refs` | Default `allowed_tools` |
|---|---|---|---|
| `assistant` | Helpful assistant with working directory; `Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})` | `["core", "orchestration", "runtime", "filesystem", "agent"]` | `["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]` |
| `task_refiner` | Refine the real user request; `""` welcome. | `[]` | `[]` |
| `task_researcher` | Gather focused context; `""` welcome. | `["filesystem"]` | `["view"]` |
| `reduce` | Retain architectural history; `""` welcome. | `[]` | `[]` |

### `[roles.model]`

Optional partial model profile for the role: `name` (string), `reasoning_effort` (enum), `max_tokens` (u32),
`temperature`/`top_p` (f32), `top_k`/`max_retries` (u32), and `retry_timeout`/`request_timeout_seconds` (u64). Every
omitted field inherits its `[model]` value. The example below changes only name and effort.

### `[roles.mcp]`

MCP configuration for the role. Omitting the whole table gives empty lists; if the table is present, both lists are
required. The shipped values vary by role as shown above.

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
| `allowed_tools` | string[] | `[]` | Global tool filter; role merging replaces it with that role’s `allowed_tools`, including an empty list. |

### `[[mcp.servers]]`

MCP server definitions. Three types supported: `builtin`, `http`, `stdio`.

**Builtin servers** (declared in the template; enabled through role references or exact `auto_bind`, no external
process):

| Server | Tools | Description |
|--------|-------|-------------|
| `core` | `recall` (when attention or governance is enabled) | Session-memory retrieval; governance defaults on and planning is supervisor-internal |
| `orchestration` | `tap`, `schedule`, `monitor` | Delegation, scheduled messages, and event-stream monitoring |
| `runtime` | `mcp`, `agent`, `skill`, `capability` | Harness and tool-surface reconfiguration |
| `agent` | `agent_<name>` per `[[agents]]` entry | ACP sub-agent dispatch |

> **`filesystem` is not declared here.** It is an external `stdio` server backed by octofs and provided through tap capabilities. Tool availability comes from the installed external server. `/mcp full` shows the installed server's authoritative schemas.

### Common server fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique server identifier |
| `type` | string | yes | `"builtin"`, `"http"`, or `"stdio"` |
| `timeout_seconds` | u64 | yes | Per-operation timeout; tool-call progress resets this idle deadline (template: 30) |
| `tools` | string[] | yes | Tool filter. Empty = all tools. Supports wildcards such as `"github_*"`. |
| `auto_bind` | string[] | no; absent | Exact role/tag strings to auto-include this server for; `developer` does not match `developer:general`. |

### HTTP server fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | yes | Server endpoint URL |
| `headers` | string map | no; `{}` | Headers sent on every request. Values support `{{ENV:KEY}}` placeholders. A configured `Authorization` header disables OAuth discovery. |

> **Authentication:** Configure a static `Authorization` header for bearer tokens or API keys. Without one, Octomind uses MCP Authorization Discovery (RFC 9728), registers via CIMD/DCR, and authenticates using PKCE.

### Stdio server fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `command` | string | yes | Executable to run |
| `args` | string[] | yes | Command arguments; use `[]` when none |
| `env` | string map | no; `{}` | Child environment entries; values support `{{ENV:KEY}}` placeholders |
| `cwd` | string | no; absent | Child working directory; omitted inherits Octomind's working directory (plugins may set their root) |

The template declares `core`, `runtime`, `agent`, and `orchestration`, each with `type = "builtin"`, `timeout_seconds =
30`, and `tools = []`. HTTP/stdio entries are commented examples, not active defaults. Timeouts must be 1–3600 seconds.
Auto-bound servers also gain `server_refs` and, when restricted, a `server:*` allowance in the merged role. Executables
in `.agents/tools/` form an additional dynamically registered local tool surface; they are not another template server
entry.

```toml
[mcp]
allowed_tools = []

[[mcp.servers]]
name = "search"
type = "http"
url = "http://localhost:9000/mcp"
headers = { Authorization = "Bearer {{ENV:MY_MCP_TOKEN}}" }
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]

[[mcp.servers]]
name = "files"
type = "stdio"
command = "octofs"
args = []
env = { PROJECT_LABEL = "demo" }
cwd = "."
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

The HTTP example needs your running endpoint and `MY_MCP_TOKEN`; the stdio example needs `octofs` installed. Inspect the
actual enabled surface inside a session:

```text
/mcp full
```

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
script = "/tmp/octomind-hook.sh"
timeout = 30
```

No hooks are active in the template. To try the example, create its script, add the TOML above, then launch and send a
payload (the final command goes in another terminal):

```bash
printf '#!/bin/sh\ncat\n' > /tmp/octomind-hook.sh
chmod +x /tmp/octomind-hook.sh
echo "Wait for webhook messages" | octomind run --name hooks-demo --daemon --format jsonl --hook github-push
curl --data 'Summarize the current task status' http://127.0.0.1:9876/
```

## `[[commands]]`

Custom session commands triggered with `/run <name>`. **Uses the exact same schema as `[[layers]]`** (same `LayerConfig`
struct) — see the field table below, including the required `input_mode` / `output_mode` / `output_role` fields. The
only difference is invocation: `[[commands]]` entries are run manually from a session via `/run <name>`, while
`[[layers]]` are orchestration units invoked over ACP. For `[[commands]]`, `name` is the token you type after `/run`.

```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

The template ships only the `reduce` command shown above. Run it inside an existing session:

```text
/run reduce
```

## `[[layers]]`

Optional reusable ACP-invocable units; the template has no active `[[layers]]` entries. `[[commands]]` uses the same
schema. Layers delegate to roles via the ACP protocol — the actual model, system prompt, and MCP configuration live in
`[[roles]]`, not here.

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
command = "octomind acp task_researcher"
workdir = "."
```

The template’s one agent is named `context_gatherer`, with `workdir = "."` and command `octomind acp context_gatherer`.
It does not define a matching `context_gatherer` role. The example above points the same tool at the shipped
`task_researcher` role; that role still needs an available `filesystem` server.

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

The template ships `review`, `explain`, `optimize`, `test`, and `debug`, each with its own description and prompt text
in [default.toml](../../config-templates/default.toml). There is no generic default for `name` or `prompt`;
`description` is absent when omitted. Text is injected verbatim, without variable substitution.

```text
/prompt review
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
| `threshold` | usize | `70000` | Baseline automatic compression trigger in tokens; `0` disables automatic checks, while `/done` can still force compression |

The automatic trigger adapts within a long turn: successive folds raise the baseline geometrically, capped under the
usable ceiling; a real user turn resets it. Compression may therefore wait beyond `threshold`.

> **Depth is computed, not configured.** When compression becomes eligible, how deep each compression goes is derived per cycle from the measured session growth rate and the context ceiling — the lower of `max_session_tokens_threshold` (see Performance & Limits) and the session model's usable window. The derived ratio always lands in [2.0, 16.0].

### `[compression.attention]`

Optional PACT-style provenance and archive governance around compression.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable provenance-labelled causal evidence selection and rendering |
| `validator` | bool | `true` | Reject optional compactions whose folded units have invalid attribution |
| `telemetry` | bool | `true` | Persist a content-free compression decision record beside the lossless archive |

### `[compression.attention.governance]`

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Preserve runtime-owned task pins and active frontier around compaction. |
| `verify_hash` | bool | `true` | Check governance hashes before committing compaction. |

```toml
[compression.attention]
enabled = false
validator = true
telemetry = true

[compression.attention.governance]
enabled = true
verify_hash = true
```

Keep scalar `[compression]` keys before nested `[compression.attention]` and `[compression.model]` headers; TOML assigns
later scalars to the most recent nested table.

### `[compression.model]`

Model used for compression decisions and summary generation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"octohub:auto"` | Compression model name |
| `reasoning_effort` | enum | `"medium"` | Thinking effort override |
| `max_tokens` | u32 | `16000` | Max tokens for decision + summary |
| `temperature` | f32 | `0.3` | Lower = more consistent decisions |
| `top_p` | f32 | `1.0` | Nucleus sampling |
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

The out-of-band control plane around the agent loop. It hosts learning (distill + recall + orientation memory),
deterministic detectors, the verify-gate, the external plan manager, and condense. See the [Supervisor
guide](../usage/14-supervisor.md) for how the mechanics fit together. **Strict:** the `[supervisor]` section and its
required keys must be present — a missing section or key is a hard parse error, not a silent default.

When the supervisor is active, deterministic detectors, goal recitation, and check-after-mutation pre-gates use fixed
internal thresholds. The `enabled` switches below control their respective model-driven mechanics.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for the whole control plane |

### `[supervisor.model]`

Optional partial profile shared by every supervisor mechanic: authorizer, gate, resolve, plan, condense, extraction, recall,
retention, verification, and evolution. It accepts every field from `[model]`; omitted fields inherit main. Omitting the
entire block uses `[model]` unchanged.

| Field | Type | Template default | Meaning |
|---|---|---|---|
| `name` | string | `"octohub:auto"` | Shared supervisor model. |
| `reasoning_effort` | enum | `"medium"` | Reasoning effort hint. |
| `max_tokens` | u32 | `8192` | Output token ceiling. |
| `temperature` | f32 | `0.0` | Sampling temperature. |
| `top_p` | f32 | `1.0` | Nucleus sampling. |
| `top_k` | u32 | `0` | Top-k sampling; disabled at zero. |
| `max_retries` | u32 | `1` | Retry attempts. |
| `retry_timeout` | u64 | `30` | Backoff base in seconds. |
| `request_timeout_seconds` | u64 | `300` | Per-request timeout; zero is unlimited. |

### `[supervisor.learning]`

Cross-session adaptive learning. Extracts lessons and orientation memory (durable subject understanding, recalled as
working assumptions to verify) from sessions and injects them into future sessions. See [Learning
Guide](../usage/13-learning.md) for full details.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the learning system (lessons + orientation) |

### `[supervisor.learning.evolution]`

Optional grounded behavior evolution. When enabled, newly stored quote-backed rules and verified experiences may produce
scoped native skill or guardrail candidates. Synthesis and admission both use the single `[supervisor.model]` profile,
which must support structured output. Thresholds and trial limits are fixed internal constants.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable detached candidate synthesis and lifecycle-managed trials |

### `[supervisor.gate]`

Verify-gate on self-reported completion. Free deterministic pre-gates run first (no model call); the LLM checklist runs
only if those pass.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the verify-gate |

### `[supervisor.authorizer]`

Checks the exact proposed tool operations against user intent after native pre-call guards and before execution.
Uses `[supervisor.model]`; unavailable decisions hold calls. Requires the supervisor master switch for root sessions.
Inherited delegation constraints remain enforced in child sessions. See [Tool authorization](../usage/14-supervisor.md#tool-authorization).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable user-intent tool authorization |

### `[supervisor.plan]`

Adaptive external plan manager. The specialist has no plan mutation tool; a sparse hidden signal emitted alongside real
work wakes this manager only when planning or a transition is needed.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable adaptive external planning |

### `[supervisor.condense]`

Task-aware narrowing of oversized plain-text tool outputs. A result whose own output exceeds `tokens_threshold` becomes
a candidate; smaller results in the same round are passed through untouched and never shown to the condenser. One shared
supervisor-model call per round selects, by original line ranges over a bounded query/diagnostic-aware view, what the
current task needs; kept lines are reconstructed verbatim, and irrelevant results get deterministic notices rather than
model-authored summaries. Full originals are spilled to session files first when the active role can read them back. The
`mcp_response_tokens_threshold` prefix-cut is applied **before** condensation.

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

[supervisor.authorizer]
enabled = false

[supervisor.plan]
enabled = true

[supervisor.condense]
enabled = true
adaptive = false
tokens_threshold = 5000
```

## Parsed fields absent from active template settings

These are accepted configuration surfaces, not additional shipped defaults. Runtime-only `role_map`,
`runtime_output_mode`, `working_directory`, and `config_path` are skipped by serde and cannot be set in TOML.

| Path | Type | Omitted behavior | Meaning |
|---|---|---|---|
| Root `system` | optional string | Absent | Legacy serialized prompt; session prompts still come from roles. Mentioned only in template comments; place it before any table. |
| `roles.model` | partial profile or model string | Inherit main profile | All nine `[model]` fields are accepted; the template roles omit it. |
| `roles.temperature`, `roles.top_p`, `roles.top_k` | optional f32, f32, u32 | Inherit main | Legacy flat sampling input; nested model values win and new serialization uses the nested profile. |
| `mcp.servers.auto_bind` | optional string array | No automatic binding | Exact-match role/tag activation. |
| Stdio `mcp.servers.env` | string map | `{}` | Child environment overrides. |
| Stdio `mcp.servers.cwd` | optional string | Inherit working directory | Child process directory. |
| `layers.workdir`, `commands.workdir` | string | `"."` | Execution directory relative to session workdir. |
| Legacy `compression.decision` | partial profile | Used only if `compression.model` is absent | Load normalization maps it to `[compression.model]`; a nested `model` string becomes `name`. |
| Legacy flat model fields on root/role/supervisor | Same eight parameter types as `[model]` | Fill nested-profile gaps | `reasoning_effort`, `max_tokens`, `temperature`, `top_p`, `top_k`, `max_retries`, `retry_timeout`, `request_timeout_seconds`. |
| Legacy `supervisor.learning.model` and flat model parameters | Ignored | Removed during normalization | Learning uses the shared supervisor profile. |
| `registry.cache_ttl_hours` | u64 | `24` | Manifest refresh interval, described below. |

The root `model` string and historical flat model parameters are migration/normalization inputs, not the version-12
output format; use `[model]`. The `ProvidersConfig`/`OpenRouterConfig` compatibility types in `src/config/providers.rs`
are not fields of the loaded `Config`; they do not enable TOML credential storage.

## `[registry]`

Controls caching of agent manifests fetched from taps. Registry sources themselves are managed with `octomind tap
<user/repo> [path]` and `octomind untap <user/repo>`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cache_ttl_hours` | u64 | `24` | How long a fetched tap manifest is cached before re-checking. |

Fetched manifests are cached at `<data>/agents/<category>/<variant>.toml`. Within the TTL the cached manifest is served
immediately; once stale, the cached copy is still served (stale-serve) while a background refresh fetches the latest
version. See [Tap System](../integration/04-tap-system.md) for the registry behavior.

```toml
[registry]
cache_ttl_hours = 24
```

## Guardrails (`.agents/guardrails.toml`)

This separate project file is parsed by `src/config/guardrails.rs`; none of these keys belongs in the main config. Each
table is an optional array, defaulting to empty. The complete operating guide and script examples are in
[Guardrails](../usage/18-guardrails.md).

| Table | Required keys (string) | Optional keys (type; default) | Meaning |
|---|---|---|---|
| `[[pipe]]` | `name`, `command` | `match` (regex string; absent), `when` (`first`/`any`; `any`), `roles` (string array; `[]`) | Transform input before the model. |
| `[[guard]]` | `match`, `message` | `has` (string or string array; `[]`), `when` (string array; `[]`) | Deny matching tool calls subject to capability/history conditions. |
| `[[hook]]` | `script` | `match` (target string; absent), `result` (regex string; absent), `on` (`success`/`error`/`any`; `any`) | Process tool results. |
| `[[validator]]` | `name`, `script` | `match` (final-message regex string; absent), `when` (string array; `[]`), `roles` (string array; `[]`) | Validate assistant turns. |

`when` conditions on guards/validators use `+target` (used) or `-target` (not used). Role filters here accept an exact
tag or domain prefix. Main-config MCP `auto_bind` remains exact-match only. Project `[[hook]]` is distinct from the
main-config HTTP listener `[[hooks]]`.

```toml
# .agents/guardrails.toml
[[guard]]
match = 'shell(command=^rm\s+-rf)'
message = "Recursive force deletion is blocked in this project."
```

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

```bash
octomind vars --preview
```

```toml
[[roles]]
name = "brief"
system = "Answer briefly. Working directory: {{CWD}}. Role: {{ROLE}}."
welcome = "Ready in {{CWD}}"

[roles.mcp]
server_refs = []
allowed_tools = []
```

```bash
octomind run brief
```

## Common questions

- **Why did an override lose fields?** Same-name array entries replace the entire earlier entry. Copy the
  complete role/server/command before changing it; scalar tables such as `[model]` deep-merge instead.
- **Why is a tool missing?** Check role `server_refs`, exact `auto_bind` tags, server `tools`, and role
  `allowed_tools`, then inspect `/mcp full`. A template role reference does not install an external server.
- **Why is a compression setting ignored?** Root compression keys must precede its nested table headers.
  Use `[compression.model]`; `[compression.decision]` is only a legacy load alias when the current table is absent.
- **Why did validation reject a short config?** Existing configs are strict, not sparse overlays on built-in
  defaults. Start from the complete template, then split it if needed.

## Source map

Defaults and long prompt strings: [default.toml](../../config-templates/default.toml). Loading and merging:
[loading.rs](../../src/config/loading.rs), [merge.rs](../../src/config/merge.rs). Schemas:
[config/mod.rs](../../src/config/mod.rs), [model.rs](../../src/config/model.rs), [roles.rs](../../src/config/roles.rs),
[mcp.rs](../../src/config/mcp.rs), [layer_trait.rs](../../src/session/layers/layer_trait.rs),
[supervisor/mod.rs](../../src/supervisor/mod.rs).

## See also

- [CLI Reference](01-cli-reference.md)
- [Session Commands](02-session-commands.md)
- [Environment Variables](04-environment-variables.md)
- [Configuration Guide](../usage/03-configuration.md)
