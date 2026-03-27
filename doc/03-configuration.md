# Configuration Guide

## Overview

Octomind uses a hierarchical configuration system that allows for flexible customization while providing sensible defaults. Configuration is stored in system-wide directories and supports role-specific overrides with inheritance patterns.

**Configuration Location:**
- **macOS/Linux**: `~/.config/octomind/config.toml`
- **Windows**: `%APPDATA%/octomind/config.toml`

**Multi-File Configuration Support:**
Octomind supports loading multiple configuration files from the config directory. All `.toml` files in `~/.config/octomind/` are merged together, allowing you to organize configuration into separate files (e.g., `roles.toml`, `layers.toml`, `workflows.toml`).

## Configuration Hierarchy

The configuration system follows a template-based approach:

1. **Template Defaults** (`config-templates/default.toml`) - All default values and structure
2. **Multi-File Configuration** - All `.toml` files in config directory are merged
3. **User Configuration** - Optional user config file for persistent customization

**Note**: The environment variable `OCTOMIND_CONFIG_PATH` can be used to specify a custom configuration path.

## Registry Taps

Octomind uses a Homebrew-style tap system to manage agent manifests from multiple sources. Taps are Git repositories or local directories containing agent definitions.

### Tap Management

```bash
# Add a GitHub tap (clones https://github.com/user/octomind-repo)
octomind tap user/repo

# Add a local tap
octomind tap user/repo /path/to/local

# List all active taps
octomind tap

# Remove a tap
octomind untap user/repo
```

### How Taps Work

1. **Priority**: User-added taps are searched in the order they were added, with the built-in default tap (`muvon/tap`) as the last fallback.
2. **Auto-Update**: GitHub taps are automatically updated (via `git pull`) when the session starts, ensuring you always have the latest agent manifests.
3. **Storage**: Cloned taps are stored in `~/.local/share/octomind/taps/`.
4. **Manifests**: Taps are expected to have an `agents/` directory containing category subdirectories and `.toml` manifest files (e.g., `agents/coding/rust-expert.toml`).

## Placeholders and Inputs

Octomind supports dynamic placeholders in agent manifests and configuration files.

### Template Variables
Standard variables available in system prompts and instructions:
- `{{ROLE}}` - Current role name
- `{{CWD}}` - Current working directory
- `{{DATE}}` - Current date and time
- `{{SYSTEM}}` - Complete system information
- `{{CONTEXT}}` - Project context (README, git status, etc.)

### Persistent Inputs (`{{INPUT:KEY}}`)
Used for persistent credentials or settings:
1. Loads values from `~/.local/share/octomind/inputs.toml`
2. Prompts the user for missing values once, then saves them
3. Substitutes placeholders in manifests

### Environment Variables (`{{ENV:KEY}}`)
Used for environment-specific settings with `.env` fallback:
1. Uses existing environment variable if set
2. Otherwise prompts the user and persists to `./.env` in the current directory
3. Automatically loaded on subsequent runs

### Viewing Variables
```bash
# List all available variables
octomind vars

# Show expanded values
octomind vars --expand
```

- **Template-Based**: All defaults defined in `config-templates/default.toml`
- **Environment Override**: Any setting can be overridden with `OCTOMIND_*` variables
- **Role-Based**: Developer (full tools), Assistant (chat-only), and custom roles
- **Security First**: API keys are ONLY set via environment variables
- **Capabilities**: Provider overrides for tap agent capabilities (e.g., codesearch)

### Role Configuration

Octomind uses role-based configuration with built-in roles and custom role support:

**Built-in Roles:**
- **Developer Role**: Full MCP tool access (core, filesystem, agent), optimized for development tasks
- **Assistant Role**: Limited tool access (core, agent only) for general assistance

**Custom Roles**: Define specific tool permissions, models, and configurations in the template

**Environment Overrides**: Use `OCTOMIND_ROLES__ROLENAME__SETTING` format for role-specific overrides

### Layer Configuration

Layers are chained AI sub-agents that run after each response. When a layer is used as an agent tool, its name is automatically prefixed with `agent_` (e.g., a layer named `researcher` becomes the tool `agent_researcher`).

### Capabilities Configuration

Capability provider overrides allow tap agents to use alternative providers for specific capabilities:

```toml
[capabilities]
# Override codesearch capability to use octocode provider
codesearch = "octocode"

# Multiple capability overrides
memory = "octomem"
knowledge = "octoknowledge"
```

Each capability maps to a provider configuration in the tap's `capabilities/` directory.

### Configuration Path Override

Use a custom configuration file or directory:

```bash
# Use specific config file
export OCTOMIND_CONFIG_PATH="/path/to/your/config.toml"

# Use config directory (loads all .toml files)
export OCTOMIND_CONFIG_PATH="/path/to/config-directory"
```
### Configuration Management

```bash
# Generate default configuration (optional - uses template defaults)
octomind config

# View current configuration
octomind config --show

# Validate configuration
octomind config --validate

# Set specific values
octomind config --model "openrouter:anthropic/claude-sonnet-4"
octomind config --log-level debug
```
### Example Configuration File

**View Complete Template**: [`config-templates/default.toml`](../config-templates/default.toml)

```toml
# Configuration version (DO NOT MODIFY)
version = 1

# ═══════════════════════════════════════════════════════════════════════════════
# SYSTEM-WIDE SETTINGS
# ═══════════════════════════════════════════════════════════════════════════════

# Log level for system messages (none, info, debug)
log_level = "none"

# Default model for all operations (provider:model format)
model = "openrouter:anthropic/claude-sonnet-4"

# Custom instructions file name (relative to project root)
# This file will be automatically loaded as a user message in new sessions
# Set to empty string to disable: custom_instructions_file_name = ""
custom_instructions_file_name = "INSTRUCTIONS.md"

# Performance & Limits
mcp_response_warning_threshold = 10000
max_session_tokens_threshold = 0
cache_tokens_threshold = 2048
cache_timeout_seconds = 240
use_long_system_cache = true

# ═══════════════════════════════════════════════════════════════════════════════
# ROLE CONFIGURATIONS
# ═══════════════════════════════════════════════════════════════════════════════

# Developer role - full development environment
[[roles]]
name = "developer"
model = "openrouter:anthropic/claude-sonnet-4"  # Optional model override
temperature = 0.3
top_p = 0.7
top_k = 20
system = """You are an Octomind – top notch fully autonomous AI developer..."""
welcome = "Hello! Octomind ready to serve you. Working dir: {{CWD}} (Role: {{ROLE}})"

[roles.mcp]
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]

# Assistant role - optimized for general assistance
[[roles]]
name = "assistant"
temperature = 0.6
top_p = 0.8
top_k = 40
system = "You are a helpful assistant."
welcome = "Hello! Octomind ready to assist you. Working dir: {{CWD}} (Role: {{ROLE}})"

[roles.mcp]
server_refs = ["core", "agent"]
allowed_tools = ["core:*", "agent:*"]
# ═══════════════════════════════════════════════════════════════════════════════
# MCP (MODEL CONTEXT PROTOCOL) SERVERS
# ═══════════════════════════════════════════════════════════════════════════════

[mcp]
allowed_tools = []

# Built-in MCP servers (always available)
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

# External filesystem server (octofs)
# Provides: view, text_editor, batch_edit, extract_lines, shell, workdir, ast_grep
[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
args = []
timeout_seconds = 30
tools = []

# Example external MCP server configuration:
# [[mcp.servers]]
# name = "external_tools"
# type = "http"
# url = "https://mcp.so/server/custom-tools"
# timeout_seconds = 30
# tools = []
```

**Important Notes:**
- **API Keys**: Set via environment variables only (e.g., `OPENROUTER_API_KEY`)
- **Server References**: Roles use `server_refs` to reference servers by name
- **Builtin Servers**: Core and agent are always available
- **Filesystem Server**: The `filesystem` server is external (octofs stdio) and must be configured in the MCP servers list
### Custom Instructions and Constraints

Octomind supports automatic loading of custom instructions and constraints:

- **`custom_instructions_file_name`** (default: `"INSTRUCTIONS.md"`): Content loaded as a user message in new sessions
- **`custom_constraints_file_name`** (default: `"CONSTRAINTS.md"`): Content appended to each user request in `<constraints>...</constraints>` tags

Set to empty string to disable: `custom_constraints_file_name = ""`

### How It Works

1. **Automatic Loading**: When starting a new session, Octomind checks for the configured file in the current working directory
2. **Template Variables**: The file content supports all template variables (e.g., `{{ROLE}}`, `{{CWD}}`, `{{DATE}}`)
3. **Session Integration**: Content is added as a user message after the welcome message
4. **Caching**: Instructions are automatically cached for token efficiency
5. **Optional**: Can be disabled by setting the filename to an empty string

### Example INSTRUCTIONS.md

```markdown
# Project: {{PROJECT_NAME}}
Working Directory: {{CWD}}
Current Role: {{ROLE}}
Date: {{DATE}}

## Project Guidelines
- Follow the existing code patterns in this codebase
- Use the project's specific naming conventions
- Ensure all changes are backward compatible

## Architecture Notes
- This is a Rust project using the MCP protocol
- Configuration is template-based with no hardcoded defaults
- All API keys must be set via environment variables

## Development Workflow
- Use batch_edit for multiple file changes
- Check memories first before investigating
- Focus on the specific task requested
```

All standard template variables are supported in custom instructions:
- `{{ROLE}}` - Current role (developer, assistant, etc.)
- `{{CWD}}` - Current working directory
- `{{HOME}}` - User home directory
- `{{DATE}}` - Current date and time
- `{{SYSTEM}}` - System information
- `{{CONTEXT}}` - Additional context if available
### Best Practices

1. **Project-Specific**: Include information specific to your project's architecture and conventions
2. **Role-Aware**: Use `{{ROLE}}` to provide role-specific guidance
3. **Concise**: Keep instructions focused and actionable
4. **Version Control**: Include the instructions file in your repository for team consistency
5. **Regular Updates**: Keep instructions current as your project evolves

### Disabling Custom Instructions

To disable the feature entirely:

```toml
custom_instructions_file_name = ""
```

Or simply remove/rename the instructions file from your project directory.

## AI Provider Configuration

### Required Format

All models must use the `provider:model` format:

```toml
### Role-Specific Model Overrides

Roles can specify their own models, which take precedence over the global model:

```toml
[[roles]]
name = "developer"
model = "openrouter:anthropic/claude-sonnet-4"

[[roles]]
name = "assistant"
model = "openai:gpt-4o-mini"
```
- **OpenRouter**: `openrouter:provider/model` - Multi-provider access through OpenRouter
- **OpenAI**: `openai:model-name` - Direct OpenAI API access
- **Anthropic**: `anthropic:model-name` - Direct Anthropic API access
- **Google Vertex AI**: `google:model-name` - Google Cloud Vertex AI
- **Amazon Bedrock**: `amazon:model-name` - AWS Bedrock models
- **Cloudflare Workers AI**: `cloudflare:model-name` - Edge AI inference

## Environment Variables

### API Keys (REQUIRED)

```bash
# 🔐 AI Provider Keys (REQUIRED)
export OPENROUTER_API_KEY="your_openrouter_key"
export OPENAI_API_KEY="your_openai_key"
export ANTHROPIC_API_KEY="your_anthropic_key"

# 🌐 Cloud Provider Credentials
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
export AWS_ACCESS_KEY_ID="your_aws_access_key"
export AWS_SECRET_ACCESS_KEY="your_aws_secret_key"
export CLOUDFLARE_API_TOKEN="your_cloudflare_token"

# 📊 Optional Embedding Provider Keys
# (Embeddings are now handled by external MCP servers like octocode)

### Configuration Path Override

Use a custom configuration file or directory:

```bash
# Use specific config file
export OCTOMIND_CONFIG_PATH="/path/to/custom-config.toml"

# Use config directory (loads all .toml files)
export OCTOMIND_CONFIG_PATH="/path/to/config-directory"

# Works with all commands
octomind run    # Uses custom config
```

**Default locations (when OCTOMIND_CONFIG_PATH not set):**
- **macOS/Linux**: `~/.config/octomind/config.toml`
- **Windows**: `%APPDATA%/octomind/config.toml`

### Configuration Overrides

Environment variables are the PRIMARY method of configuration:

```bash
# 🔧 Global Configuration Overrides
export OCTOMIND_LOG_LEVEL="debug"
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_CUSTOM_INSTRUCTIONS_FILE_NAME="PROJECT_GUIDE.md"

# 🛠️ Role-Specific Overrides (using array index)
export OCTOMIND_ROLES__0__MODEL="openai:gpt-4o"
export OCTOMIND_ROLES__0__TEMPERATURE="0.1"

### Security Best Practices

1. 🔒 NEVER commit API keys to version control
2. 🌐 Use environment variables for ALL sensitive data
3. 🛡️ Set restrictive file permissions on config files
4. 🔍 Validate configuration before deployment

```bash
# Set secure permissions on config file
chmod 600 ~/.local/share/octomind/config/config.toml
```

### Configuration Validation

```bash
# Validate your configuration
octomind config --validate

# Show only customized values
octomind config --show-customized

# Show all default values
octomind config --show-defaults
```

## Role-Specific Configuration

Octomind uses a `[[roles]]` array for role definitions. Each role can have its own model, system prompt, and MCP tool configuration.

### Developer Role

```toml
[[roles]]
name = "developer"
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.3
system = "You are an Octomind AI developer assistant with full access to development tools."

[roles.mcp]
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]
```

### Assistant Role

```toml
[[roles]]
name = "assistant"
model = "openrouter:anthropic/claude-3.5-haiku"
system = "You are a helpful assistant."

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"] # Only allow viewing files
```

### Custom Roles

Create specialized roles for specific use cases.

```toml
[[roles]]
name = "code-reviewer"
model = "openrouter:anthropic/claude-sonnet-4"
system = "You are a code review expert focused on security and best practices."

[roles.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["core:*", "filesystem:view"]
```


## Workflow Configuration

Workflows are Octomind's brain-inspired planning system that enables complex, multi-step AI processing with validation, feedback loops, and conditional branching. See [doc/10-workflows.md](./10-workflows.md) for comprehensive documentation.

### Basic Workflow Structure

```toml
[[workflows]]
name = "simple_workflow"
description = "A simple workflow that executes a single layer"

[[workflows.steps]]
name = "analyze"
type = "once"
layer = "task_refiner"
```

### Workflow Step Types

Workflows support five control flow primitives:

1. **Once** - Execute a layer once
2. **Loop** - Repeat until exit condition or max iterations
3. **Foreach** - Iterate over parsed items
4. **Conditional** - Branch based on pattern matching
5. **Parallel** - Execute layers in parallel

### Example: Feedback Loop Workflow

```toml
[[workflows]]
name = "feedback_loop"
description = "Iterative refinement with validation"

[[workflows.steps]]
name = "refine_loop"
type = "loop"
max_iterations = 5
exit_pattern = "COMPLETE"

  [[workflows.steps.substeps]]
  name = "propose"
  type = "once"
  layer = "task_refiner"

  [[workflows.steps.substeps]]
  name = "validate"
  type = "once"
  layer = "validator"
```

For complete workflow documentation, see [doc/10-workflows.md](./10-workflows.md).

## Layered Architecture Configuration

### Layer Configuration Requirements

**Important**: All layers, commands, and agents now require a `description` field:
- **Layers**: Used for documentation and understanding layer purpose
- **Commands**: Displayed in `/help` command output
- **Agents**: Used as MCP function description for tool discovery

### Layer-Specific Models

All layers use the same GenericLayer implementation with different configurations.
Each layer supports input_mode and output_mode for flexible behavior.

```toml
[[layers]]
name = "task_refiner"
description = "Refines and clarifies user requests for better processing by subsequent layers"
model = "openrouter:openai/gpt-4.1-mini"
temperature = 0.2
input_mode = "last"
output_mode = "none"  # Intermediate layer

[layers.mcp]
server_refs = []
allowed_tools = []

[[layers]]
name = "task_researcher"
description = "Gathers information and context needed for development tasks through code analysis and research"
model = "openrouter:google/gemini-2.5-flash-preview"
temperature = 0.2
input_mode = "last"
output_mode = "append"  # Adds research findings to session

[layers.mcp]
server_refs = ["filesystem", "octocode"]
allowed_tools = ["view"]

### Custom Layer Configuration

Create layers with any combination of settings (description is required):

```toml
[[layers]]
name = "custom_analyzer"
description = "Performs specialized analysis of code patterns and architecture"
model = "openrouter:openai/gpt-4.1-mini"
temperature = 0.1
input_mode = "last"
output_mode = "append"

[layers.mcp]
server_refs = ["filesystem"]
allowed_tools = ["text_editor", "view"]
[[layers]]
name = "code_optimizer"
description = "Optimizes code for performance and maintainability"
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.2
input_mode = "all"
output_mode = "append"

[layers.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["text_editor", "shell"]
```

### Agent Configuration

Agents are defined with a minimal structure — just a `name`, `description`, and `command` pointing at any ACP-compatible server. Each agent becomes a separate MCP tool (e.g., `agent_context_gatherer`):

```toml
# Built-in context gatherer — uses the context_gatherer role
[[agents]]
name = "context_gatherer"
description = "Gather detailed context from files and codebase. Reads files, searches code patterns, and provides comprehensive information about specific areas of the codebase for development tasks."
command = "octomind acp --role context_gatherer"
workdir = "."  # Working directory for agent execution (default: current directory)

# Example: Code Reviewer Agent
# [[agents]]
# name = "code_reviewer"
# description = "Review code for quality, best practices, security issues, and performance problems."
# command = "octomind acp --role code_reviewer"
# workdir = "."
```

**Fields:**
- `name` (string, required): Unique identifier — exposed as MCP tool `agent_<name>`
- `description` (string, required): Shown as the MCP tool description to the AI
- `command` (string, required): Shell command that starts an ACP server over stdio
- `workdir` (string, optional, default `"."` ): Working directory for the subprocess

**Key Features:**
- **ACP Protocol**: Each agent call spawns a real subprocess and drives the full ACP handshake
- **Any ACP Server**: `command` can point to any ACP-compatible binary, not just Octomind
- **Required Description**: Used as MCP function description shown to the AI
- **Isolated Execution**: Each call runs in its own subprocess with its own session

### Command Configuration

Commands use the same `LayerConfig` structure and can be invoked with `/run <command_name>`:

```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
model = "openrouter:openai/o4-mini"
system_prompt = "You are a Session History Reducer..."
temperature = 0.2
input_mode = "all"
output_mode = "replace"  # Replace session content with compressed history

[commands.mcp]
server_refs = []
allowed_tools = []
```

## MCP Configuration

### Server Registry Configuration

The MCP system uses a server registry approach that eliminates configuration duplication. Servers are defined once in a central registry and referenced by roles and commands:

```toml
# MCP Server Configuration - Define servers in main MCP section
[mcp]
allowed_tools = []

# Built-in servers (always available)
[[mcp.servers]]
name = "core"
type = "builtin"
timeout_seconds = 30
args = []
tools = []  # Empty means all tools enabled

[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
args = []
timeout_seconds = 30
tools = []  # Empty means all tools enabled
# External HTTP server
[[mcp.servers]]
name = "external_tools"
type = "http"
url = "https://mcp.so/server/custom-tools"
auth_token = "optional_token"
timeout_seconds = 30
tools = []  # Empty means all tools enabled

# External command-based server
[[mcp.servers]]
name = "local_tools"
type = "stdin"
command = "python"
args = ["-m", "my_mcp_server", "--port", "8008"]
timeout_seconds = 30
tools = []

# Role configurations reference servers by name
[developer.mcp]
enabled = true
server_refs = ["core", "filesystem"]  # Reference servers by name
allowed_tools = []  # Empty means all tools from referenced servers

# Role-specific override with limited servers
[assistant.mcp]
enabled = true
server_refs = ["filesystem"]  # Only filesystem tools
allowed_tools = ["text_editor", "view"]  # Limit to specific tools
# Global MCP fallback
[mcp]
enabled = true
server_refs = ["core", "filesystem"]  # Default servers
```

### Server Types

- **external**: External MCP servers (HTTP or command-based)

### Auto-Bind Configuration

MCP servers can be configured to automatically activate for specific roles using the `auto_bind` field:

```toml
[[mcp.servers]]
name = "my-server"
type = "stdio"
command = "my-mcp-server"
auto_bind = ["developer", "reviewer"]  # Auto-activate for these roles
```

When a role is loaded, any server with that role in its `auto_bind` list will be automatically enabled:
- **During session start**: Servers with matching `auto_bind` are added to the role's server list
- **With `mcp persist`**: When persisting an enabled server, `auto_bind` is set to the current role; when persisting a disabled server, `auto_bind` is cleared

This is useful for servers that should always be available for certain roles without manual configuration in each role's `server_refs`.

### Migration from Legacy Configuration

The MCP configuration supports multiple formats. The server registry approach is the recommended method:

**Oldest format (no longer supported):**
```toml
[mcp]
enabled = true
providers = ["core"]
```

**Previous format (still supported):**
```toml
[mcp]
enabled = true

[[mcp.servers]]
enabled = true
name = "core"
type = "builtin"

[[mcp.servers]]
enabled = true
name = "filesystem"
type = "builtin"
```

**New registry format (recommended):**
```toml
# Define servers in main MCP section
[[mcp.servers]]
name = "core"
type = "builtin"
timeout_seconds = 30
args = []
tools = []

[[mcp.servers]]
name = "filesystem"
type = "builtin"
timeout_seconds = 30
args = []
tools = []

# Reference from roles
[developer.mcp]
enabled = true
server_refs = ["core", "filesystem"]
```

**Migration benefits:**
1. **Eliminates duplication** - Define servers once, reference everywhere
2. **Better organization** - Clear separation between server definitions and role configurations
3. **Easier maintenance** - Update server configuration in one place
4. **Cleaner configs** - Roles only specify which servers they need

### OAuth 2.1 + PKCE Authentication

HTTP MCP servers can be secured with OAuth 2.1 + PKCE (Proof Key for Code Exchange) authentication. This is useful for connecting to services that require OAuth authorization:

```toml
# HTTP MCP server with OAuth 2.1 + PKCE authentication
[[mcp.servers]]
name = "github_mcp"
type = "http"
url = "https://api.github.com/mcp"
timeout_seconds = 30
tools = []

# OAuth configuration (optional)
[mcp.servers.oauth]
client_id = "your-oauth-client-id"
client_secret = "your-oauth-client-secret"
authorization_url = "https://github.com/login/oauth/authorize"
token_url = "https://github.com/login/oauth/access_token"
callback_url = "http://localhost:34567/oauth/callback"
scopes = ["repo", "read:org"]
```

**OAuth Configuration Fields:**
- `client_id` - OAuth application client ID
- `client_secret` - OAuth application client secret
- `authorization_url` - Authorization endpoint URL
- `token_url` - Token endpoint URL
- `callback_url` - Local callback URL for OAuth flow (typically `http://localhost:PORT/oauth/callback`)
- `scopes` - List of OAuth scopes to request

**How It Works:**
1. When Octomind connects to the server, it initiates OAuth flow
2. User is directed to authorization URL in browser
3. After authorization, token is exchanged and stored
4. Subsequent requests use the OAuth token automatically

## Token Management

### Conversation Compression System

Octomind features an intelligent conversation compression system that automatically manages token limits by compressing older conversation exchanges while preserving recent context.

#### How It Works

When your session approaches token thresholds, the system:

1. **Detects Token Threshold**: Monitors session tokens against configured pressure levels (50k, 100k, 150k tokens)
2. **AI Decision**: AI decides whether compression is beneficial (self-reflection)
3. **Preserves Recent Context**: Last 4 turns (2 exchanges) remain uncompressed for continuity
4. **Compresses Older Exchanges**: Uses plan compression infrastructure to summarize older content
5. **Cache-Aware**: Calculates net benefit considering cache invalidation costs

#### Configuration

```toml
[compression]
# Enable compression hints (shows suggestions when context grows)
hints_enabled = true
# Show hints when context reaches this pressure level (0.0-1.0)
hints_pressure_threshold = 0.7
# Minimum tool executions between hints to avoid spamming
hints_min_interval = 5

# Compression triggers at these token thresholds with corresponding compression ratios
# Each level defines: threshold (absolute token count) and target_ratio (compression strength)
# The threshold determines WHICH compression ratio to use, exponential cooldown determines WHEN to compress

[[compression.pressure_levels]]
threshold = 50000
target_ratio = 2.0  # Light: 50% reduction (compress to 1/2 size)

[[compression.pressure_levels]]
threshold = 100000
target_ratio = 4.0  # Medium: 75% reduction (compress to 1/4 size)

[[compression.pressure_levels]]
threshold = 150000
target_ratio = 8.0  # Aggressive: 87.5% reduction (compress to 1/8 size)
# Maximum number of critical knowledge entries retained across compressions
# Each compression may extract a short snippet of critical knowledge (decisions,
# constraints, user preferences). Only the last N entries are kept.
knowledge_retention = 10

# Decision model configuration for compression decisions and summary generation
# Use a fast, cheap model like Haiku for cost savings (10x cheaper than Sonnet)
[compression.decision]
model = "anthropic:claude-haiku-4-5"
max_tokens = 16000
temperature = 0.3
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
ignore_cost = false  # When true, compression decision API cost is not tracked
```

#### Features

- **AI-Driven Decision**: AI decides when compression is beneficial
- **Cache-Aware Economics**: Only compresses when profitable (considers cache invalidation costs)
- **Context Preservation**: Last 4 turns always remain uncompressed
- **Visual Feedback**: Clear indication when compression occurs
- **Zero Configuration**: Works automatically with sensible defaults
- **Knowledge Retention**: Critical knowledge (decisions, constraints, preferences) preserved across compressions

#### Fallback Mode

When compression is disabled, the system uses `max_session_tokens_threshold` as a simple trigger (0 = disabled):

```toml
max_session_tokens_threshold = 0
```

### Manual Token Management

Use session commands to manage tokens:
- `/info` - Show token usage breakdown
- `/done` - Optimize context

### Understanding Compression Ratios

- **target_ratio = 2.0**: Compress conversation to 50% of original size
- **target_ratio = 4.0**: Compress conversation to 25% of original size
- **target_ratio = 8.0**: Compress conversation to 12.5% of original size

Higher ratios = more aggressive compression = smaller context = lower future costs

### Exponential Cooldown Mechanism

Octomind uses exponential cooldown to prevent futile compression loops during tool-call chains:

**How it works:**
- Each consecutive compression (without a user message) requires exponentially more token growth
- Formula: `required_growth = 10% × 2^n` where n = consecutive compressions
- Prevents compression when context temporarily grows then shrinks (common in tool-call sequences)

**Example sequence:**
```
1st compression at 50k tokens → requires 10% growth (55k) for next compression
2nd compression at 55k tokens → requires 20% growth (66k) for next compression
3rd compression at 66k tokens → requires 40% growth (92k) for next compression
4th+ compression → requires 80-100% growth (context must nearly double)
```

**Benefits:**
- Prevents wasteful re-compression during temporary context spikes
- Allows compression when context genuinely grows
- Automatically resets when user sends a message (new conversation turn)

### Cache-Aware Economics

Compression considers the cost of cache invalidation:

- **Cache Write Cost**: 1.25x base token cost (Anthropic 5-minute TTL standard)
- **Cache Read Cost**: 0.1x base token cost (90% savings)
- **Compression Impact**: Invalidates cache, forcing rewrite at 1.25x cost
- **Future Savings**: Smaller context = lower costs for all future turns

The system only compresses if the net benefit (future savings minus cache invalidation cost) is positive.

### Environment Variable Overrides
Override compression settings via environment variables:

```bash
# Adjust pressure levels
export OCTOMIND_COMPRESSION__PRESSURE_LEVELS__0__THRESHOLD=60000
export OCTOMIND_COMPRESSION__PRESSURE_LEVELS__0__TARGET_RATIO=2.5

# Use cheaper model for compression decisions
export OCTOMIND_COMPRESSION__DECISION_MODEL="openrouter:anthropic/claude-haiku"

# Adjust hints
export OCTOMIND_COMPRESSION__HINTS_ENABLED=false
export OCTOMIND_COMPRESSION__HINTS_PRESSURE_THRESHOLD=0.8

# Adjust knowledge retention
export OCTOMIND_COMPRESSION__KNOWLEDGE_RETENTION=15
```
### Monitoring Compression

Use the `/info` command to see compression statistics:

```
Compression Statistics:
  Total compressions: 3
  Average reduction: 72.5%
  Total tokens saved: 45,000
  Cost saved: $0.045
```

### Best Practices

1. **Start Conservative**: Begin with 50k token threshold, adjust based on your workflow
2. **Monitor Costs**: Use `/info` to track compression effectiveness
3. **Use Decision Model**: Set `decision_model` to a cheaper model for significant cost savings
4. **Preserve Context**: Compression preserves last 4 turns uncompressed for continuity
5. **Disable if Needed**: Set `max_session_tokens_threshold = 0` to disable compression fallback

### Troubleshooting Compression

**Compression not triggering:**
- Verify `pressure_levels` array is not empty
- Use `/info` to see current token count vs. thresholds
- Check that `hints_enabled = true` (default)
**Compression too aggressive:**
- Lower the `target_ratio` values (e.g., 2.0 instead of 4.0)
- Increase the `threshold` values (e.g., 75000 instead of 50000)

**Compression not saving money:**
- Enable `decision_model` to use cheaper model for decisions
- Increase thresholds to compress less frequently
- Consider disabling compression if your sessions are short

## Webhook Hooks

Octomind supports HTTP webhook listeners that pipe payloads through scripts and inject the output into the session as user messages. This enables external systems to trigger actions in your Octomind session.

### Configuration

Configure webhook hooks in your `config.toml`:

```toml
# Example: GitHub push webhook
[[hooks]]
name = "github-push"
bind = "0.0.0.0:9876"
script = "/path/to/process-github-push.sh"
timeout = 30  # seconds (default: 30)
```

### Activation

Activate hooks when starting a session:

```bash
# Start session with webhook hook active
octomind run --hook github-push --daemon --format jsonl
```

### Script Interface

Your hook script receives:

**Input:**
- `stdin`: Raw HTTP body from the webhook request
- `stdout`: Message to inject into session (if exit code 0)
- `stderr`: Error information (logged on non-zero exit)

**Environment Variables:**
- `HOOK_NAME`: Name of the hook from config
- `HOOK_METHOD`: HTTP method (GET, POST, etc.)
- `HOOK_PATH`: URL path from the request
- `HOOK_QUERY`: Query string parameters
- `HOOK_CONTENT_TYPE`: Content-Type header value
- `HOOK_SESSION`: Current session ID
- `HOOK_HEADER_*`: All HTTP headers (e.g., `HOOK_HEADER_X_GITHUB_EVENT`)

### Example Script

```bash
#!/bin/bash
# process-github-push.sh - Handle GitHub push webhook

# Parse JSON payload
REPO=$(jq -r '.repository.name' <<< "$HOOK_BODY")
BRANCH=$(jq -r '.ref' <<< "$HOOK_BODY" | sed 's|refs/heads/||')

# Output message to inject into session
echo "GitHub push to $REPO on branch $BRANCH. Check the changes and suggest improvements."

exit 0
```

### Use Cases

- **CI/CD Integration**: Trigger tests or deployments from CI webhooks
- **GitHub Integration**: React to push, PR, or issue events
- **Monitoring Alerts**: Inject alerts from monitoring systems
- **Chat Bot Integration**: Bridge external chat systems to Octomind

### Security Considerations

- Hook scripts run with the same permissions as Octomind
- Validate webhook payloads in your scripts
- Use HTTPS endpoints in production
- Consider authentication tokens for webhook sources

## Command Layers

Octomind supports command layers for specialized processing with improved input handling:

```toml
# Developer role command layers
[developer.commands.estimate]
name = "estimate"
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = "You are a project estimation expert..."
temperature = 0.2
input_mode = "last"  # Case-insensitive: "last", "all"

[developer.commands.estimate.mcp]
server_refs = []  # Reference servers from registry

[developer.commands.review]
name = "review"
model = "openrouter:anthropic/claude-sonnet-4"
system_prompt = "You are a code review expert..."
temperature = 0.1
input_mode = "all"  # Gets full conversation context

[developer.commands.review.mcp]
server_refs = ["core", "filesystem"]  # Access to development tools
allowed_tools = ["text_editor", "shell"]  # Limit to specific tools
```

### Input Mode Enhancements

Command layers now feature robust input processing:

- **Case-insensitive**: `"Last"`, `"last"`, `"LAST"` all work
- **Smart context extraction**: `"last"` mode gets the last assistant response
- **Proper session context**: Commands receive the appropriate session history
- **Error handling**: Clear error messages for invalid input modes

### Tool Execution Improvements

Command tools now use smart routing:

- **Server mapping**: Tools are automatically routed to the correct server type
- **Error prevention**: Tools no longer sent to incompatible servers
- **Clear diagnostics**: Better error messages when tool execution fails
- **Registry integration**: Uses the centralized MCP server registry

## Validation and Security

### Configuration Validation

```bash
# Validate configuration
octomind config --validate
```

Common validation checks:
- Model format validation (`provider:model`)
- API key presence (warns if missing)
- Threshold value validation
- MCP server configuration validation
- Role inheritance validation

### Security Best Practices

1. **Never commit API keys** to version control
2. **Use environment variables** for sensitive data
3. **Validate configuration** before deploying
4. **Use secure file permissions** for config files
5. **Limit tool access** in custom roles

```bash
# Secure config file permissions
chmod 600 ~/.local/share/octomind/config/config.toml
```

## Migration Guide

### From Legacy Configuration

**Old format (deprecated):**
```toml
[mcp]
enabled = true
providers = ["core"]

[openrouter]
model = "anthropic/claude-sonnet-4"
```

**New format (required):**
```toml
[developer.mcp]
enabled = true

[[developer.mcp.servers]]
enabled = true
name = "core"
type = "builtin"

[developer.config]
model = "openrouter:anthropic/claude-sonnet-4"
```

### Automatic Migration

Octomind automatically migrates legacy configurations on load, but it's recommended to update manually for better control.

## Troubleshooting

### Common Issues

1. **Invalid model format**
  ```
  Error: Invalid model format 'anthropic/claude-sonnet-4'
  Solution: Use 'openrouter:anthropic/claude-sonnet-4'
  ```

2. **Missing API keys**
  ```
  Warning: API key not found
  Solution: Set environment variable or update config
  ```

3. **Tool execution failures**
  ```
  Tool execution failed: Unknown tool 'view'
  Solution: Check MCP server configuration and tool routing
  ```

4. **Input mode configuration errors**
  ```
  Unknown input mode: 'Last'. Valid options: last, all
  Solution: Use lowercase input modes: 'last', 'all'
  ```

5. **Legacy configuration fields**
   ```
   Unknown configuration field: enable_auto_truncation
   Unknown configuration field: max_request_tokens_threshold
   Solution: Update to max_session_tokens_threshold, remove enable_auto_truncation
   ```

6. **Configuration validation failed**
  ```bash
  octomind config --validate
  ```

7. **Role inheritance issues**
  ```
  Error: Custom role configuration invalid
  Solution: Ensure custom roles inherit from assistant base
  ```

8. **MCP server registry issues**
  ```
  Failed to execute tool: No servers available to process tool
  Solution: Check server_refs and ensure servers are defined in registry
  ```

### Debug Configuration

```toml
[openrouter]
log_level = "debug"
```

This enables detailed logging for troubleshooting configuration issues.

### Configuration Examples

See the `doc/examples/` directory for complete configuration examples:
- `layer_config.toml` - Layered architecture configuration
- `command_layers_config.toml` - Command layers configuration
- `simple_commands.toml` - Basic command configuration
