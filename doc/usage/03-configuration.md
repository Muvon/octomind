# Configuration

Configure Octomind's models, tools, and runtime limits. This guide covers file locations, merge order, and overrides for
users maintaining their own TOML configuration.

## Create and Inspect the Configuration

Octomind creates the default configuration automatically when no TOML configuration exists. You can also create or
inspect it explicitly:

```bash
octomind config             # create config.toml when absent
octomind config --show      # display key effective settings
octomind config --validate  # validate the merged configuration
octomind config --upgrade   # run the current migration explicitly
```

The default template is embedded in the binary from `config-templates/default.toml`. Older configurations are upgraded
automatically during load; a migration writes a versioned backup beside the original before replacing it.

## File Locations

| Platform | Data directory | Main config file |
|----------|----------------|------------------|
| macOS | `~/.local/share/octomind/` | `~/.local/share/octomind/config/config.toml` |
| Linux | `~/.local/share/octomind/` | `~/.local/share/octomind/config/config.toml` |
| Windows | `%LOCALAPPDATA%/octomind/` | `%LOCALAPPDATA%/octomind/config/config.toml` |

The data directory also contains saved sessions, logs, cache data, taps, and learning records. Two environment variables
relocate configuration and state:

| Variable | Effect |
|----------|--------|
| `OCTOMIND_DATA_DIR` | Replaces the platform data directory for config and other persistent state |
| `OCTOMIND_CONFIG_PATH` | Selects a specific main config file; its parent becomes the multi-file merge directory |

For a separate persistent setup, set the data directory before starting Octomind:

```bash
OCTOMIND_DATA_DIR="$HOME/.local/share/octomind-demo" octomind config --show
```

To load an existing alternate configuration, use its absolute path. All TOML files alongside it also load:

```bash
OCTOMIND_CONFIG_PATH="$HOME/.config/octomind/config.toml" octomind config --validate
```

`OCTOMIND_CONFIG_PATH` does not relocate credentials: the user `.env` remains under `<data-dir>/config/`. Host-local
sockets and PID files use the system runtime/temp directory rather than `OCTOMIND_DATA_DIR`.

With a custom config path, `config --show` still labels its output with the standard config path, and
`config --upgrade` targets that standard path. Normal loading migrates the selected custom file automatically.

## Core Settings

The shipped root settings begin with the following. Edit these existing keys before any TOML table header; do not append
them beneath `[model]`. Leave `version` to the migration system.

```toml
version = 13
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

The `default` value is the tag used when `octomind run` receives no tag. `assistant:concierge` comes from the built-in
`muvon/tap`; it is distinct from the local `assistant` role in `[[roles]]`.

List themes and select one with:

```bash
octomind config --list-themes
octomind config --markdown-theme dark
```

For every root field and validation rule, see the [Configuration Reference](../reference/03-config-reference.md).

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

`[supervisor.model]`, `[compression.model]`, and `[roles.model]` accept the same fields as partial overrides; omitted
values inherit from `[model]`. The shipped template gives supervisor and compression their own complete profiles, both
named `octohub:auto`.

Model names must use `provider:model`. For the interactive CLI, model-name precedence is:

```text
runtime override > active role profile > tap model mapping > main [model]
```

For example, after configuring the provider credential:

```bash
octomind run -m 'openai:gpt-5.6-sol'
```

This changes the session model name. Supervisor and compression keep their own model profiles; configure those too when
moving away from OctoHub. See [AI Providers](04-providers.md#bring-your-own-keys).

## Roles and Tags

A plain tag selects a local `[[roles]]` entry; a tag containing `:` resolves a tap manifest. See [Roles](06-roles.md)
for complete examples, profile inheritance, and tool permissions.

## MCP Servers

The default registry declares four built-in MCP servers:

| Server | Tool group |
|--------|------------|
| `core` | `recall` when compression attention or its governance is enabled; planning is supervisor-internal |
| `orchestration` | Tap, schedule, and monitor tools |
| `runtime` | MCP, agent, skill, and capability management |
| `agent` | Tools generated from configured `[[agents]]` entries |

External servers use `http` or `stdio`. For stdio, set `PROJECT_MCP_COMMAND` to the absolute path of your installed MCP
server executable; this example assumes it speaks MCP on stdio without arguments. Add its required arguments to `args`
if needed. Export the path in the shell that will launch Octomind (replace the user-supplied path):

```bash
export PROJECT_MCP_COMMAND="/path/to/installed/mcp-server"
```

Save this definition as `mcp-project.toml` in the config directory:

```toml
[[mcp.servers]]
name = "project_tools"
type = "stdio"
command = "{{ENV:PROJECT_MCP_COMMAND}}"
args = []
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

Then start the matching role and inspect the server:

```bash
octomind run developer:general
```

```text
/mcp
```

`auto_bind` matches exact role strings: `developer` does not match `developer:general`. For a newly added reference, it
also adds a `project_tools:*` grant when the role has a nonempty allowlist. Registration alone does not enable a server;
use `auto_bind` or a role's `server_refs`. HTTP header values support `{{ENV:KEY}}`; without an explicit `Authorization`
header, the HTTP client can
use MCP authorization discovery. See [MCP Tools](07-mcp-tools.md) and [Config
Reference](../reference/03-config-reference.md#mcp) for HTTP setup.

## Multi-File Configuration

Octomind merges every `*.toml` file in the selected config directory:

1. `config.toml` loads first.
2. Other regular TOML files load alphabetically.
3. Files named `mcp-*.toml` load last; `mcp.toml` is a regular file.
4. Tables merge recursively and later scalar values replace earlier ones.
5. Arrays of tables are concatenated; entries with the same `name` are deduplicated with the last entry kept.
6. Other arrays are replaced by the later value.

This makes a file such as `mcp-project.toml` an explicit override for a same-named server declared earlier. The last
same-named array entry replaces the whole entry, so repeat all required server or role fields. Only the literal filename
`config.toml` gets first priority, even with `OCTOMIND_CONFIG_PATH` set.

For example, put this partial override in `settings.toml` alongside `config.toml`, then validate the merged result:

```toml
log_level = "debug"

[model]
reasoning_effort = "high"
```

```bash
octomind config --validate
```

## Tap and Capability Overrides

`[capabilities]` selects a provider file inside a tap. When a capability has no override, its provider name is
`default`:

```toml
[capabilities]
codesearch = "octocode"
```

`[taps]` changes only the model name for a tap tag; the rest of the main model profile remains inherited until an active
role profile overrides it:

```toml
[taps]
"developer:general" = "ollama:glm-5.3"
```

These examples require the selected capability provider file in a tap and the selected model in Ollama. Apply either
override to the existing table, then run:

```bash
octomind run developer:general
```

## Project Instructions and Template Variables

When `AGENTS.md` exists in the working directory, Octomind loads its non-empty contents into a new session as project
instructions and expands the same placeholders used by role `system` and `welcome` text.

For example, place this text in your project's `AGENTS.md`:

```text
Working directory: {{CWD}}
Explain your proposed changes before editing files. Preserve unrelated changes.
```

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

`octomind vars` additionally reports `{{HOME}}`, but `{{HOME}}` is not expanded in role prompts or `AGENTS.md`.
Conversely, role prompt expansion supports `{{ROLE}}`, while the standalone `vars` command has no active role value to
list.

## Troubleshooting

**Why is a config edit ignored?** Check later files in the merge directory, and check whether the key is beneath the
right table header. Validate after editing:

```bash
octomind config --validate
octomind config --show
```

**Why does my exported credential lose to another value?** Loading order is shell environment, user
`<data-dir>/config/.env`, then working-directory `.env`; each later file overrides earlier values. See [AI
Providers](04-providers.md#environment-file-precedence) for examples.

## See also

- [Configuration Reference](../reference/03-config-reference.md) — complete field reference
- [Environment Variables](../reference/04-environment-variables.md) — runtime and credential variables
- [AI Providers](04-providers.md) — model gateways and provider setup
- [Compression](08-compression.md) — compression behavior and configuration
- [Supervisor](14-supervisor.md) — supervisor behavior and configuration
- [Workflows](09-workflows.md) — external workflow configuration
