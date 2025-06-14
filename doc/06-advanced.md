# Advanced Features Guide

## Overview

Octomind's advanced features enable sophisticated development workflows through MCP tool integration, layered AI architecture, and extensible configuration. This guide covers capabilities beyond basic session usage.

## MCP (Model-Centric Programming) Protocol

### What is MCP?

MCP enables AI models to use external tools and services through a standardized protocol. Octomind provides development capabilities through natural conversation by integrating tools seamlessly into AI interactions.

### Built-in MCP Tools

#### Developer Tools (server_type: "developer")
- **shell**: Execute terminal commands and development scripts
- **agent**: Route tasks to configured AI layers for specialized processing
- **Code analysis**: Built-in code understanding and project analysis

#### Filesystem Tools (server_type: "filesystem")
- **text_editor**: Read, write, edit files with multiple operations (view, create, str_replace, insert, line_replace, undo_edit, view_many, batch_edit)
- **list_files**: Browse directory structures with pattern matching and content search
- **html2md**: Convert HTML content to Markdown format

### Agent Tools Reference

The agent system enables task delegation to specialized AI agents configured in your system. Each configured agent becomes a separate MCP tool that routes tasks to specialized AI layers.

#### How It Works

1. **Configure Agents**: Define agents in the `[[agents]]` section of your config
2. **Configure Layers**: Create corresponding layers that agents will use
3. **Use Agent Tools**: Each agent becomes a tool like `agent_code_reviewer`, `agent_debugger`, etc.

#### Agent Configuration

First, define your agents in `config.toml`:

```toml
# Agent definitions - each becomes a separate MCP tool
[[agents]]
name = "code_reviewer"
description = "Review code for performance, security, and best practices issues. Analyzes code quality and suggests improvements."

[[agents]]
name = "debugger"
description = "Analyze bugs, trace issues, and suggest debugging approaches. Helps identify root causes and solutions."

[[agents]]
name = "architect"
description = "Design system architecture and evaluate technical decisions. Provides high-level design guidance."
```

#### Layer Configuration

Then create corresponding layers that agents will use:

```toml
# Agent layers - specialized AI configurations
[[layers]]
name = "code_reviewer"
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = "You are a senior code reviewer. Analyze code for quality, performance, security, and best practices. Provide detailed feedback with specific suggestions for improvement."
temperature = 0.1
input_mode = "Last"
builtin = false

[layers.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "list_files"]

[[layers]]
name = "debugger"
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = "You are an expert bug hunter and debugger. Analyze code and logs to identify issues, trace problems to their root cause, and suggest fixes."
temperature = 0.1
input_mode = "Last"
builtin = false

[layers.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "shell", "list_files"]

[[layers]]
name = "architect"
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = "You are a senior software architect. Design system architecture, evaluate technical decisions, and provide high-level design guidance."
temperature = 0.2
input_mode = "Last"
builtin = false

[layers.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "list_files"]
```

#### Usage Examples

Once configured, each agent becomes a separate tool:

**Code Review Agent:**
```bash
# In session
agent_code_reviewer(task="Review this function for performance issues and suggest improvements")
```

**Debugging Agent:**
```bash
# In session
agent_debugger(task="Help me debug this error: Cannot find module 'express'")
```

**Architecture Agent:**
```bash
# In session
agent_architect(task="Design a scalable architecture for user authentication system")
```

#### Tool Parameters

Each agent tool has the same parameter structure:

**Parameters:**
- `task` (string, required): Task description in human language for the agent to process

#### Key Features

- **Individual Tools**: Each agent becomes a separate MCP tool (e.g., `agent_code_reviewer`)
- **Layer Integration**: Uses the full layer system (models, prompts, MCP tools)
- **Configurable**: Custom agent descriptions and specialized layers
- **Isolated Processing**: Each agent runs in its own session context
- **Tool Access**: Agents can use MCP tools based on their layer configuration
- **Flexible**: Easy to add new specialized agents for different tasks

### Text Editor Tool Reference

The `text_editor` tool provides comprehensive file manipulation capabilities through multiple commands:

#### Individual Operations

**view** - Examine file contents or directory listings
```json
{"command": "view", "path": "src/main.rs"}
{"command": "view", "path": "src/main.rs", "view_range": [10, 20]}
{"command": "view", "path": "src/"}
```

**create** - Create new files with content
```json
{"command": "create", "path": "src/new_module.rs", "file_text": "pub fn hello() {\n    println!(\"Hello!\");\n}"}
```

**str_replace** - Replace specific strings in files
```json
{"command": "str_replace", "path": "src/main.rs", "old_str": "fn old_name()", "new_str": "fn new_name()"}
```

**insert** - Insert text at specific line positions
```json
{"command": "insert", "path": "src/main.rs", "insert_line": 5, "new_str": "// New comment\nlet x = 10;"}
```

**line_replace** - Replace content within specific line ranges
```json
{"command": "line_replace", "path": "src/main.rs", "view_range": [5, 8], "new_str": "fn updated_function() {\n    // New implementation\n}"}
```

**undo_edit** - Revert the most recent edit
```json
{"command": "undo_edit", "path": "src/main.rs"}
```

**view_many** - View multiple files simultaneously
```json
{"command": "view_many", "paths": ["src/main.rs", "src/lib.rs", "tests/test.rs"]}
```

#### Batch Operations

**batch_edit** - Perform multiple editing operations in a single call
```json
{
  "command": "batch_edit",
  "operations": [
    {
      "operation": "str_replace",
      "path": "src/main.rs",
      "old_str": "old_function_name",
      "new_str": "new_function_name"
    },
    {
      "operation": "insert",
      "path": "src/lib.rs",
      "insert_line": 5,
      "new_str": "// New comment\nuse new_module;"
    },
    {
      "operation": "line_replace",
      "path": "src/config.rs",
      "view_range": [10, 15],
      "new_str": "// Updated configuration\nconst NEW_CONFIG: &str = \"value\";"
    }
  ]
}
```

**Batch Edit Features:**
- **Maximum 50 operations** per batch for performance
- **Supported operations**: str_replace, insert, line_replace
- **Cross-file editing**: Make changes across multiple files simultaneously
- **Detailed reporting**: Success/failure status for each operation
- **Error isolation**: Failed operations don't affect successful ones
- **File history preservation**: Each operation saves file history for undo

**When to Use Batch Edit:**
- ✅ **Multiple file refactoring** - Rename functions across files
- ✅ **Consistent changes** - Apply same pattern to multiple files
- ✅ **Independent modifications** - Changes that don't depend on each other
- ✅ **Bulk updates** - Update imports, comments, or configuration
- ❌ **Sequential dependencies** - When changes depend on previous results
- ❌ **Complex logic** - When you need conditional modifications

### MCP Server Configuration

The MCP system uses a centralized server configuration in the main `[mcp]` section:

```toml
# MCP Server Configuration - Define servers once, reference everywhere
[mcp]
allowed_tools = []

# Built-in server definitions
[[mcp.servers]]
name = "developer"
server_type = "developer"
mode = "http"
timeout_seconds = 30
args = []
tools = []  # Empty means all tools enabled
builtin = true

[[mcp.servers]]
name = "filesystem"
server_type = "filesystem"
mode = "http"
timeout_seconds = 30
args = []
tools = []  # Empty means all tools enabled
builtin = true

# External HTTP server example
[[mcp.servers]]
name = "web_search"
server_type = "external"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "optional_token"
mode = "http"
timeout_seconds = 30
tools = []
builtin = false

# External command-based server example
[[mcp.servers]]
name = "local_tools"
server_type = "external"
command = "python"
args = ["-m", "my_mcp_server", "--port", "8008"]
mode = "stdin"
timeout_seconds = 30
tools = ["custom_tool1", "custom_tool2"]  # Only these tools enabled
builtin = false
```

### Role-Based Server Access

Roles reference servers from the main MCP configuration and can limit tool access:

```toml
# Developer role with full access
[developer.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = []  # Empty means all tools from referenced servers

# Assistant role with limited access
[assistant.mcp]
server_refs = ["filesystem"]
allowed_tools = ["text_editor", "list_files"]  # Only specific tools

# Custom role with external tools
[code-reviewer.mcp]
server_refs = ["developer", "web_search"]
allowed_tools = ["text_editor", "shell"]
```

### Server Types

- **developer**: Built-in development tools (shell commands, code analysis)
- **filesystem**: Built-in file operations (reading, writing, editing files)
- **external**: External MCP servers (HTTP or command-based)

### External MCP Servers

#### HTTP-based Servers
```toml
[[mcp.servers]]
name = "web_tools"
server_type = "external"
url = "https://api.example.com/mcp"
auth_token = "your_token"
mode = "http"
timeout_seconds = 30
tools = []
builtin = false
```

#### Command-based Servers
```toml
[[mcp.servers]]
name = "custom_tools"
server_type = "external"
command = "python"
args = ["/path/to/mcp_server.py"]
mode = "stdin"
timeout_seconds = 30
```

## Layered Architecture

### Overview

For complex development tasks, Octomind uses a flexible multi-stage AI processing system where each layer is fully configurable through the configuration file. All layers use the same `GenericLayer` implementation with different configurations.

```mermaid
graph TB
    A[User Input] --> B[Layer Pipeline]
    B --> C[Query Processor - output_mode: none]
    C --> D[Context Generator - output_mode: replace]
    D --> E[Final Response]


```

### Layer Configuration System

All layers are configured through the `[[layers]]` section in your configuration file. Each layer supports:

- **Input Mode**: How the layer receives input (`last`, `all`, `summary`)
- **Output Mode**: How the layer affects the session (`none`, `append`, `replace`)
- **Model Selection**: Specific model for this layer
- **MCP Tools**: Which tools the layer can access
- **Custom Prompts**: Layer-specific system prompts

#### Output Modes Explained

- **`none`**: Intermediate layer that doesn't modify the session (like query_processor)
- **`append`**: Adds layer output as a new message to the session
- **`replace`**: Replaces the entire session content with the layer output

**Context Management Commands:**
- **`/done`**: Task completion using current model - comprehensive summarization with memorization and auto-commit

### Built-in Layer Types

#### Query Processor
- **Purpose**: Analyze and improve user requests
- **Configuration**: `output_mode = "none"` (intermediate processing)
- **Default Model**: Fast, cost-effective model for text analysis

#### Context Generator
- **Purpose**: Gather project context and prepare comprehensive responses
- **Configuration**: `output_mode = "replace"` (replaces input with enriched context)
- **Default Model**: Balanced model with tool access for code analysis

#### Reducer
- **Purpose**: Optimize and compress session history
- **Configuration**: `output_mode = "replace"` (replaces session with compressed content)
### Layered Architecture Configuration

All layers are configured through the `[[layers]]` section with consistent parameters:

```toml
[developer]
enable_layers = true

# All layers use the same GenericLayer implementation with different configurations

[[layers]]
name = "query_processor"
model = "openrouter:openai/gpt-4.1-mini"
temperature = 0.2
input_mode = "Last"
output_mode = "none"  # Intermediate layer - doesn't modify session
builtin = true

[layers.mcp]
server_refs = []
allowed_tools = []

[[layers]]
name = "context_generator"
model = "openrouter:google/gemini-2.5-flash-preview"
temperature = 0.2
input_mode = "Last"
output_mode = "replace"  # Replaces input with processed context
builtin = true

[layers.mcp]
server_refs = ["developer", "filesystem", "octocode"]
allowed_tools = ["search_code", "view_signatures", "list_files"]

[[layers]]
name = "reducer"
model = "openrouter:openai/o4-mini"
temperature = 0.2
input_mode = "All"
output_mode = "replace"  # Replaces entire session with reduced content
builtin = true

[layers.mcp]
server_refs = []
allowed_tools = []
```

### Custom Layer Configuration

You can create custom layers with any combination of settings:

```toml
[[layers]]
name = "code_reviewer"
model = "openrouter:anthropic/claude-3.5-sonnet"
system_prompt = "You are a senior code reviewer..."
temperature = 0.1
input_mode = "Last"
output_mode = "append"  # Add review results to session
builtin = false

[layers.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "list_files"]
```
allowed_tools = ["core", "text_editor"]
input_mode = "last"

[[layers]]
name = "developer"
enabled = true
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.3
enable_tools = true
input_mode = "all"
```

### Session Commands for Layers

- `/layers` - Toggle layered processing on/off
- `/done` - Manually trigger context optimization
- `/info` - View token usage by layer

## Token Management

### Automatic Token Management

```toml
[developer]
# Warn when tool outputs exceed threshold
mcp_response_warning_threshold = 20000

# Auto-truncate context when limit reached
max_request_tokens_threshold = 50000
enable_auto_truncation = false

# Cache management
cache_tokens_pct_threshold = 40
```

### Session Token Commands

- `/cache` - Mark cache checkpoint for cost savings
- `/truncate [threshold]` - Toggle auto-truncation
- `/info` - Display token usage and cost breakdown

## Advanced Configuration Patterns

### Multi-Provider Setup
```toml
# Use different providers for different purposes
[developer]
model = "openrouter:anthropic/claude-sonnet-4"  # Main development
query_processor_model = "openai:gpt-4o-mini"   # Fast processing
context_generator_model = "google:gemini-1.5-flash"  # Good balance

[assistant]
model = "openrouter:anthropic/claude-3.5-haiku"  # Lightweight chat
```

### Role-Specific Tool Access
```toml
# Security-focused role
[security-reviewer]
model = "openrouter:anthropic/claude-3.5-sonnet"
enable_layers = true

[security-reviewer.mcp]
enabled = true
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "shell"]  # Limited tools for security focus

# Documentation role
[docs-writer]
model = "openrouter:openai/gpt-4o"
enable_layers = false

[docs-writer.mcp]
enabled = true
server_refs = ["filesystem"]
allowed_tools = ["text_editor", "html2md"]  # Only doc-related tools
```

### External Tool Integration
```toml
# Web development setup
[web-dev]
model = "openrouter:anthropic/claude-sonnet-4"

[web-dev.mcp]
enabled = true
server_refs = ["developer", "filesystem", "web_tools"]

# Add web-specific MCP server
[[mcp.servers]]
name = "web_tools"
server_type = "external"
url = "https://mcp.so/server/web-dev-tools"
mode = "http"
timeout_seconds = 30
tools = []
builtin = false
```

## Session Management

### Session Persistence
- **Save sessions**: All conversations are automatically saved
- **Resume sessions**: Continue where you left off
- **Session switching**: Work on multiple projects simultaneously

### Session Commands
```bash
# In any session
/help              # Show all available commands
/list              # List all sessions
/session [name]    # Switch to another session
/save              # Manually save current session
/model [model]     # Change AI model
/clear             # Clear screen
/exit              # Exit session
```

### Session Organization
```bash
# Start named sessions for different purposes
octomind session --name "feature-auth"
octomind session --name "bugfix-login"
octomind session --name "refactor-api"

# Resume specific sessions
octomind session --resume "feature-auth"
```

## Development Workflow Integration

### Project Context Collection
Sessions automatically analyze:
- **Project structure** and organization
- **Configuration files** and build systems
- **Documentation** and README files
- **Git repository** information

### Natural Development Tasks
Instead of complex commands, simply ask:
- **"How does authentication work?"** - AI analyzes auth code
- **"Add logging to the login function"** - AI implements logging
- **"Why is the build failing?"** - AI checks build errors
- **"Refactor this function"** - AI improves code structure

### Code Analysis Capabilities
Through natural conversation:
- **File exploration**: "Show me the main configuration files"
- **Code understanding**: "Explain how this module works"
- **Pattern finding**: "Find all error handling patterns"
- **Dependency analysis**: "What files import this module?"

## Performance Optimization

### Model Selection Strategy
1. **Fast models** for simple analysis (Query Processor)
2. **Balanced models** for information gathering (Context Generator)
3. **Powerful models** for complex development tasks (Developer)

### Tool Usage Optimization
- **Batch operations**: Use `view_many` for reading multiple files, `batch_edit` for modifying multiple files
- **Specific patterns**: Use `list_files` with patterns to filter results
- **Smart caching**: Use `/cache` before large context operations

### Context Management
- **Auto-truncation**: Enable for long sessions
- **Task completion**: Use `/done` to finalize tasks with memorization and commit
- **Token monitoring**: Use `/info` to track usage

## Troubleshooting

### Common Issues

#### MCP Configuration Problems
```bash
# Validate configuration
octomind config --validate

# Check MCP server connectivity
# (Server status is checked automatically when tools are used)
```

#### Tool Access Issues
- **Check role configuration**: Ensure server_refs include needed servers
- **Verify tool permissions**: Check allowed_tools list
- **External server issues**: Verify URL and authentication

#### Layer Performance Issues
```bash
# Monitor layer performance
/info

# Disable layers temporarily
/layers

# Optimize context
/done
```

#### Token Limit Issues
```bash
# Enable auto-truncation
/truncate 30000

# Mark cache checkpoint
/cache

# Optimize context manually
/done
```

### Debug Mode
```bash
# Enable debug logging in session
/loglevel debug

# Or in configuration
log_level = "debug"
```

## Best Practices

### MCP Usage
1. **Start with built-in servers** before adding external ones
2. **Limit tool access** in specialized roles for security
3. **Test external servers** thoroughly before deployment
4. **Monitor tool performance** through session feedback

### Layered Architecture
1. **Enable for complex tasks** that benefit from specialized processing
2. **Use appropriate models** for each layer's complexity
3. **Monitor token usage** across layers with `/info`
4. **Optimize context** regularly with `/done`

### Session Management
1. **Use descriptive names** for sessions
2. **Save important sessions** manually when needed
3. **Switch sessions** for different projects or tasks
4. **Monitor token usage** to control costs

### Development Workflow
1. **Ask natural questions** instead of trying to construct complex commands
2. **Be specific** about what you want to accomplish
3. **Use session commands** to manage context and performance
4. **Leverage auto-analysis** by letting sessions examine your project structure

## Migration from Legacy Configuration

### MCP Migration
**Old format:**
```toml
[mcp]
enabled = true
providers = ["core"]
```

**Current format:**
```toml
[[mcp.servers]]
name = "developer"
server_type = "developer"
mode = "http"
timeout_seconds = 30
args = []
tools = []
builtin = true

[developer.mcp]
server_refs = ["developer"]
allowed_tools = []
```

### Provider Migration
**Old format:**
```toml
model = "anthropic/claude-3.5-sonnet"
```

**New format:**
```toml
model = "openrouter:anthropic/claude-3.5-sonnet"
```

Octomind automatically migrates legacy configurations, but manual updates provide better control and understanding of the new simplified structure.
