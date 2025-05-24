# Configuration Guide

## Overview

Octodev uses a hierarchical configuration system that allows for flexible customization while providing sensible defaults. Configuration is stored in `.octodev/config.toml` files and supports role-specific overrides with inheritance patterns.

## Configuration Hierarchy

The configuration system follows this priority order:
1. Role-specific configuration (e.g., `[developer]`, `[assistant]`, `[custom-role]`)
2. Global configuration sections
3. Environment variables
4. Default values

### Role Inheritance

Custom roles inherit from the assistant role as a base, then apply their own overrides:
- **Developer role**: Inherits from global settings with developer-specific overrides
- **Assistant role**: Base configuration for all custom roles
- **Custom roles**: Inherit from assistant, then apply custom overrides

## Basic Configuration

### Creating Configuration

```bash
# Create default configuration
octodev config

# Set embedding provider
octodev config --provider fastembed

# Configure with validation
octodev config --validate
```

### Example Configuration File

```toml
# Global embedding configuration
embedding_provider = "fastembed"

[fastembed]
code_model = "all-MiniLM-L6-v2"
text_model = "all-MiniLM-L6-v2"

[jina]
code_model = "jina-embeddings-v2-base-code"
text_model = "jina-embeddings-v3"

# GraphRAG configuration
[graphrag]
enabled = false
description_model = "openrouter:openai/gpt-4.1-nano"
relationship_model = "openrouter:openai/gpt-4.1-nano"

# Provider configurations (centralized API keys)
[providers.openrouter]
api_key = "your_openrouter_key"  # Optional, can use env var

[providers.openai]
api_key = "your_openai_key"  # Optional, can use env var

[providers.anthropic]
api_key = "your_anthropic_key"  # Optional, can use env var

[providers.google]
project_id = "your-gcp-project-id"
region = "us-central1"

[providers.amazon]
region = "us-east-1"
access_key_id = "your_access_key"  # Optional, can use env var
secret_access_key = "your_secret_key"  # Optional, can use env var

[providers.cloudflare]
account_id = "your_account_id"
api_token = "your_api_token"  # Optional, can use env var

# Legacy OpenRouter configuration (for backward compatibility)
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true
log_level = "info"
mcp_response_warning_threshold = 20000
max_request_tokens_threshold = 50000
cache_tokens_pct_threshold = 40

# Developer role configuration
[developer]
system = "You are an Octodev AI developer assistant with full access to development tools."

[developer.config]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

[developer.mcp]
enabled = true

[[developer.mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"

[[developer.mcp.servers]]
enabled = true
name = "filesystem"
server_type = "filesystem"

# Assistant role configuration (tools disabled by default)
[assistant]
system = "You are a helpful assistant."

[assistant.config]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false

[assistant.mcp]
enabled = false

# Custom role example
[my-custom-role]
system = "You are a specialized assistant for my specific use case."

[my-custom-role.config]
model = "openrouter:openai/gpt-4o"
enable_layers = true

[my-custom-role.mcp]
enabled = true

[[my-custom-role.mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"
tools = ["shell", "text_editor"]  # Limit to specific tools

# External MCP server example
[[my-custom-role.mcp.servers]]
enabled = true
name = "WebSearch"
server_type = "external"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "optional_token"
tools = []  # Empty means all tools enabled
```

## AI Provider Configuration

### Required Format

All models must use the `provider:model` format:

```toml
[developer.config]
model = "openrouter:anthropic/claude-sonnet-4"

[assistant.config]
model = "openai:gpt-4o-mini"

[my-custom-role.config]
model = "anthropic:claude-3-5-haiku"
```

### Supported Providers

- **OpenRouter**: `openrouter:provider/model`
- **OpenAI**: `openai:model-name`
- **Anthropic**: `anthropic:model-name`
- **Google Vertex AI**: `google:model-name`
- **Amazon Bedrock**: `amazon:model-name`
- **Cloudflare Workers AI**: `cloudflare:model-name`

## Environment Variables

### API Keys

```bash
# AI Provider Keys
export OPENROUTER_API_KEY="your_key"
export OPENAI_API_KEY="your_key"
export ANTHROPIC_API_KEY="your_key"

# Google Vertex AI
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
export GOOGLE_PROJECT_ID="your-project-id"
export GOOGLE_REGION="us-central1"

# Amazon Bedrock
export AWS_ACCESS_KEY_ID="your_access_key"
export AWS_SECRET_ACCESS_KEY="your_secret_key"
export AWS_REGION="us-east-1"

# Cloudflare Workers AI
export CLOUDFLARE_ACCOUNT_ID="your_account_id"
export CLOUDFLARE_API_TOKEN="your_api_token"

# Embedding Provider Keys
export JINA_API_KEY="your_jina_key"
```

### Configuration Overrides

Environment variables take precedence over configuration files:

```bash
export OCTODEV_LOG_LEVEL="debug"
export OCTODEV_EMBEDDING_PROVIDER="jina"
```

## Role-Specific Configuration

### Developer Role

Developer role is designed for full development assistance:

```toml
[developer]
system = "You are an Octodev AI developer assistant with full access to development tools."

[developer.config]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

[developer.mcp]
enabled = true

[[developer.mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"

[[developer.mcp.servers]]
enabled = true
name = "filesystem"
server_type = "filesystem"
```

### Assistant Role

Assistant role is optimized for simple conversations:

```toml
[assistant]
system = "You are a helpful assistant."

[assistant.config]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false

[assistant.mcp]
enabled = false
```

### Custom Roles

Create specialized roles for specific use cases:

```toml
[code-reviewer]
system = "You are a code review expert focused on security and best practices."

[code-reviewer.config]
model = "openrouter:anthropic/claude-3.5-sonnet"
enable_layers = true

[code-reviewer.mcp]
enabled = true

[[code-reviewer.mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"
tools = ["text_editor", "semantic_code"]  # Limited tool set
```

## Layered Architecture Configuration

### Layer-Specific Models

```toml
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

# Specific models for each layer
query_processor_model = "openrouter:openai/gpt-4.1-nano"
context_generator_model = "openrouter:google/gemini-2.5-flash-preview"
developer_model = "openrouter:anthropic/claude-sonnet-4"
reducer_model = "openrouter:openai/o4-mini"
```

### Custom Layer Configuration

```toml
[[layers]]
name = "query_processor"
enabled = true
model = "openrouter:openai/gpt-4.1-nano"
temperature = 0.1
enable_tools = false
input_mode = "Last"

[[layers]]
name = "context_generator"
enabled = true
model = "openrouter:google/gemini-2.5-flash-preview"
temperature = 0.2
enable_tools = true
allowed_tools = ["core", "text_editor"]
input_mode = "Last"

[[layers]]
name = "developer"
enabled = true
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.3
enable_tools = true
input_mode = "All"
```

## MCP Configuration

### New Server-Based Configuration

The MCP system has been refactored to use a unified server configuration approach:

```toml
# Role-specific MCP configuration
[developer.mcp]
enabled = true

# Built-in server types
[[developer.mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"  # Built-in developer tools

[[developer.mcp.servers]]
enabled = true
name = "filesystem"
server_type = "filesystem"  # Built-in filesystem tools

# External HTTP server
[[developer.mcp.servers]]
enabled = true
name = "WebSearch"
server_type = "external"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "optional_token"
tools = []  # Empty means all tools enabled

# External command-based server
[[developer.mcp.servers]]
enabled = true
name = "LocalTools"
server_type = "external"
command = "python"
args = ["-m", "my_mcp_server", "--port", "8008"]
timeout_seconds = 30
tools = []
```

### Server Types

- **developer**: Built-in developer tools (shell, code search, file operations)
- **filesystem**: Built-in filesystem tools (file reading, writing, listing)
- **external**: External MCP servers (HTTP or command-based)

### Legacy Provider Support

For backward compatibility, the old `providers` format is still supported but will be migrated:

```toml
# Legacy format (deprecated)
[mcp]
enabled = true
providers = ["core"]

# New format (recommended)
[mcp]
enabled = true

[[mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"
```

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

Octodev supports command layers for specialized processing:

```toml
# Developer role command layers
[developer.commands.estimate]
name = "estimate"
enabled = true
model = "openrouter:openai/gpt-4.1-mini"
system_prompt = "You are a project estimation expert..."
temperature = 0.2
input_mode = "Last"

[developer.commands.estimate.mcp]
enabled = false

[developer.commands.review]
name = "review"
enabled = true
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = "You are a code review expert..."
temperature = 0.1
input_mode = "All"

[developer.commands.review.mcp]
enabled = true
servers = ["developer"]
allowed_tools = ["text_editor", "semantic_code"]
```

## Validation and Security

### Configuration Validation

```bash
# Validate configuration
octodev config --validate
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
chmod 600 .octodev/config.toml
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
server_type = "developer"

[developer.config]
model = "openrouter:anthropic/claude-3.5-sonnet"
```

### Automatic Migration

Octodev automatically migrates legacy configurations on load, but it's recommended to update manually for better control.

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

3. **Configuration validation failed**
   ```bash
   octodev config --validate
   ```

4. **Role inheritance issues**
   ```
   Error: Custom role configuration invalid
   Solution: Ensure custom roles inherit from assistant base
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