# Layer Configuration Guide

## Overview

Octodev supports a multi-layer architecture for processing user queries. Each layer serves a specific function in the pipeline:

1. **Query Processor Layer**: Analyzes and clarifies user requests
2. **Context Generator Layer**: Gathers relevant context using tools
3. **Developer Layer**: Implements solutions based on the context
4. **Reducer Layer**: Optimizes and summarizes context for future interactions

## Configuration

Layers can be configured in two ways:

1. **Default Configuration**: When no specific layer configuration is provided in `config.toml`
2. **Custom Configuration**: By defining layers in the `config.toml` file

### Layer Configuration Options

Each layer supports the following configuration options:

| Option | Description | Example |
|--------|-------------|--------|
| `name` | Identifier for the layer | `"query_processor"` |
| `enabled` | Whether the layer is active | `true` |
| `model` | The model to use for this layer | `"openai/gpt-4.1-nano"` |
| `system_prompt` | Custom system instructions | `"You analyze queries..."` |
| `temperature` | Creativity setting (0.0-1.0) | `0.2` |
| `enable_tools` | Whether tools can be used | `true` |
| `allowed_tools` | Specific tools that can be used | `["shell", "text_editor"]` |
| `input_mode` | How input is prepared from previous layer | `"Last"`, `"All"`, or `"Summary"` |

### Input Modes

The `input_mode` setting controls how each layer receives input from the previous layer:

- **Last**: Only the most recent output is used (default)
- **All**: All context from previous layers is included
- **Summary**: A summarized version of all previous context is used

## Example Configuration

```toml
# Layer configurations 
[[layers]]
name = "query_processor"
enabled = true
model = "openai/gpt-4.1-nano"
system_prompt = "Custom system prompt for query processing layer."
temperature = 0.1
enable_tools = false
allowed_tools = []
input_mode = "Last"

[[layers]]
name = "context_generator"
enabled = true
model = "google/gemini-2.5-flash-preview"
system_prompt = "Custom system prompt for context generator."
temperature = 0.2
enable_tools = true
allowed_tools = ["shell", "text_editor", "list_files"]
input_mode = "Last"

[[layers]]
name = "developer"
enabled = true
model = "anthropic/claude-sonnet-4"
system_prompt = "Custom system prompt for developer."
temperature = 0.3
enable_tools = true
allowed_tools = []
input_mode = "All"

[[layers]]
name = "reducer"
enabled = false
model = "openai/o4-mini"
system_prompt = "Custom system prompt for reducer."
temperature = 0.2
enable_tools = false
allowed_tools = []
input_mode = "Summary"
```

## Default Layer Configuration

If no layers are configured, the system uses default settings:

- **Query Processor**: Model `openai/gpt-4.1-nano`, no tools
- **Context Generator**: Model `google/gemini-2.5-flash-preview`, limited tools for context gathering
- **Developer**: Uses the main model specified in the config, all tools enabled

## Enabling/Disabling Layers

The entire layer system can be enabled/disabled with the `openrouter.enable_layers` setting:

```toml
[openrouter]
enable_layers = true  # Set to false to disable the layer system
```

Individual layers can be enabled/disabled with the `enabled` setting in each layer configuration.

## Placeholder Variables in System Prompts

System prompts can include placeholder variables that get replaced with actual content at runtime:

| Placeholder | Description | Example |
|-------------|-------------|--------|
| `%{CONTEXT}` | All project context information | Project README, file tree, git status, etc. |
| `%{GIT_STATUS}` | Only git status information | Shows modifications, untracked files, etc. |
| `%{GIT_TREE}` | Only file tree information | List of files in the project |
| `%{README}` | Only the README content | Contents of README.md if available |

Example usage in a system prompt:

```toml
system_prompt = "You are a coding assistant. %{CONTEXT}"
```

This allows for selectively including only the context elements that are needed for each layer.