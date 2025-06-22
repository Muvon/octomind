# Configuration Guide

## Overview

Octomind uses a hierarchical configuration system that allows for flexible customization while providing sensible defaults. Configuration is stored in system-wide directories and supports role-specific overrides with inheritance patterns.

**Configuration Location:**
- **macOS/Linux**: `~/.local/share/octomind/config/config.toml`
- **Windows**: `%LOCALAPPDATA%/octomind/config/config.toml`

## Configuration Hierarchy

The configuration system follows a strict, hierarchical priority order:
1. Environment Variables (Highest Priority)
2. Configuration File
3. Default Template Values (Lowest Priority)

### Configuration Principles

- **Explicit Configuration**: All settings must be explicitly defined
- **No Hardcoded Defaults**: Default values are in the configuration template
- **Environment Variable Precedence**: Environment variables always override file-based settings
- **Security First**: Sensitive data like API keys are ONLY set via environment variables

### Role Configuration

Roles now use a simplified, more explicit configuration model:
- **System-Wide Model**: A single model is used across all roles
- **Explicit Role Settings**: Each role defines its own specific configuration
- **Minimal Inheritance**: Roles have minimal default settings
- **Environment Variable Overrides**: Can modify any configuration setting

## Basic Configuration

### Creating Configuration

```bash
# Create default configuration
octomind config

# Set embedding provider
octomind config --provider fastembed

# Configure with validation
octomind config --validate
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
max_request_tokens_threshold = 20000
enable_auto_truncation = false
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
model = "amazon:claude-3-5-sonnet"  # Using Amazon Bedrock
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
export OCTOMIND_MODEL="openrouter:anthropic/claude-3.5-sonnet"
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
model = "openrouter:anthropic/claude-3.5-sonnet"
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
model = "openrouter:anthropic/claude-3.5-sonnet"
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
model = "openrouter:anthropic/claude-3.5-sonnet"
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

### Automatic Token Management

```toml
[openrouter]
# Warn when MCP tools generate large outputs (in tokens)
mcp_response_warning_threshold = 20000

# Auto-truncate context when this limit is reached
max_request_tokens_threshold = 50000
enable_auto_truncation = false

# Automatically move cache markers when context reaches this percentage
cache_tokens_pct_threshold = 40
```

### Manual Token Management

Use session commands to manage tokens:
- `/cache` - Mark cache checkpoint
- `/truncate [threshold]` - Toggle auto-truncation
- `/info` - Show token usage breakdown
- `/done` - Optimize context

## Command Layers

Octomind supports command layers for specialized processing with improved input handling:

```toml
# Developer role command layers
[developer.commands.estimate]
name = "estimate"
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = "You are a project estimation expert..."
temperature = 0.2
input_mode = "last"  # Case-insensitive: "last", "all", "summary"

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
  Unknown input mode: 'Last'. Valid options: last, all, summary
  Solution: Use lowercase input modes: 'last', 'all', 'summary'
  ```

5. **Configuration validation failed**
  ```bash
  octomind config --validate
  ```

6. **Role inheritance issues**
  ```
  Error: Custom role configuration invalid
  Solution: Ensure custom roles inherit from assistant base
  ```

7. **MCP server registry issues**
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
