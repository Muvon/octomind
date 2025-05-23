# Configuration Guide

## Overview

OctoDev uses a hierarchical configuration system that allows for flexible customization while providing sensible defaults. Configuration is stored in `.octodev/config.toml` files and supports mode-specific overrides.

## Configuration Hierarchy

The configuration system follows this priority order:
1. Mode-specific configuration (e.g., `[agent]`, `[chat]`)
2. Global configuration
3. Environment variables
4. Default values

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

# Global OpenRouter configuration
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true
enable_markdown_rendering = true
mcp_response_warning_threshold = 20000
max_request_tokens_threshold = 50000
cache_tokens_pct_threshold = 40

# GraphRAG configuration
[graphrag]
enabled = false
description_model = "openrouter:openai/gpt-4.1-nano"
relationship_model = "openrouter:openai/gpt-4.1-nano"

# Global MCP configuration
[mcp]
enabled = true
providers = ["core"]
servers = []

# Agent mode configuration (inherits global settings)
[agent]
system = "You are an Octodev AI developer assistant."
[agent.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

# Chat mode configuration (tools disabled by default)
[chat]
system = "You are a helpful assistant."
[chat.mcp]
enabled = false
[chat.openrouter]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false
```

## AI Provider Configuration

### Required Format

All models must use the `provider:model` format:

```toml
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"

[agent.openrouter]
model = "openai:gpt-4o"

[chat.openrouter]
model = "anthropic:claude-3-5-haiku"
```

### Supported Providers

- **OpenRouter**: `openrouter:provider/model`
- **OpenAI**: `openai:model-name`
- **Anthropic**: `anthropic:model-name`
- **Google Vertex AI**: `google:model-name`

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

# Embedding Provider Keys
export JINA_API_KEY="your_jina_key"
```

### Configuration Overrides

Environment variables take precedence over configuration files:

```bash
export OCTODEV_LOG_LEVEL="debug"
export OCTODEV_EMBEDDING_PROVIDER="jina"
```

## Mode-Specific Configuration

### Agent Mode

Agent mode is designed for full development assistance:

```toml
[agent]
system = "You are an Octodev AI developer assistant with full access to development tools."

[agent.openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

[agent.mcp]
enabled = true
providers = ["core", "filesystem"]
```

### Chat Mode

Chat mode is optimized for simple conversations:

```toml
[chat]
system = "You are a helpful assistant."

[chat.openrouter]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false

[chat.mcp]
enabled = false
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

### Basic MCP Setup

```toml
[mcp]
enabled = true
providers = ["core"]
servers = []
```

### External MCP Servers

```toml
# Remote MCP server
[[mcp.servers]]
enabled = true
name = "WebSearch"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "optional_token"
tools = []  # Empty means all tools enabled

# Local MCP server
[[mcp.servers]]
enabled = true
name = "LocalTools"
command = "python"
args = ["-m", "my_mcp_server", "--port", "8008"]
mode = "http"
timeout = 30
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

### Security Best Practices

1. **Never commit API keys** to version control
2. **Use environment variables** for sensitive data
3. **Validate configuration** before deploying
4. **Use secure file permissions** for config files

```bash
# Secure config file permissions
chmod 600 .octodev/config.toml
```

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

### Debug Configuration

```toml
[openrouter]
log_level = "debug"
```

This enables detailed logging for troubleshooting configuration issues.