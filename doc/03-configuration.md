# Configuration Guide

## Overview

Octomind uses a hierarchical configuration system that allows for flexible customization while providing sensible defaults. Configuration is stored in system-wide directories and supports role-specific overrides with inheritance patterns.

**Configuration Location:**
- **macOS/Linux**: `~/.config/octomind/config.toml`
- **Windows**: `%APPDATA%/octomind/config.toml`

**Multi-File Configuration Support:**
Octomind supports loading multiple configuration files from the config directory. All `.toml` files in `~/.config/octomind/` are merged together, allowing you to organize configuration into separate files (e.g., `roles.toml`, `layers.toml`, `workflows.toml`).

## Configuration Hierarchy

The configuration system follows a template-based approach with environment variable overrides:

1. **Template Defaults** (`config-templates/default.toml`) - All default values and structure
2. **Multi-File Configuration** - All `.toml` files in config directory are merged
3. **Environment Variables** - Override any setting with `OCTOMIND_*` prefix
4. **User Configuration** - Optional user config file for persistent customization

### Configuration Principles

- **Template-Based**: All defaults defined in `config-templates/default.toml`
- **Environment Override**: Any setting can be overridden with `OCTOMIND_*` variables
- **Role-Based**: Developer (full tools), Assistant (chat-only), and custom roles
- **Security First**: API keys are ONLY set via environment variables

### Role Configuration

Octomind uses role-based configuration with built-in roles and custom role support:

**Built-in Roles:**
- **Developer Role**: Full MCP tool access, optimized for development tasks
- **Assistant Role**: Chat-only mode with limited tool access for general assistance

**Custom Roles**: Define specific tool permissions, models, and configurations in the template

**Environment Overrides**: Use `OCTOMIND_ROLES__ROLENAME__SETTING` format for role-specific overrides

## Configuration System

### Template-Based Configuration

Octomind uses a template-based configuration system where all defaults are defined in `config-templates/default.toml`. This ensures:

- **No Hardcoded Values**: All settings are configurable
- **Consistent Defaults**: Same defaults across all installations
- **Easy Customization**: Override any setting via environment variables
- **Version Control**: Template changes are tracked and documented

### Environment Variable Overrides

Any setting in the template can be overridden using environment variables with the `OCTOMIND_` prefix:

```bash
# System-wide settings
export OCTOMIND_LOG_LEVEL="debug"
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_MAX_TOKENS="8192"

# Role-specific overrides (use double underscores for nested settings)
export OCTOMIND_ROLES__DEVELOPER__MODEL="openai:gpt-4o"
export OCTOMIND_ROLES__DEVELOPER__TEMPERATURE="0.1"

# Layer overrides
export OCTOMIND_LAYERS__TASK_REFINER__MODEL="openrouter:anthropic/claude-haiku"

# MCP server overrides
export OCTOMIND_MCP__SERVERS__OCTOCODE__TIMEOUT_SECONDS="300"
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
mcp_response_warning_threshold = 20000
max_session_tokens_threshold = 20000
cache_tokens_threshold = 2048
cache_timeout_seconds = 240
use_long_system_cache = true

# ═══════════════════════════════════════════════════════════════════════════════
# ROLE CONFIGURATIONS
# ═══════════════════════════════════════════════════════════════════════════════

# Developer role - full development environment
[developer]
enable_layers = true
layer_refs = []
system = """You are an Octomind – top notch fully autonomous AI developer..."""

# MCP configuration for developer role
[developer.mcp]
server_refs = ["developer", "filesystem", "web", "octocode"]
allowed_tools = []

# Assistant role - optimized for general assistance
[assistant]
enable_layers = false
layer_refs = []
system = "You are a helpful assistant."

# MCP configuration for assistant role
[assistant.mcp]
server_refs = ["filesystem"]
allowed_tools = []

# ═══════════════════════════════════════════════════════════════════════════════
# MCP (MODEL CONTEXT PROTOCOL) SERVERS
# ═══════════════════════════════════════════════════════════════════════════════

[mcp]
allowed_tools = []

# Built-in MCP servers
[[mcp.servers]]
name = "developer"
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

[[mcp.servers]]
name = "web"
type = "builtin"
timeout_seconds = 30
args = []
tools = []

[[mcp.servers]]
name = "octocode"
type = "stdin"
command = "octocode"
args = ["mcp", "--path=."]
timeout_seconds = 240
tools = []

# Example external MCP server configuration:
# [[mcp.servers]]
# name = "web_search"
# type = "http"
# url = "https://mcp.so/server/webSearch-Tools"
# timeout_seconds = 30
# tools = []
```

**Important Notes:**
- **API Keys**: Set via environment variables only (e.g., `OPENROUTER_API_KEY`)
- **Server References**: Roles use `server_refs` to reference servers by name
- **Tool Filtering**: Use `allowed_tools` to limit available tools per role
- **Builtin Servers**: Developer, filesystem, web, and octocode are always available

## Custom Instructions File

Octomind supports automatic loading of custom instructions from a project-specific file. This feature allows you to provide context, guidelines, or project-specific information that will be automatically included in every new session.

### Configuration

```toml
# Custom instructions file name (relative to project root)
# This file will be automatically loaded as a user message in new sessions
# Set to empty string to disable: custom_instructions_file_name = ""
custom_instructions_file_name = "INSTRUCTIONS.md"
```

### How It Works

1. **Automatic Loading**: When starting a new session, Octomind checks for the configured file in the current working directory
2. **Template Variables**: The file content supports all template variables (e.g., `%{ROLE}`, `%{CWD}`, `%{DATE}`)
3. **Session Integration**: Content is added as a user message after the welcome message
4. **Caching**: Instructions are automatically cached for token efficiency
5. **Optional**: Can be disabled by setting the filename to an empty string

### Example INSTRUCTIONS.md

```markdown
# Project: %{PROJECT_NAME}
Working Directory: %{CWD}
Current Role: %{ROLE}
Date: %{DATE}

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

### Template Variables Available

All standard template variables are supported in custom instructions:
- `%{ROLE}` - Current role (developer, assistant, etc.)
- `%{CWD}` - Current working directory
- `%{DATE}` - Current date and time
- `%{SYSTEM}` - System information
- `%{CONTEXT}` - Additional context if available

### Best Practices

1. **Project-Specific**: Include information specific to your project's architecture and conventions
2. **Role-Aware**: Use `%{ROLE}` to provide role-specific guidance
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
[developer.config]
model = "openrouter:anthropic/claude-sonnet-4"

[assistant.config]
model = "openai:gpt-4o-mini"

[my-custom-role.config]
model = "amazon:claude-sonnet-4"  # Using Amazon Bedrock
# or
model = "cloudflare:llama-3.1-8b-instruct"  # Using Cloudflare Workers AI
```

### Supported Providers

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
export JINA_API_KEY="your_jina_key"
```

### Configuration Overrides

Environment variables are the PRIMARY method of configuration:

```bash
# 🔧 Global Configuration Overrides
export OCTOMIND_LOG_LEVEL="debug"
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_CUSTOM_INSTRUCTIONS_FILE_NAME="PROJECT_GUIDE.md"
export OCTOMIND_EMBEDDING_PROVIDER="jina"

# 🛠️ Role-Specific Overrides
export OCTOMIND_DEVELOPER_ENABLE_LAYERS="true"
export OCTOMIND_ASSISTANT_ENABLE_LAYERS="false"
```

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

### Developer Role

Developer role is designed for full development assistance and inherits from global MCP configuration:

```toml
# Global MCP configuration
[mcp]
enabled = true

[[mcp.servers]]
enabled = true
name = "developer"
type = "builtin"

[[mcp.servers]]
enabled = true
name = "filesystem"
type = "builtin"

[[mcp.servers]]
enabled = true
name = "web"
type = "builtin"

# Developer role (inherits global MCP automatically)
[developer]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true
system = "You are an Octomind AI developer assistant with full access to development tools."
```

### Assistant Role

Assistant role is optimized for simple conversations with tools disabled:

```toml
[assistant]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false
system = "You are a helpful assistant."

[assistant.mcp]
enabled = false  # Override global MCP to disable tools
```

### Custom Roles

Create specialized roles for specific use cases. Custom roles inherit from assistant role first, then apply their own overrides:

```toml
[code-reviewer]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true
system = "You are a code review expert focused on security and best practices."

[code-reviewer.mcp]
enabled = true

[[code-reviewer.mcp.servers]]
enabled = true
name = "developer"
type = "builtin"
tools = ["text_editor", "shell"]  # Limited tool set
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
allowed_tools = ["list_files", "semantic_search", "view_signatures"]
```

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
allowed_tools = ["text_editor", "list_files"]

[[layers]]
name = "code_optimizer"
description = "Optimizes code for performance and maintainability"
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.2
input_mode = "all"
output_mode = "append"

[layers.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "shell"]
```

### Agent Configuration

Agents use the same `LayerConfig` structure as commands and layers. Each agent becomes a separate MCP tool (e.g., `agent_context_gatherer`):

```toml
[[agents]]
name = "context_gatherer"
description = "Gather detailed context from files and codebase. Reads files, searches code patterns, and provides comprehensive information about specific areas of the codebase for development tasks."
model = "openrouter:google/gemini-2.5-flash-preview"
max_tokens = 16384
system_prompt = """You are a comprehensive context gatherer and code analyst..."""
temperature = 0.2
input_mode = "last"
output_mode = "none"  # Return only the gathered context (cleanest for tool use)

[agents.mcp]
server_refs = ["filesystem", "octocode"]
allowed_tools = ["text_editor", "list_files", "semantic_search", "view_signatures"]

[[agents]]
name = "code_reviewer"
description = "Review code for performance, security, and best practices issues. Analyzes code quality and suggests improvements."
model = "openrouter:anthropic/claude-sonnet-4"
max_tokens = 8192
system_prompt = "You are a senior code reviewer..."
temperature = 0.1
input_mode = "last"
output_mode = "none"  # Return only the review results

[agents.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "list_files"]
```

**Key Features:**
- **Unified Configuration**: Same structure as layers and commands
- **Required Description**: Used as MCP function description
- **Output Control**: `output_mode` controls what the agent tool returns
- **MCP Integration**: Full access to development tools via MCP configuration

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

### New Server Registry Configuration

The MCP system has been significantly improved with a new server registry approach that eliminates configuration duplication. Servers are now defined once in a central registry and referenced by roles and commands:

```toml
# MCP Server Configuration - Define servers in main MCP section
[mcp]
allowed_tools = []

# Built-in servers (always available)
[[mcp.servers]]
name = "developer"
type = "builtin"
timeout_seconds = 30
args = []
tools = []  # Empty means all tools enabled

[[mcp.servers]]
name = "filesystem"
type = "builtin"
timeout_seconds = 30
args = []
tools = []  # Empty means all tools enabled

# External HTTP server
[[mcp.servers]]
name = "web_search"
type = "http"
url = "https://mcp.so/server/webSearch-Tools"
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
server_refs = ["developer", "filesystem"]  # Reference servers by name
allowed_tools = []  # Empty means all tools from referenced servers

# Role-specific override with limited servers
[assistant.mcp]
enabled = true
server_refs = ["filesystem"]  # Only filesystem tools
allowed_tools = ["text_editor", "list_files"]  # Limit to specific tools

# Global MCP fallback
[mcp]
enabled = true
server_refs = ["developer", "filesystem"]  # Default servers
```

### Server Types

- **developer**: Built-in developer tools (shell, code search, file operations)
- **filesystem**: Built-in filesystem tools (file reading, writing, listing)
- **web**: Built-in web tools (web search, HTML conversion)
- **external**: External MCP servers (HTTP or command-based)

### Migration from Legacy Configuration

The MCP configuration has evolved through several iterations. The new server registry approach is the recommended method:

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
name = "developer"
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
name = "developer"
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

[[mcp.servers]]
name = "web"
type = "builtin"
timeout_seconds = 30
args = []
tools = []

# Reference from roles
[developer.mcp]
enabled = true
server_refs = ["developer", "filesystem", "web"]
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

## Embedding Configuration

### FastEmbed (Offline)

```toml
embedding_provider = "fastembed"

[fastembed]
code_model = "all-MiniLM-L6-v2"
text_model = "all-MiniLM-L6-v2"
```

Available FastEmbed models:
- `all-MiniLM-L6-v2` (default, lightweight)
- `all-MiniLM-L12-v2` (better quality)
- `multilingual-e5-small` (multilingual support)
- `multilingual-e5-base`
- `multilingual-e5-large`

### Jina (Cloud)

```toml
embedding_provider = "jina"

[jina]
code_model = "jina-embeddings-v2-base-code"
text_model = "jina-embeddings-v3"
```

## GraphRAG Configuration

```toml
[graphrag]
enabled = true
description_model = "openrouter:openai/gpt-4.1-nano"
relationship_model = "openrouter:openai/gpt-4.1-nano"
```

## Token Management

### Smart Session Continuation System

Octomind features an intelligent session continuation system that automatically manages token limits while preserving essential context through AI-driven file analysis.

#### How It Works

When your session approaches token limits during any operation (user input, tool execution, long conversations), the system:

1. **Detects Token Threshold**: Monitors session tokens against `max_session_tokens_threshold`
2. **Requests Summary**: Automatically injects a structured summary request to the AI
3. **Parses File Requirements**: AI specifies needed files in format `filename:startline:endline`
4. **Reads File Context**: Automatically includes file contents with line numbers
5. **Resets Session**: Continues with preserved summary and full file context

#### Configuration

```toml
# Token threshold for smart continuation (0 = disabled, >0 = enabled)
max_session_tokens_threshold = 20000

# When threshold exceeded, system automatically:
# - Requests structured summary from AI
# - Parses required file contexts
# - Resets session with preserved context
```

#### Features

- **Zero Configuration**: No prompts to configure - all built-in
- **AI-Driven Context**: AI selects exactly which files and line ranges to preserve
- **Seamless Continuation**: No interruption to your workflow
- **Visual Feedback**: Clear indication when continuation occurs
- **Error Resilience**: Graceful handling of missing files or parsing errors
- **Performance Limits**: Maximum 10 file contexts, reasonable line limits

#### File Context Format

The AI specifies required files using this exact format:
```
src/config/mod.rs:95:105
src/session/chat/response.rs:264:280
```

The system automatically reads these files and includes them with 1-indexed line numbers in the continuation.

### Automatic Token Management

```toml
[openrouter]
# Warn when MCP tools generate large outputs (in tokens)
mcp_response_warning_threshold = 20000

# Smart session continuation when this limit is reached (0 = disabled)
max_session_tokens_threshold = 50000

# Automatically move cache markers when context reaches this percentage
cache_tokens_pct_threshold = 40
```

### Manual Token Management

Use session commands to manage tokens:
- `/cache` - Mark cache checkpoint
- `/info` - Show token usage breakdown
- `/done` - Optimize context

## Smart Adaptive Compression

Octomind features an intelligent compression system that automatically reduces conversation context when token usage grows, while maintaining cost-effectiveness through cache-aware decision making.

**For detailed technical information about compression, see [Advanced Features - Smart Adaptive Compression System](./06-advanced.md#smart-adaptive-compression-system).**

### How Compression Works

The compression system operates on three principles:

1. **Token-Based Triggers**: Compression activates when absolute token count exceeds configured thresholds (not pressure ratios)
2. **Cache-Aware Economics**: Before compressing, the system calculates if compression saves money considering cache invalidation costs
3. **Semantic Preservation**: Uses discourse-aware semantic chunking to preserve important information while reducing size

### Configuration

```toml
[compression]
# Enable compression hints (shows suggestions when context grows)
hints_enabled = true
# Show hints when context reaches this pressure level (0.0-1.0)
hints_pressure_threshold = 0.7
# Minimum tool executions between hints to avoid spamming
hints_min_interval = 5

# Enable adaptive token-based compression (RECOMMENDED)
adaptive_threshold = true

# Compression triggers at these token thresholds with corresponding compression ratios
# Each level defines: threshold (absolute token count) and target_ratio (compression strength)
# Compression triggers when context exceeds ANY threshold, using the highest matched ratio

[[compression.pressure_levels]]
threshold = 50000
target_ratio = 2.0  # Light: 50% reduction (compress to 1/2 size)

[[compression.pressure_levels]]
threshold = 100000
target_ratio = 4.0  # Medium: 75% reduction (compress to 1/4 size)

[[compression.pressure_levels]]
threshold = 150000
target_ratio = 8.0  # Aggressive: 87.5% reduction (compress to 1/8 size)

# Optional: Use a cheaper model for compression decisions
# Recommended: "openrouter:anthropic/claude-haiku" (10x cheaper, 3x faster)
# If not set, uses the session's main model (more expensive)
# decision_model = "openrouter:anthropic/claude-haiku"
```

### Understanding Compression Ratios

- **target_ratio = 2.0**: Compress conversation to 50% of original size
- **target_ratio = 4.0**: Compress conversation to 25% of original size
- **target_ratio = 8.0**: Compress conversation to 12.5% of original size

Higher ratios = more aggressive compression = smaller context = lower future costs

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
# Enable/disable compression
export OCTOMIND_COMPRESSION__ADAPTIVE_THRESHOLD=true

# Adjust pressure levels
export OCTOMIND_COMPRESSION__PRESSURE_LEVELS__0__THRESHOLD=60000
export OCTOMIND_COMPRESSION__PRESSURE_LEVELS__0__TARGET_RATIO=2.5

# Use cheaper model for compression decisions
export OCTOMIND_COMPRESSION__DECISION_MODEL="openrouter:anthropic/claude-haiku"

# Adjust hints
export OCTOMIND_COMPRESSION__HINTS_ENABLED=false
export OCTOMIND_COMPRESSION__HINTS_PRESSURE_THRESHOLD=0.8
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
5. **Disable if Needed**: Set `adaptive_threshold = false` to disable compression entirely

### Troubleshooting Compression

**Compression not triggering:**
- Check `adaptive_threshold = true` is set
- Verify `pressure_levels` array is not empty
- Use `/info` to see current token count vs. thresholds

**Compression too aggressive:**
- Lower the `target_ratio` values (e.g., 2.0 instead of 4.0)
- Increase the `threshold` values (e.g., 75000 instead of 50000)

**Compression not saving money:**
- Enable `decision_model` to use cheaper model for decisions
- Increase thresholds to compress less frequently
- Consider disabling compression if your sessions are short

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
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = "You are a code review expert..."
temperature = 0.1
input_mode = "all"  # Gets full conversation context

[developer.commands.review.mcp]
server_refs = ["developer", "filesystem"]  # Access to development tools
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
model = "anthropic/claude-3.5-sonnet"
```

**New format (required):**
```toml
[developer.mcp]
enabled = true

[[developer.mcp.servers]]
enabled = true
name = "developer"
type = "builtin"

[developer.config]
model = "openrouter:anthropic/claude-3.5-sonnet"
```

### Automatic Migration

Octomind automatically migrates legacy configurations on load, but it's recommended to update manually for better control.

## Troubleshooting

### Common Issues

1. **Invalid model format**
  ```
  Error: Invalid model format 'anthropic/claude-3.5-sonnet'
  Solution: Use 'openrouter:anthropic/claude-3.5-sonnet'
  ```

2. **Missing API keys**
  ```
  Warning: API key not found
  Solution: Set environment variable or update config
  ```

3. **Tool execution failures**
  ```
  Tool execution failed: Unknown tool 'list_files'
  Solution: Check MCP server configuration and tool routing
  ```

4. **Input mode configuration errors**
  ```
  Unknown input mode: 'Last'. Valid options: last, all
  Solution: Use lowercase input modes: 'last', 'all'
  ```

5. **Session continuation not working**
  ```
  Session continues growing without continuation
  Solution: Check max_session_tokens_threshold > 0 (0 = disabled)
  ```

6. **Legacy configuration fields**
  ```
  Unknown configuration field: enable_auto_truncation
  Unknown configuration field: max_request_tokens_threshold
  Solution: Update to max_session_tokens_threshold, remove enable_auto_truncation
  ```

7. **Configuration validation failed**
  ```bash
  octomind config --validate
  ```

8. **Role inheritance issues**
  ```
  Error: Custom role configuration invalid
  Solution: Ensure custom roles inherit from assistant base
  ```

9. **MCP server registry issues**
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
