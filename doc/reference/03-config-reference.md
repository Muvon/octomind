# Configuration Reference: Octomind TOML Config Fields and Defaults

Use this reference when editing Octomind’s TOML configuration or diagnosing a load error, with every field and its
default. It covers the complete shipped template, optional parsed fields, and model, role, tool, and plugin
settings.

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

This allows organizing config by concern (e.g., `mcp-github.toml`, `commands-custom.toml`).

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
| `version` | u32 | `36` | Config version. Do not modify. Used for automatic upgrades. |
| `log_level` | string | `"info"` | Logging verbosity: `"none"`, `"info"`, `"debug"` |
| `default` | string | `"assistant"` | Default agent for bare `run`, `acp`, and `server`. See note below. |
| `sandbox` | bool | `false` | Restrict writes for `run`, `acp`, and `server`; those commands also accept `--sandbox`. |
| `telemetry` | bool | `true` | Anonymous usage telemetry. Overridden per-run by `OCTOMIND_TELEMETRY`, and by `DO_NOT_TRACK=1` before either. See [Telemetry](04-environment-variables.md#telemetry) for the exact field list. |
| `commit_attribution` | string | `"Co-Authored-By: Octomind <345424151+octomindat3am@users.noreply.github.com>"` | Git trailer the agent appends to commits it writes, unless the user says otherwise about attribution. `""` = no trailer and no system-prompt instruction. |

> **About the `default` value:** `"assistant"` is a **tap agent** shipped by the built-in default tap `muvon/tap` (which resolves to the GitHub repo `github.com/muvon/octomind-tap`) — *not* a role defined in this config file. It is the global assistant. A name resolves against your local `[[roles]]` first, so a local role with the same name as a tap agent takes priority; otherwise it starts the tap agent of that name. The value is a tag: `<namespace>` for the base agent of a namespace, or `<namespace>:<variant>` for a variant from a private tap; `<namespace>:default` is the base agent.

```toml
version = 36
log_level = "info"
default = "assistant"
sandbox = false
telemetry = true
commit_attribution = "Co-Authored-By: Octomind <345424151+octomindat3am@users.noreply.github.com>"
```

## Performance & Limits

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mcp_response_tokens_threshold` | usize | `20000` | Hard limit on MCP response tokens. Responses truncated when exceeded. Also the shared per-prompt budget for ACP embedded text resources and file links. `0` = unlimited. |
| `max_session_tokens_threshold` | usize | `300000` | Full-context safety limit, capped further by model window minus output reservation. `0` removes this configured limit, not the model ceiling. Maximum `2,000,000`; unrecoverable overflow errors before a provider request. |
| `cache_keepalive_enabled` | bool | `false` | Keep the prompt cache warm with pings while the session waits for a reply or a long tool run. Interval is 0.9 × the TTL of the newest cache marker (270s for the 5m conversation cache). Each ping is a cache pre-warm (`max_tokens: 0`, cache read only, no output). The ping count per wait is priced from model pricing and the session's own waits (replies, background work and tool runs learned separately), bounded by `(cache_write − cache_read) / cache_read × tail share` pings, by two pings until three waits of a kind are measured, and by the longest wait of that kind seen; a known scheduled wake is bridged when affordable. No ping is sent when the next request compresses or trims the conversation anyway, and a ping that misses the cache or is rate-limited stops the keepalive. `/info` reports the pings sent, their cost and the last budget. Only providers with a keepalive policy are pinged; through `octohub:` (>= 0.16.3) the hub pre-warms only the lane that served the conversation, when its provider has a policy. |

```toml
mcp_response_tokens_threshold = 20000
max_session_tokens_threshold = 300000
cache_keepalive_enabled = false
```

## User Interface

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_markdown_rendering` | bool | `true` | Pretty-print AI responses with markdown rendering. |
| `markdown_theme` | string | `"default"` | Theme: `"default"`, `"dark"`, `"light"`, `"ocean"`, `"solarized"`, `"monokai"` |
| `max_session_spending_threshold` | f64 | `0.0` | USD spent since last accepted checkpoint; interactive CLI asks to continue, piped/ACP/WebSocket declines. `0.0` = no limit. |
| `max_request_spending_threshold` | f64 | `0.0` | Request spending limit. Stops execution when exceeded. `0.0` = no limit. |
| `max_identical_rounds` | usize | `10` | Hard stop for runaway tool loops: when this many tool rounds in a row repeat calls and results already seen since the last new result (the previous round again, or a cycle through a few), the turn ends with a notice instead of another model request. `0` = off. Required in the file; the version 32 upgrade adds it, taking the value of the former `supervisor.max_identical_rounds`. |

```toml
enable_markdown_rendering = true
markdown_theme = "default"
max_session_spending_threshold = 0.0
max_request_spending_threshold = 0.0
max_identical_rounds = 10
```

## Capability auto-activation

| Field | Type | Default | Description |
|---|---|---|---|
| `auto_capabilities` | bool | `true` | Enable automatic capability activation on user messages. Disable to require manual `capability(action="enable")` calls. |

```toml
# Root keys must precede the first table header.
auto_capabilities = true
```

## `plugins`

A plugin is a separate program that takes part in the agent loop. Octomind sends it an event at fixed points (session
start, each prompt, before and after tool calls, the end of a turn, and so on) and applies the effects it answers
with. The list is optional; an absent or empty list means a plain agent loop. See [Plugins](../usage/14-supervisor.md)
for how octomind starts and supervises them.

The shipped `config-templates/default.toml` has `plugins = []`. A **new** configuration, created on first run or by
`octomind config`, lists one entry, Besydr (an inline array, because root keys must precede the first table header):

```toml
plugins = [
	{ name = "besydr", type = "stdio", command = "besydr", args = ["connect"], install_manifest = "https://cdn.besydr.com/latest.json", install_key = "uU5IoojVdfJZj5/fU4JQE8CXnmYXNBGo5nyLgWcNNrw=" },
]
```

If `besydr` is not installed, octomind downloads it on first use from the signed release the entry names and never
updates it by itself. To run without it, set `enabled = false` on the entry or remove the entry. Entries may also be
written as `[[plugins]]` tables, for example in a split-config file.

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | required | Unique name, used in notices and logs. A duplicate name is a validation error. |
| `type` | enum | required | `"stdio"` (octomind starts the program and talks over its standard input and output), `"socket"` (a local unix socket; not supported on Windows), or `"http"` (octomind posts each message to a URL). |
| `command` | string | absent | Program to start. Required for `stdio`; for `socket` it is started only when nothing answers on `path`. |
| `args` | string array | `[]` | Arguments for `command`. |
| `path` | string | absent | Socket path. Required for `socket`. |
| `url` | string | absent | Endpoint, starting with `http://` or `https://`. Required for `http`. |
| `headers` | string map | `{}` | Extra request headers for `http`. Values support `{{ENV:KEY}}`. |
| `enabled` | bool | `true` | Set `false` to keep the entry but not start the plugin. |
| `roles` | string array | `[]` | Roles the plugin applies to; empty means all roles. An entry matches the running agent's tag exactly, or names its namespace, so `developer` also covers `developer:acme`. |
| `timeout_seconds` | u64 | absent | Upper limit in seconds for how long octomind waits for one event's answer without hearing from the plugin, `1` to `3600`. A plugin declares that time per event (30 seconds for an event it declares none, and for the handshake); with this set, the smaller value applies. |
| `install_manifest` | string | absent | `https://` URL of a signed release manifest. When `command` is not installed, octomind downloads the program for this machine from it. Only for `stdio` and `socket`; needs `command` and `install_key`. |
| `install_key` | string | absent | Ed25519 public key (base64 of 32 bytes) the manifest must be signed with. Needs `install_manifest`; each is invalid without the other. |

Unknown keys in an entry are rejected. When a plugin cannot be started or does not answer, octomind goes on without it
and shows a notice; after 3 unanswered events in a row the plugin is paused for 30 seconds and then tried again (see
[Failures](../integration/05-plugins.md#failures)). Besydr's own settings live in Besydr's config file, not here: see
[Besydr's configuration](https://besydr.com/docs/configuration).

## `[model]`

The complete main model profile and inheritance baseline. Persistent role and compression profiles use the
same fields; name-only tap/workflow overrides retain the inherited parameters.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"octohub:auto"` | Provider-qualified model identifier |
| `reasoning_effort` | enum | `"medium"` | `"none"`, `"low"`, `"medium"`, `"high"`, `"xhigh"`, or `"max"`. `"none"` requests no reasoning and switches thinking off where the provider has a switch (Alibaba, DeepSeek, Z.AI, OpenRouter, OpenAI); the default for `[compression.model]`, whose output is a summary, not a decision |
| `max_tokens` | u32 | `32768` | Maximum output tokens; `0` uses provider behavior |
| `max_retries` | u32 | `1` | Provider retry attempts |
| `retry_timeout` | u64 | `30` | Exponential-backoff base in seconds |
| `request_timeout_seconds` | u64 | `300` | Total deadline per HTTP/WebSocket request attempt; `0` disables it |
| `stream_timeout_seconds` | u64 | `0` | SSE/WebSocket response inactivity limit; `0` disables it; never extends the total deadline |
| `request_transport` | enum | `"websocket"` | `"websocket"` or `"http"`. Applies to `octohub:` and `chatgpt:` models only; other providers ignore it. Required in the file; the v26 migration adds it as `"websocket"` |

`request_transport` applies to every `octohub:` and `chatgpt:` model call, including compression
calls that inherit it. Set `request_transport = "http"` to opt out. WebSocket errors do not silently fall back to
HTTP or replay a request. The library derives the Octohub WebSocket URL from the existing Octohub HTTP(S) base URL,
preserving private hosts and ports; no separate endpoint setting is required.

ChatGPT WebSocket calls: each received response event or ping/pong resets the inactivity timer; waiting for a
session connection does not consume it. Connection setup has a separate deadline.

```toml
[model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 32768
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
stream_timeout_seconds = 0
request_transport = "websocket"
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

String-to-string map, default `{}`, of tap agent tag to model name. A key is a tag (`"acme:reviewer"`) or a namespace
(`"acme"`, the base agent and every variant without a key of its own); an exact tag wins over its namespace. It changes
only `name`; all other parameters come from the main profile before any independent role override.

```toml
[taps]
"developer" = "ollama:glm-5.3"
"assistant" = "openai:gpt-5.6-luna"
```

**Priority (highest wins):** explicit runtime override > the active role's `[roles.model]` > the tap name mapping >
`[model]`.

1. `--model` CLI flag (if provided)
2. The `model` the agent's role/manifest declares (for `developer`, the manifest's role model)
3. Main `[model]` profile — its `name` is replaced by the matching tap mapping when present

`[taps]` only applies to tap agents. Plain roles resolve `[roles.model]` directly against `[model]`.

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
| `assistant` | Helpful assistant with working directory; `Hello! Ready to code. Working in {{CWD}} (Role: {{ROLE}})` | `["core", "orchestration", "runtime", "filesystem"]` | `["core:*", "orchestration:*", "runtime:*", "filesystem:*"]` |
| `task_refiner` | Refine the real user request; `""` welcome. | `[]` | `[]` |
| `task_researcher` | Gather focused context; `""` welcome. | `["filesystem"]` | `["view"]` |
| `reduce` | Retain architectural history; `""` welcome. | `[]` | `[]` |

### `[roles.model]`

Optional partial model profile for the role. Every omitted field inherits its `[model]` value. The example below
changes only name and effort.

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
server_refs = ["core", "orchestration", "runtime", "filesystem"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*"]
```

## `[mcp]`

Global MCP (Model Context Protocol) configuration.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_tools` | string[] | `[]` | Global tool filter; role merging replaces it with that role’s `allowed_tools`, including an empty list. |
| `code_mode` | bool | `true` | On: external MCP tools (stdio/http) are not sent to the model as tool definitions; the builtin `code` tool lists them by signature and the model calls them from scripts, in every session whatever the role grants. Off: every tool is sent as a tool definition and there is no `code` tool. |

### `[[mcp.servers]]`

MCP server definitions. Three types supported: `builtin`, `http`, `stdio`.

**Builtin servers** (declared in the template; enabled through role references or exact `auto_bind`, no external
process):

| Server | Tools | Description |
|--------|-------|-------------|
| `core` | `recall` (when attention or governance is enabled) | Session-memory retrieval; governance defaults on |
| `orchestration` | `tap`, `schedule`, `monitor` | Delegation, scheduled messages, and event-stream monitoring |
| `runtime` | `mcp`, `skill`, `capability` | Harness and tool-surface reconfiguration |

> **`filesystem` is not declared here.** It is an external `stdio` server backed by octofs and provided through tap capabilities. Tool availability comes from the installed external server. `/mcp full` shows the installed server's authoritative schemas.

### Common server fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique server identifier |
| `type` | string | yes | `"builtin"`, `"http"`, or `"stdio"` |
| `timeout_seconds` | u64 | yes | Per-operation timeout; tool-call progress resets this idle deadline, and it holds while the user answers an MCP elicitation (template: 30) |
| `tools` | string[] | yes | Tool filter. Empty = all tools. Supports wildcards such as `"github_*"`. |
| `auto_bind` | string[] | no; absent | Exact role names to auto-include this server for. |

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

The template declares `core`, `runtime`, and `orchestration`, each with `type = "builtin"`, `timeout_seconds =
30`, and `tools = []`. HTTP/stdio entries are commented examples, not active defaults. Timeouts must be 1–3600 seconds.
Auto-bound servers also gain `server_refs` and, when restricted, a `server:*` allowance in the merged role. Executables
in `.agents/tools/` form an additional dynamically registered local tool surface; they are not another template server
entry.

```toml
[mcp]
allowed_tools = []
code_mode = true

[[mcp.servers]]
name = "search"
type = "http"
url = "http://localhost:9000/mcp"
headers = { Authorization = "Bearer {{ENV:MY_MCP_TOKEN}}" }
timeout_seconds = 30
tools = []
auto_bind = ["developer"]

[[mcp.servers]]
name = "files"
type = "stdio"
command = "octofs"
args = []
env = { PROJECT_LABEL = "demo" }
cwd = "."
timeout_seconds = 30
tools = []
auto_bind = ["developer"]
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

Custom session commands triggered with `/run <name>`. Each entry delegates to a role over ACP — the actual model, system
prompt, and MCP configuration live in `[[roles]]`, not here. `name` is the token you type after `/run`. See the field
table below, including the required `input_mode` / `output_mode` / `output_role` fields.

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

> **Multi-step AI workflows** are no longer defined in this config. Use the external CLI: `octomind workflow <file.toml>` — see [doc/usage/09-workflows.md](../usage/09-workflows.md).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Command name, typed after `/run` |
| `description` | string | required | Human-readable description (used in help, MCP) |
| `command` | string | required | ACP command to execute: `"octomind acp <role_name>"` |
| `workdir` | string | `"."` | Working directory (relative to session workdir). The only optional field. |
| `input_mode` | string | **required** | How input is fed: `"last"`, `"all"`, `"summary"` |
| `output_mode` | string | **required** | How output affects session: `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | **required** | Role for output messages: `"assistant"`, `"user"` |

> `input_mode`, `output_mode`, and `output_role` have **no default** — config loading fails if any is omitted. Only `workdir` is optional.

## Migrating legacy agents

Version 36 removes the stock `[[agents]]` example. Custom ACP commands and synchronous tool returns have
no automatic equivalent: preserve their command and workdir, migrate their callers, then remove the entries.
No tap permission is added automatically. For role-based delegation see [tap roles](../use-cases/05-multi-agent-delegation.md).

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

## `[timing]`

Per-person calibration of the human-time and energy estimate in `/report`. The defaults are starting guesses; see
[Human Time and Energy](../usage/05-sessions.md#human-time-and-energy) for the model, its sources, and the calibration
procedure. Reading, review and typing speeds and the flag thresholds are fixed research values in
[timing.rs](../../src/session/timing.rs), not config.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `think_overhead_min` | f64 | `1.5` | Thinking time added to every turn's reading + typing estimate, minutes |
| `deliberation_factor` | f64 | `2.0` | Cap on active time before an input, as a multiple of the turn estimate |
| `attention_window_min` | f64 | `30.0` | Minutes after your last input during which an agent run earns wait time |
| `review_weight` | f64 | `2.0` | Energy per minute of reading code, relative to dialog, spec writing and behavior checks (weight 1) |
| `wait_weight` | f64 | `0.25` | Energy per minute of attended waiting |
| `switch_cost_dhe` | f64 | `0.05` | Energy cost of one switch between parallel sessions, DHE |

```toml
[timing]
think_overhead_min = 1.5
deliberation_factor = 2.0
attention_window_min = 30.0
review_weight = 2.0
wait_weight = 0.25
switch_cost_dhe = 0.05
```

Configs from before schema version 20 gain this section on upgrade; values already present are kept.

## `[compression]`

Automatic context compression system.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `knowledge_retention` | usize | `25` | Max critical knowledge entries retained across compressions |
| `analysis_findings_max_tokens` | usize | `6000` | Hard token budget for retained analysis findings; `0` disables retention |
| `threshold` | usize | `120000` | Baseline automatic compression trigger in tokens; `0` disables automatic checks, while `/done` can still force compression |

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
| `reasoning_effort` | enum | `"none"` | Thinking effort override; compression produces summaries rather than decisions, so the shipped default disables reasoning where supported |
| `max_tokens` | u32 | `32000` | Max tokens for decision + summary |
| `max_retries` | u32 | `1` | Retry attempts |
| `retry_timeout` | u64 | `30` | Retry backoff base (seconds) |
| `request_timeout_seconds` | u64 | `300` | Total deadline per HTTP/WebSocket request attempt; `0` disables it |
| `stream_timeout_seconds` | u64 | `0` | SSE/WebSocket response inactivity limit; `0` disables it; never extends the total deadline |
| `request_transport` | enum | `"websocket"` | `"websocket"` or `"http"` (octohub and chatgpt only) |

```toml
[compression]
knowledge_retention = 25
analysis_findings_max_tokens = 6000
threshold = 120000

[compression.model]
name = "octohub:auto"
reasoning_effort = "none"
max_tokens = 32000
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

## Parsed fields absent from active template settings

These are accepted configuration surfaces, not additional shipped defaults. Runtime-only `role_map`,
`runtime_output_mode`, `working_directory`, and `config_path` are skipped by serde and cannot be set in TOML.

| Path | Type | Omitted behavior | Meaning |
|---|---|---|---|
| Root `system` | optional string | Absent | Legacy serialized prompt; session prompts still come from roles. Mentioned only in template comments; place it before any table. |
| `roles.model` | partial profile or model string | Inherit main profile | All `[model]` fields are accepted; the template roles omit it. |
| `mcp.servers.auto_bind` | optional string array | No automatic binding | Exact-match role/tag activation. |
| Stdio `mcp.servers.env` | string map | `{}` | Child environment overrides. |
| Stdio `mcp.servers.cwd` | optional string | Inherit working directory | Child process directory. |
| `commands.workdir` | string | `"."` | Execution directory relative to session workdir. |
| Legacy `compression.decision` | partial profile | Used only if `compression.model` is absent | Load normalization maps it to `[compression.model]`; a nested `model` string becomes `name`. |
| Legacy flat model fields on root/role | Same parameter types as `[model]` | Fill nested-profile gaps | `reasoning_effort`, `max_tokens`, `max_retries`, `retry_timeout`, `request_timeout_seconds`, `stream_timeout_seconds`, `request_transport`. |
| `registry.cache_ttl_hours` | u64 | `24` | Manifest refresh interval, described below. |
| `registry.tap`, `registry.ref` | string | required; the template sets them | Built-in tap and the revision of it to serve, described below. |

The root `model` string and historical flat model parameters are migration/normalization inputs, not the version-12
output format; use `[model]`. The `ProvidersConfig`/`OpenRouterConfig` compatibility types in `src/config/providers.rs`
are not fields of the loaded `Config`; they do not enable TOML credential storage.

`temperature`, `top_p`, and `top_k` are deprecated since config version 34. They are ignored and reported as
deprecated when loaded; `octomind config --upgrade` removes them, and they will be removed in a later release.

## `[registry]`

Controls caching of agent manifests fetched from taps. Registry sources themselves are managed with `octomind tap
<user/repo> [path]` and `octomind untap <user/repo>`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cache_ttl_hours` | u64 | `24` | How long a fetched tap manifest is cached before re-checking. |
| `tap` | string | none; required, the template sets `"muvon/tap"` | The built-in tap, in `user/repo` form. It is always loaded, after the taps you add with `octomind tap`, and cannot be added or removed with that command. |
| `ref` | string | none; required, set by the config template | Branch, tag or commit of the built-in tap to serve. Taps you add yourself follow their default branch. Octomind has no built-in fallback for either key; the config upgrade writes them. |

Fetched manifests are cached at `<data>/agents/<tag>.<source>.toml`, with the `:` of a variant tag written as `_`, where `<source>` is the tap, and for the built-in tap the `ref` as well. Within the TTL the cached manifest is served
immediately; once stale, the cached copy is still served (stale-serve) while a background refresh fetches the latest
version. See [Tap System](../integration/04-tap-system.md) for the registry behavior.

```toml
[registry]
cache_ttl_hours = 24
tap = "muvon/tap"
ref = "<branch|tag|commit>"
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

`when` conditions on guards/validators use `+target` (used) or `-target` (not used). A role filter here matches the running agent's tag exactly,
or names its namespace (`developer` covers `developer:acme`). Main-config MCP `auto_bind` remains exact-match only. Project `[[hook]]` is distinct from the
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
[mcp.rs](../../src/config/mcp.rs), [plugins.rs](../../src/config/plugins.rs),
[layer_trait.rs](../../src/session/layers/layer_trait.rs).

## See also

- [CLI Reference](01-cli-reference.md)
- [Session Commands](02-session-commands.md)
- [Environment Variables](04-environment-variables.md)
- [Configuration Guide](../usage/03-configuration.md)
