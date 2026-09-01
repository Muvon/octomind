# Configuration

How Octomind's TOML configuration works — file locations, merge order, model purposes, roles, tools, and common overrides.

## Create and Inspect the Configuration

Octomind creates the default configuration automatically when no TOML configuration exists. You can also create or inspect it explicitly:

```bash
octomind config             # create config.toml when absent
octomind config --show      # display key effective settings
octomind config --validate  # validate the merged configuration
octomind config --upgrade   # run the current migration explicitly
```

The default template is embedded in the binary from `config-templates/default.toml`. Older configurations are upgraded automatically during load; a migration writes a versioned backup beside the original before replacing it.

## File Locations

| Platform | Data directory | Main config file |
|----------|----------------|------------------|
| macOS | `~/.local/share/octomind/` | `~/.local/share/octomind/config/config.toml` |
| Linux | `~/.local/share/octomind/` | `~/.local/share/octomind/config/config.toml` |
| Windows | `%LOCALAPPDATA%/octomind/` | `%LOCALAPPDATA%/octomind/config/config.toml` |

The data directory also contains saved sessions, logs, cache data, taps, and learning records. Two environment variables relocate configuration and state:

| Variable | Effect |
|----------|--------|
| `OCTOMIND_DATA_DIR` | Replaces the platform data directory for config and other persistent state |
| `OCTOMIND_CONFIG_PATH` | Selects a specific main config file; its parent becomes the multi-file merge directory |

## Core Settings

The shipped root settings begin with:

```toml
version = 12
log_level = "info"
default = "assistant:concierge"
sandbox = false
telemetry = true

mcp_response_tokens_threshold = 20000
max_session_tokens_threshold = 200000
cache_keepalive_enabled = false
cache_keepalive_max_idle_seconds = 1800

enable_markdown_rendering = true
markdown_theme = "default"
max_session_spending_threshold = 0.0
max_request_spending_threshold = 0.0
auto_capabilities = true
```

The `default` value is the tag used when `octomind run` receives no tag. `assistant:concierge` comes from the built-in `muvon/tap`; it is distinct from the local `assistant` role in `[[roles]]`.

Use `octomind config --list-themes` to list accepted markdown themes. For every root field and validation rule, see the [Configuration Reference](../reference/03-config-reference.md).

## Model Profiles and Purposes

Octomind has exactly three request purposes:

| Purpose | Configuration | Used for |
|---------|---------------|----------|
| `main` | `[model]` | The active session conversation and its cache keepalive |
| `supervisor` | `[supervisor.model]` | Gate, resolution, planning, condensation, and learning calls |
| `compression` | `[compression.model]` | Conversation-compression decisions and summaries |

The main profile is complete and is the inheritance baseline:

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

`[supervisor.model]`, `[compression.model]`, and `[roles.model]` accept the same fields as partial overrides; omitted values inherit from `[model]`. The shipped template gives supervisor and compression their own complete profiles, both named `octohub:auto`.

Model names must use `provider:model`. For the interactive CLI, model-name precedence is:

```text
runtime override > active role profile > tap model mapping > main [model]
```

For example, `octomind run -m 'openai:gpt-5.6-sol'` overrides the selected model for that session, while a `[roles.model]` table can override any subset of the profile for one role.

## Roles and Tags

A plain tag selects a local role. A tag containing `:` resolves an agent manifest from taps and merges it into the loaded configuration.

```toml
[[roles]]
name = "assistant"
system = "You are a helpful assistant. Working directory: {{CWD}}"
welcome = "Ready in {{CWD}} as {{ROLE}}."

[roles.model]
reasoning_effort = "high"

[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
```

An empty `allowed_tools` list means no tool restriction within the referenced servers. See [Roles](06-roles.md) for role inheritance and permissions.

## MCP Servers

The default registry declares four built-in MCP servers:

| Server | Tool group |
|--------|------------|
| `core` | Core session tools, including conditional recall |
| `orchestration` | Tap, schedule, and monitor tools |
| `runtime` | MCP, agent, skill, and capability management |
| `agent` | Tools generated from configured `[[agents]]` entries |

External servers use `http` or `stdio`:

```toml
[[mcp.servers]]
name = "project_tools"
type = "stdio"
command = "project-tools"
args = ["mcp"]
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "remote_tools"
type = "http"
url = "https://example.invalid/mcp"
headers = { Authorization = "Bearer {{ENV:REMOTE_MCP_TOKEN}}" }
timeout_seconds = 30
tools = []
```

HTTP header values support `{{ENV:KEY}}`. Without an explicit `Authorization` header, the HTTP client can use MCP authorization discovery. See [MCP Tools](07-mcp-tools.md) for the tool surface and [Config Reference](../reference/03-config-reference.md#mcp) for all server fields.

## Multi-File Configuration

Octomind merges every `*.toml` file in the selected config directory:

1. `config.toml` loads first.
2. Other regular TOML files load alphabetically.
3. Files named `mcp-*.toml` load last; `mcp.toml` is a regular file.
4. Tables merge recursively and later scalar values replace earlier ones.
5. Arrays of tables are concatenated; entries with the same `name` are deduplicated with the last entry kept.
6. Other arrays are replaced by the later value.

This makes a file such as `mcp-project.toml` an explicit override for a same-named server declared earlier.

## Tap and Capability Overrides

`[capabilities]` selects a provider file inside a tap. When a capability has no override, its provider name is `default`:

```toml
[capabilities]
codesearch = "octocode"
```

`[taps]` changes only the model name for a tap tag; the rest of the main model profile remains inherited until an active role profile overrides it:

```toml
[taps]
"developer:general" = "ollama:glm-5.3"
```

## Project Instructions and Template Variables

When `AGENTS.md` exists in the working directory, Octomind loads its non-empty contents into a new session as project instructions and expands the same placeholders used by role `system` and `welcome` text.

| Placeholder | Value |
|-------------|-------|
| `{{CWD}}` | Current working directory |
| `{{ROLE}}` | Active role, or `unknown` when no role was supplied to expansion |
| `{{DATE}}` | Current date and timezone |
| `{{SHELL}}` | Current shell information |
| `{{OS}}` | Operating-system information |
| `{{BINARIES}}` | Detected development tools |
| `{{GIT_STATUS}}` | Git status, or an empty string when unavailable |
| `{{GIT_TREE}}` | Project file tree, or an empty string when unavailable |
| `{{README}}` | Root README content, or an empty string when unavailable |
| `{{SYSTEM}}` | Combined shell, OS, directory, and tool information |
| `{{CONTEXT}}` | Combined README, Git status, and file-tree context |

Inspect context values with:

```bash
octomind vars            # list names
octomind vars --preview  # preview values (-p)
octomind vars --expand   # print full values (-e)
```

`octomind vars` additionally reports `{{HOME}}`, but `{{HOME}}` is not expanded in role prompts or `AGENTS.md`. Conversely, role prompt expansion supports `{{ROLE}}`, while the standalone `vars` command has no active role value to list.

## Further Reading

- [Configuration Reference](../reference/03-config-reference.md) — complete field reference
- [Environment Variables](../reference/04-environment-variables.md) — runtime and credential variables
- [AI Providers](04-providers.md) — model gateways and provider setup
- [Compression](08-compression.md) — compression behavior and configuration
- [Supervisor](14-supervisor.md) — supervisor behavior and configuration
- [Workflows](09-workflows.md) — external workflow configuration
