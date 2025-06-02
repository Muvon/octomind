# Session Modes and Interactive Usage

## Overview

Octomind supports flexible session roles for different use cases, with two defaults provided and an extensible system for custom roles.

## Session Roles Comparison

| Feature | Developer Role | Assistant Role | Custom Roles |
|---------|----------------|----------------|---------------|
| **Purpose** | Full development assistance | Simple conversation | Specialized use cases |
| **Indexing** | Full codebase indexing | No indexing (faster startup) | Configurable |
| **Tools** | All development tools enabled | Tools disabled by default | Configurable |
| **Layers** | Supports layered architecture | Direct model interaction | Configurable |
| **Context** | Full project context | Minimal context | Configurable |
| **Resource Usage** | Higher (more features) | Lower (lightweight) | Depends on configuration |
| **Inheritance** | Inherits from global config | Base for custom roles | Inherits from assistant |

## Developer Role

Developer role is the default and provides comprehensive development assistance.

### Starting Developer Role

```bash
# Default role (developer)
octomind session

# Explicitly specify developer role
octomind session --role=developer

# Developer role with specific model
octomind session --role=developer --model="openrouter:anthropic/claude-sonnet-4"

# Named developer session
octomind session --role=developer -n development_session
```

### Developer Role Features

#### Full Tool Access
- **Shell commands**: Execute terminal commands
- **File operations**: Read, write, edit files
- **Code search**: Semantic code search
- **Project analysis**: Understanding codebase structure
- **GraphRAG**: Code relationship analysis

#### Project Context Collection
- README.md content
- Git status and branch information
- File tree structure
- Project metadata

#### Layered Architecture
Three-layer processing for complex tasks:
1. **Query Processor**: Analyzes user requests
2. **Context Generator**: Gathers relevant information
3. **Developer**: Executes development tasks

### Developer Role Configuration

```toml
[developer]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true
system = "You are an Octomind AI developer assistant with full access to development tools."

# MCP configuration using new server registry approach
[developer.mcp]
enabled = true
server_refs = ["developer", "filesystem"]  # Reference servers from registry
allowed_tools = []  # Empty means all tools from referenced servers

# Server registry (define once, reference everywhere)
[mcp_server_registry.developer]
enabled = true
name = "developer"
server_type = "developer"

[mcp_server_registry.filesystem]
enabled = true
name = "filesystem"
server_type = "filesystem"
```

## Assistant Role

Assistant role is optimized for lightweight conversations without the overhead of full development tools.

### Starting Assistant Role

```bash
# Assistant role
octomind session --role=assistant

# Assistant role with specific model
octomind session --role=assistant --model="openai:gpt-4o-mini"

# Named assistant session
octomind session --role=assistant -n quick_chat
```

### Assistant Role Features

#### Lightweight Operation
- No codebase indexing
- Faster startup time
- Lower resource usage
- Simpler system prompts

#### Optional Tool Access
```toml
[assistant.mcp]
enabled = true  # Can enable tools if needed
server_refs = ["filesystem"]  # Specific servers only
allowed_tools = ["text_editor", "list_files"]  # Limited tools
```

### Assistant Role Configuration

```toml
[assistant]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false
system = "You are a helpful assistant."

[assistant.mcp]
enabled = false  # Tools disabled by default
```

## Custom Roles

Custom roles inherit from the assistant role as a base, then apply their own overrides. This provides a flexible system for creating specialized configurations.

### Creating Custom Roles

```bash
# Use a custom role
octomind session --role=code-reviewer
octomind session --role=security-analyst
octomind session --role=documentation-writer
```

### Custom Role Configuration

```toml
# Code reviewer role
[code-reviewer]
model = "openrouter:anthropic/claude-3.5-sonnet"
enable_layers = true
system = "You are a code review expert focused on security and best practices."

[code-reviewer.mcp]
enabled = true
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "list_files"]

# Security analyst role
[security-analyst]
model = "openrouter:anthropic/claude-3.5-sonnet"
enable_layers = true
system = "You are a security expert focused on finding vulnerabilities and security issues."

[security-analyst.mcp]
enabled = true
server_refs = ["developer"]
allowed_tools = ["shell"]  # Limited to analysis tools

# Documentation writer role
[documentation-writer]
model = "openrouter:openai/gpt-4o"
enable_layers = false
system = "You are a technical writer focused on creating clear, comprehensive documentation."

[documentation-writer.mcp]
enabled = true
server_refs = ["filesystem"]
allowed_tools = ["text_editor", "list_files"]  # Only file operations
```

### Role Inheritance

Custom roles follow this inheritance pattern:
1. **Start with assistant role** as the base configuration
2. **Apply custom overrides** from the role-specific configuration
3. **Merge MCP settings** with server registry references

```toml
# Assistant base (inherited by all custom roles)
[assistant]
model = "openrouter:anthropic/claude-3.5-haiku"
enable_layers = false
system = "You are a helpful assistant."

[assistant.mcp]
enabled = false

# Custom role inherits from assistant, then applies overrides
[my-custom-role]
model = "openrouter:openai/gpt-4o"  # Override model
enable_layers = true                # Override layers
system = "Custom system prompt"     # Override system prompt

[my-custom-role.mcp]
enabled = true                      # Override MCP enabled
server_refs = ["filesystem"]        # Add specific servers
```

## Session Management

### Creating and Managing Sessions

```bash
# Create new named session
octomind session -n project_review

# Resume existing session
octomind session -r project_review

# List all sessions
octomind session --list

# Session with custom model
octomind session --model="anthropic:claude-3-5-sonnet" -n analysis
```

### Session Commands

During a session, use these commands:

#### Navigation Commands
- `/help` - Show all available commands
- `/list` - List all sessions
- `/session [name]` - Switch to another session
- `/exit` or `/quit` - Exit current session

#### Configuration Commands
- `/model [model]` - Show/change current model
- `/info` - Display token usage and costs
- `/debug` - Toggle debug mode

#### Context Management
- `/cache` - Mark cache checkpoint
- `/truncate [threshold]` - Toggle auto-truncation
- `/done` - Optimize context and restart layers
- `/clear` - Clear screen
- `/save` - Save session

#### Architecture Commands
- `/layers` - Toggle layered processing

### Session Storage

Sessions are stored in `.octomind/sessions/`:

```
.octomind/sessions/
├── default.jsonl           # Default session
├── project_review.jsonl    # Named session
└── quick_chat.jsonl        # Chat mode session
```

Each session file contains:
- Message history
- Token usage statistics
- Layer processing stats
- Cache markers
- Session metadata

## Layered Architecture

### How Layers Work

The layered architecture processes complex requests through specialized stages:

```mermaid
graph TB
    A[User Input] --> B[Query Processor]
    B --> C[Context Generator]
    C --> D[Developer]
    D --> E[Response]

    F[Manual /done] --> G[Reducer]
    G --> H[Optimized Context]
```

### Layer Configuration

#### Default Layers
```toml
[openrouter]
enable_layers = true

# Uses default models for each layer:
# - Query Processor: openrouter:openai/gpt-4.1-nano
# - Context Generator: openrouter:google/gemini-2.5-flash-preview
# - Developer: main model from config
```

#### Custom Layer Models
```toml
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true

# Override models for specific layers
query_processor_model = "openrouter:openai/gpt-4.1-nano"
context_generator_model = "openrouter:google/gemini-1.5-flash"
developer_model = "openrouter:anthropic/claude-sonnet-4"
reducer_model = "openrouter:openai/gpt-4o-mini"
```

#### Advanced Layer Configuration
```toml
[[layers]]
name = "query_processor"
enabled = true
model = "openrouter:openai/gpt-4.1-nano"
temperature = 0.1
enable_tools = false
input_mode = "Last"
system_prompt = "You analyze and improve user queries."

[[layers]]
name = "context_generator"
enabled = true
model = "openrouter:google/gemini-1.5-flash"
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

### Input Modes

Layers can process input in different modes:

- **Last**: Only the most recent output from previous layer
- **All**: All context from previous layers
- **Summary**: Summarized version of all previous context

## Tool Integration (MCP)

### Available Tools

#### Core Tools
- **shell**: Execute shell commands
- **text_editor**: Edit files
- **list_files**: Browse directories
- **html2md**: Convert HTML to Markdown

#### Development Tools
- **Project analysis**: Built-in code understanding

### Tool Usage Examples

```bash
# In session, AI can use tools automatically:
> "List all Python files in the src directory"

AI uses: list_files
Parameters: {"directory": "src", "pattern": "*.py"}

> "Show me the authentication function"

AI analyzes files and finds relevant code automatically

> "Edit the config file to add a new setting"

AI uses: text_editor
Parameters: {"command": "str_replace", "path": "config.toml", ...}
```

### Tool Configuration

```toml
# Global MCP configuration
[mcp]
enabled = true
providers = ["core"]

# Mode-specific tool access
[agent.mcp]
enabled = true
providers = ["core", "filesystem", "development"]

[chat.mcp]
enabled = false  # No tools in chat mode
```

## Performance and Cost Optimization

### Model Selection by Use Case

#### For Quick Questions (Assistant Role)
```toml
[assistant]
model = "google:gemini-1.5-flash"  # Fast and cheap
```

#### For Development Work (Developer Role)
```toml
[developer]
model = "openrouter:anthropic/claude-sonnet-4"  # Best reasoning
```

#### Layer-Specific Optimization
```toml
# Cheap models for simple processing
query_processor_model = "google:gemini-1.5-flash"
context_generator_model = "openai:gpt-4o-mini"

# Expensive model only for final development work
developer_model = "openrouter:anthropic/claude-sonnet-4"
```

### Token Management

#### Automatic Management
```toml
[openrouter]
cache_tokens_pct_threshold = 40  # Auto-cache at 40%
max_request_tokens_threshold = 50000  # Auto-truncate
enable_auto_truncation = true
```

#### Manual Management
```bash
# In session:
/cache           # Mark cache point
/truncate        # Toggle auto-truncation
/info            # Check token usage
/done            # Optimize context
```

## Best Practices

### Choose the Right Mode

#### Use Developer Role When:
- Working on code development
- Need access to project files
- Require code analysis
- Want AI to execute commands
- Need full project context

#### Use Assistant Role When:
- Quick questions
- General conversations
- No need for project context
- Want faster responses
- Simple text processing

#### Use Custom Roles When:
- Need specialized behavior
- Want limited tool access
- Have specific use cases
- Need role-specific prompts

### Session Organization

```bash
# Organize sessions by purpose
octomind session -n bug_fixing --role=developer
octomind session -n code_review --role=code-reviewer
octomind session -n quick_help --role=assistant
octomind session -n security_audit --role=security-analyst
```

### Cost Control

1. **Use appropriate models**: Expensive for complex, cheap for simple
2. **Enable caching**: Reduce repeated context costs
3. **Monitor usage**: Check `/info` regularly
4. **Optimize layers**: Use cheap models for processing layers
5. **Truncate context**: Use `/done` to optimize

### Session Hygiene

1. **Save regularly**: Use `/save` for important sessions
2. **Clean up**: Remove old sessions periodically
3. **Use descriptive names**: Make sessions easy to identify
4. **Resume efficiently**: Use `-r` to continue work
