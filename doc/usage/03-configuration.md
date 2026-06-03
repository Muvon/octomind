# Configuration

Octomind uses TOML configuration files stored in a platform-specific data directory.

This page covers the config file format, where it lives, and how settings resolve. Deeper topics live in their own docs: [Roles and Permissions](06-roles.md), [Compression](08-compression.md), [Providers](04-providers.md), and [Learning](13-learning.md).

## File Locations

| Platform | Data Directory | Config File |
|----------|---------------|-------------|
| macOS | `~/.local/share/octomind/` | `~/.local/share/octomind/config/config.toml` |
| Linux | `~/.local/share/octomind/` | `~/.local/share/octomind/config/config.toml` |
| Windows | `%LOCALAPPDATA%/octomind/` | `%LOCALAPPDATA%/octomind/config/config.toml` |

Full directory structure:
```
~/.local/share/octomind/
  config/
    config.toml           # Main configuration
    *.toml                # Additional config files (merged)
  sessions/               # Saved sessions
  logs/                   # Debug and error logs
  cache/                  # Cached data
  run/                    # Per-session Unix sockets and PID files (used by `send` / --daemon)
  learning/               # Cross-session adaptive learning (lessons), scoped by project/role
  agents/                 # Cached tap agent manifests (<category>/<variant>.toml)
```

Override the config location with the `OCTOMIND_CONFIG_PATH` environment variable. It points to a config **file**; its parent directory becomes the merge directory (all `*.toml` files there are loaded — see [Multi-File Configuration](#multi-file-configuration)).

## Getting Started

Generate a default configuration:

```bash
octomind config
```

On first run this writes `config.toml` to `~/.local/share/octomind/config/`. The template is **embedded in the binary** at build time (the repo's `config-templates/default.toml` is the source of truth — it is not a file on your machine). After first launch, the on-disk file is authoritative: edits you make there are what Octomind loads.

Verify and maintain your configuration:

```bash
octomind config --show      # Display the effective (merged) settings
octomind config --validate  # Check for errors
octomind config --upgrade   # Migrate an old config to the current version
```

`--upgrade` migrates an existing config to the latest version and writes a backup to `config.toml.backup`. Upgrades also run automatically on load whenever the file's `version` is older than the current one.

## How Settings Resolve

Two separate mechanisms decide the effective configuration: how the **files** merge, and how the **model** is chosen.

**File merge (last wins).** All `*.toml` files in the config directory are merged into one config. `config.toml` loads first, then the rest alphabetically, with `mcp-*.toml` files loaded **last** as overrides (see [Multi-File Configuration](#multi-file-configuration)). When two files set the same scalar, the later file wins; arrays of tables (`[[mcp.servers]]`, `[[roles]]`) are concatenated and same-name entries are deduplicated keeping the last occurrence. There is no separate runtime "defaults" tier — the embedded template is copied to disk once on first run, after which the on-disk file *is* the config.

**Model selection (precedence chain).** The model is the one field with a real precedence order:

```
CLI --model  >  role.model  >  config.model (root)
```

A plain `[[roles]]` entry's `model` is honored directly — `octomind run <role>` uses it over the root `config.model` (CLI `--model` still wins). For a tap agent (`category:variant`), a `[taps]` entry for that tag overrides the `config.model` tier, so it applies only when neither `--model` nor the agent's own role sets a model. See [Tap Model Overrides](#tap-model-overrides). (Resolution happens in `src/session/chat/session/core.rs`: `CLI --model ?? role.model ?? config.model`.)

## Core Settings

```toml
# Config version (do not modify)
version = 1

# Logging: "none", "info", "debug"
log_level = "info"

# Default model (provider:model format)
model = "openrouter:anthropic/claude-sonnet-4"

# Default tag when no TAG passed to `octomind run`
default = "assistant:concierge"

# Global max tokens
max_tokens = 16384

# Reasoning effort hint for thinking-capable models (ignored by others)
reasoning_effort = "medium"  # low | medium | high | xhigh | max

# Sandbox mode: restrict writes to working directory
sandbox = false
```

The default tag `assistant:concierge` is a **tap agent** (`category:variant`) provided by the built-in default tap `muvon/tap`, not the local `[[roles]]` `assistant` definition.

`reasoning_effort` is a system-wide hint mapped by each provider to its native thinking knob (effort string, budget tokens, etc.); models without thinking support silently ignore it. You can also change it per-session at runtime with the `/effort` command, which persists the choice in the session file.

## Custom Instructions

Octomind auto-loads two project-level files:

| File | Default | Behavior |
|------|---------|----------|
| `INSTRUCTIONS.md` | `custom_instructions_file_name` | Loaded as user message in new sessions |
| `CONSTRAINTS.md` | `custom_constraints_file_name` | Appended to each request in `<constraints>` tags |

Set to empty string to disable:
```toml
custom_instructions_file_name = ""
custom_constraints_file_name = ""
```

## Performance Settings

```toml
# Hard limit on MCP response tokens (0 = unlimited)
mcp_response_tokens_threshold = 20000

# Max tokens per session before truncation (0 = disabled)
max_session_tokens_threshold = 200000

# Prompt-cache keepalive (Anthropic-only, opt-in): ping the provider while the
# session idles so the next turn still hits the cache. Each ping costs cache-read tokens.
cache_keepalive_enabled = false
cache_keepalive_max_idle_seconds = 1800  # 0 = ping until session ends

# Automatically activate capabilities whose triggers match the user message
auto_capabilities = true

# Retry configuration
max_retries = 1
retry_timeout = 30

# Per-request HTTP timeout (0 = no timeout)
request_timeout_seconds = 300
```

Cache keepalive only applies to providers whose API supports refresh-on-read (today, Anthropic). Other providers are skipped, so enabling it does no harm but has no effect for them. Set `auto_capabilities = false` to require explicit `capability(action="enable")` calls instead of automatic matching.

**Validation limits** (`octomind config --validate` enforces these):

- `max_session_tokens_threshold` <= 2,000,000
- `cache_keepalive_max_idle_seconds` <= 86400 (24h), or 0 for unbounded
- MCP server and webhook hook timeouts must be > 0 and <= 3600 seconds
- `model` and `markdown_theme` must be non-empty; role `temperature` 0.0-2.0, `top_p` 0.0-1.0, `top_k` 1-1000

## User Interface

```toml
# Markdown rendering for AI responses
enable_markdown_rendering = true

# Theme: default, dark, light, ocean, solarized, monokai
markdown_theme = "default"

# Spending limits in USD (0.0 = no limit)
max_session_spending_threshold = 0.0
max_request_spending_threshold = 0.0
```

List available themes: `octomind config --list-themes`

## MCP Servers

Configure MCP tool servers in the `[mcp]` section:

```toml
[mcp]
allowed_tools = []  # Global restrictions (empty = none)

# Built-in servers (always available)
[[mcp.servers]]
name = "core"
type = "builtin"
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "runtime"
type = "builtin"
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "agent"
type = "builtin"
timeout_seconds = 30
tools = []

# External stdio server
[[mcp.servers]]
name = "octocode"
type = "stdio"
command = "octocode"
args = ["mcp", "--path=."]
timeout_seconds = 240
tools = []

# External HTTP server
[[mcp.servers]]
name = "github_mcp"
type = "http"
url = "https://api.github.com/mcp"
timeout_seconds = 30
tools = []
```

The three built-in servers shipped in the default config are `core` (hosts `plan`, `tap`), `runtime` (hosts `mcp`, `agent`, `skill`, `schedule`, `capability`), and `agent`. Omitting `runtime` would lose all of its tools — keep it in the list.

See [MCP Tools Reference](07-mcp-tools.md) for complete tool documentation.

## Roles

Define roles in `[[roles]]` sections:

```toml
[[roles]]
name = "assistant"
temperature = 0.3
top_p = 0.7
top_k = 20
system = "You are a helpful assistant. Working directory: {{CWD}}"
welcome = "Hello! Working in {{CWD}}"

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

See [Roles and Permissions](06-roles.md) for detailed role configuration.

## Multi-File Configuration

All `*.toml` files in the config directory are merged:

1. `config.toml` loaded first
2. Other files loaded alphabetically
3. Files matching `mcp-*.toml` are loaded **last** (as overrides), regardless of alphabetical order, so they win on same-name `[[mcp.servers]]` entries (e.g. to add `auto_bind` to a server defined earlier). Note: `mcp.toml` (no dash) is a regular file loaded in normal alphabetical order.
4. Array entries (`[[mcp.servers]]`, `[[roles]]`, etc.) are concatenated
5. Same-name entries are deduplicated (last wins)
6. Scalar values are overridden by later files

This lets you organize by concern:
```
config/
  config.toml          # Core settings
  mcp-github.toml      # GitHub MCP server
  roles-custom.toml    # Project-specific roles
```

## Capability Overrides

For tap agents, override which provider handles specific capabilities:

```toml
[capabilities]
codesearch = "octocode"
```

Each key is a capability name and the value is the provider to use. It resolves to `capabilities/<capability>/<provider>.toml` within the tap — so the example maps to `capabilities/codesearch/octocode.toml`. When no override is given for a capability, the provider defaults to `default` (i.e. `capabilities/<capability>/default.toml`).

## Tap Model Overrides

```toml
[taps]
"developer:general" = "ollama:glm-5"
"octomind:assistant" = "openai:gpt-4o"
```

**Model resolution priority:**
1. CLI `--model` flag
2. The active role's `model` field (a plain `[[roles]]` entry, or a tap agent's manifest role)
3. Global `model` in config — which a `[taps]` entry overrides for a tap agent's tag

A plain `[[roles]]` entry's `model` is honored directly. The `[taps]` override only applies to tap agents (tags with `:` like `developer:general`) and acts at the `config.model` tier — it takes effect when the agent's own role does not set a model.

## Template Variables

System prompts and welcome messages support variables:

| Variable | Description |
|----------|-------------|
| `{{CWD}}` | Current working directory |
| `{{ROLE}}` | Active role name (`unknown` when no role is set) |
| `{{DATE}}` | Current date (with timezone) |
| `{{SHELL}}` | User's shell |
| `{{BINARIES}}` | Available binaries in PATH |
| `{{OS}}` | Operating system |
| `{{HOME}}` | User's home directory path |
| `{{GIT_STATUS}}` | Git status (empty outside a git repo) |
| `{{GIT_TREE}}` | Project file tree (empty outside a git repo) |
| `{{README}}` | Project README.md contents (empty if absent) |
| `{{SYSTEM}}` | Combined system info block: date, shell, OS, binaries, CWD |
| `{{CONTEXT}}` | Combined project context block: README + git status + git tree (empty outside a git repo) |

`{{SYSTEM}}` and `{{CONTEXT}}` are the composites the default `task_refiner`/`task_researcher`/`reduce` roles rely on. The git/README variables (and `{{CONTEXT}}`) resolve to an **empty string** when the project has no git repo or no README, so prompts that use them stay valid either way.

Inspect actual values with the `vars` command at three verbosity levels:

```bash
octomind vars            # list variable names
octomind vars --preview  # 3-line preview of each value (-p)
octomind vars --expand   # full expanded values (-e)
```

## Further Reading

- [Configuration Reference](../reference/03-config-reference.md) -- every config field documented
- [Environment Variables](../reference/04-environment-variables.md) -- API keys and overrides
- [Providers](04-providers.md) -- AI provider setup
- [Compression](08-compression.md) -- compression configuration
- [Workflows](09-workflows.md) -- workflow configuration
