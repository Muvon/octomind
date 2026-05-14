# Configuration

Octomind uses TOML configuration files stored in a platform-specific data directory.

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
  sessions/               # Session history
  logs/                   # Debug and error logs
  cache/                  # Cached data
  run/                    # Unix sockets and PID files
  taps/                   # Registry taps
```

Override config directory with `OCTOMIND_CONFIG_PATH` environment variable.

## Getting Started

Generate a default configuration:

```bash
octomind config
```

This creates `config.toml` from the built-in template (`config-templates/default.toml`).

Verify your configuration:

```bash
octomind config --show      # Display current settings
octomind config --validate  # Check for errors
```

## Configuration Hierarchy

Settings are resolved in order (first match wins):

```
Environment Variables (API keys, OCTOMIND_CONFIG_PATH)
    |
Role-specific config ([[roles]])
    |
Global config (root level, [mcp])
    |
Template defaults (config-templates/default.toml)
```

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

# Sandbox mode: restrict writes to working directory
sandbox = false
```

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
# Warn when MCP response exceeds this token count (0 = disable)
mcp_response_warning_threshold = 0

# Hard limit on MCP response tokens (0 = unlimited)
mcp_response_tokens_threshold = 20000

# Max tokens per session before truncation (0 = disabled)
max_session_tokens_threshold = 200000

# Cache responses exceeding this token count (0 = no caching)
cache_tokens_threshold = 2048

# Cache lifetime in seconds
cache_timeout_seconds = 240

# Use longer cache lifetime for system messages
use_long_system_cache = true

# Retry configuration
max_retries = 1
retry_timeout = 30

# Per-request HTTP timeout (0 = no timeout)
request_timeout_seconds = 300
```

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

# Built-in servers
[[mcp.servers]]
name = "core"
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
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]
```

See [Roles and Permissions](06-roles.md) for detailed role configuration.

## Multi-File Configuration

All `*.toml` files in the config directory are merged:

1. `config.toml` loaded first
2. Other files loaded alphabetically
3. Array entries (`[[mcp.servers]]`, `[[roles]]`, etc.) are concatenated
4. Same-name entries are deduplicated (last wins)
5. Scalar values are overridden by later files

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

This maps to `capabilities/codesearch/octocode.toml` within the tap.

## Tap Model Overrides

```toml
[taps]
"developer:general" = "ollama:glm-5"
"octomind:assistant" = "openai:gpt-4o"
```

**Model resolution priority:**
1. CLI `--model` flag
2. `[taps]` override (for tap agents only)
3. Role's `model` field
4. Global `model` in config

Only applies to tap agents (tags with `:` like `developer:general`). Plain role names use role.model or config.model.
## Template Variables

System prompts and welcome messages support variables:

| Variable | Description |
|----------|-------------|
| `{{CWD}}` | Current working directory |
| `{{ROLE}}` | Active role name |
| `{{DATE}}` | Current date |
| `{{SHELL}}` | User's shell |
| `{{OS}}` | Operating system |
| `{{GIT_STATUS}}` | Git status |
| `{{README}}` | Project README.md contents |

Use `octomind vars` to see all current values.

## Further Reading

- [Configuration Reference](../reference/03-config-reference.md) -- every config field documented
- [Environment Variables](../reference/04-environment-variables.md) -- API keys and overrides
- [Providers](04-providers.md) -- AI provider setup
- [Compression](08-compression.md) -- compression configuration
- [Workflows](09-workflows.md) -- workflow configuration
