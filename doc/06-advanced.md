# Advanced Features Guide

## Overview

Octomind's advanced features enable sophisticated development workflows through MCP tool integration, layered AI architecture, and extensible configuration. This guide covers capabilities beyond basic session usage.

## MCP (Model-Centric Programming) Protocol

### What is MCP?

MCP enables AI models to use external tools and services through a standardized protocol. Octomind provides development capabilities through natural conversation by integrating tools seamlessly into AI interactions.

### MCP Protocol Compliance

**CRITICAL**: All Octomind MCP tools are fully protocol-compliant and handle errors gracefully:

- ✅ **Error Handling**: Tools return `McpToolResult::error()` instead of crashing communication
- ✅ **Parameter Validation**: Clear error messages for missing, empty, or wrong-type parameters
- ✅ **API Key Management**: Graceful handling of missing environment variables
- ✅ **Cancellation Support**: Proper handling of user cancellation requests
- ✅ **Standard Format**: All responses follow MCP standard: `{content: [{type: "text", text: "..."}], isError: true/false}`

### Built-in MCP Tools

Octomind provides four built-in MCP servers with comprehensive development capabilities:

**Developer Server** (`src/mcp/dev/`):
- `shell(command="...", background=false)` - Execute shell commands with output capture, foreground/background execution
- `ast_grep(pattern="...", language="...", rewrite="...", ...)` - Search and refactor code using AST patterns
- `plan(command="start|step|next|list|done|reset", ...)` - Structured task management with progress tracking

**Filesystem Server** (`src/mcp/fs/`):
- `view(path=\"...\", lines=[start, end], pattern=\"...\", content=\"...\", ...)` - Read files, view directories, and search file content
- `text_editor(command=\"create|str_replace|insert|line_replace|undo_edit\", path=\"...\", ...)` - Edit files
- `batch_edit(path=\"...\", operations=[...])` - Multiple file operations atomically
- `extract_lines(from_path=\"...\", from_range=[start, end], append_path=\"...\", append_line=N)` - Extract and move code blocks


**Web Server** (`src/mcp/web/`):
- `web_search(query="...", count=20, ...)` - Search the web using Brave Search API
- `image_search(query="...", count=50, ...)` - Search for images with metadata
- `video_search(query="...", count=20, ...)` - Search for videos with duration info
- `news_search(query="...", count=20, ...)` - Search for news articles
- `read_html(sources=[...])` - Convert HTML content to Markdown format

**Agent Server** (`src/mcp/agent/`):
- `agent_*()` tools - Route tasks to configured AI layers for specialized processing
- `call_llm(prompt="...", model="...", system="...", temperature=0.7)` - Direct LLM call with runtime parameters

**Tool Invocation:**

---

### plan — Structured Task Management Tool

The `plan` tool enables interactive, step-by-step task management inside Octomind sessions. It supports workflow breakdown, progress tracking, and structured execution for complex development tasks.

**Purpose:**
- Break down large objectives into clear, actionable steps
- Track progress and provide visual feedback for each step
- Integrate seamlessly with session and MCP protocols

**Commands & Parameters:**
- `command` (string, required): One of the following commands:
  - **`start`**: Begin a new plan
    - `title` (string, required): Plan title
    - `tasks` (array of objects, required): List of subtasks with `title` and `description` fields
  - **`step`**: Add progress or notes to current step
    - `content` (string, required): Progress detail
  - **`next`**: Mark current step as complete and advance
    - `content` (string, required): Completion summary
  - **`list`**: Show all steps with completion status
  - **`done`**: Mark plan as complete and trigger session cleanup
    - `content` (string, optional): Final summary
  - **`reset`**: Abort and clear current plan

**Usage Example:**
```json
{"command": "start", "title": "Implement Feature X", "tasks": [{"title": "Design API", "description": "Create API endpoints"}, {"title": "Write tests", "description": "Unit and integration tests"}, {"title": "Implement logic", "description": "Core functionality"}]}
{"command": "step", "content": "Started API design..."}
{"command": "next", "content": "API designed, moving to tests"}
{"command": "list"}
{"command": "done", "content": "Feature implemented and tests passing"}
```

**MCP Compliance:**
- All errors use `Ok(McpToolResult::error(...))` (never `Err()`)
- Parameter validation is strict; missing/invalid params return actionable MCP error objects
- Output always includes `tool_id` and follows `{content: [{type: "text", text: "..."}], isError: ...}`
- Handles cancellation, session cleanup, and preserves MCP protocol integrity

**Session Integration:**
- `/done` triggers plan completion, summary, and memory cleanup
- Full progress is tracked for review and reporting

**Benefits:**
- Structured, sequential execution of complex tasks
- Visual progress feedback within session
- Clean error handling and robust MCP protocol support

See `src/mcp/dev/plan/` for code, and test integration in `src/session/chat/session/main_loop.rs`.
- Single tool: clean header, no index
- Multiple tools: indexed headers

**Adding a Tool/Server:**
- Add your tool/server in config and code (see [08-mcp-server-development.md](./08-mcp-server-development.md))
- Always use config for registration, server_refs, allowed_tools

#### shell — Shell Command Execution

Execute shell commands with output capture, foreground/background execution:

```json
// Foreground execution (default)
{"command": "ls -la"}

// Background execution
{"command": "python -m http.server 8000", "background": true}
// Returns: {"success": true, "background": true, "pid": 12345, "message": "...", "note": "Use 'kill 12345' to terminate..."}

// Kill background process
{"command": "kill 12345"}
```

**Parameters:**
- `command` (string, required): The shell command to execute
- `background` (boolean, default: false): Run command in background and return PID instead of waiting for completion

**Key Features:**
- Working directory: All commands execute from the current working directory
- Background mode: Returns process PID for later termination with `kill <pid>`
- Output control: Large outputs are controlled by `mcp_response_tokens_threshold` setting

#### workdir — Manage Working Directory

Get or set the working directory for file and shell operations:

```json
// Get current working directory
{}

// Set new working directory
{"path": "/path/to/directory"}

// Reset to original project directory
{"reset": true}
```

**Parameters:**
- `path` (string, optional): Path to set as new working directory. Can be absolute or relative to current working directory.
- `reset` (boolean, default: false): If true, reset to original project directory (ignores `path` parameter).

**Key Features:**
- Parallel execution: Each actor can work in its own isolated git worktree
- Testing: Switch to a test directory before running tests
- Multi-project workflows: Work across multiple related projects
- Thread-local: Changes only affect the current thread/session

**Use Cases:**
```json
// Get current directory
{}

// Switch to test directory
{"path": "tests/"}

// Work in a git worktree
{"path": "/path/to/worktree"}

// Reset to project root
{"reset": true}
```

**ast_grep** - Search and refactor code using AST patterns with ast-grep (sg)
- **Structural search**: Use AST patterns instead of regex for precise code matching
- **Code refactoring**: Apply transformations using rewrite patterns
- **Multi-language support**: JavaScript, TypeScript, PHP, Rust, Python, Go, Java, C/C++
- **Context-aware output**: Configurable context lines around matches

```json
// Search for console.log calls
{"pattern": "console.log($$$)", "language": "javascript"}

// Rename function calls
{"pattern": "oldFunc($ARGS)", "rewrite": "newFunc($ARGS)", "language": "javascript"}

// Search specific files with context
{"pattern": "class $NAME", "language": "php", "paths": ["src/**/*.php"], "context": 2}
```

**Parameters:**
- `pattern` (string, required): The AST pattern to search for using ast-grep syntax
- `paths` (array, optional): File paths or glob patterns to search within (default: current directory)
- `language` (string, optional): Language of the code (e.g., 'rust', 'javascript', 'python', 'typescript', 'go', 'java', 'c', 'cpp', 'php')
- `rewrite` (string, optional): Rewrite pattern to apply for refactoring transformations
- `json_output` (boolean, default: false): Get output in JSON format
- `context` (integer, default: 0): Number of lines of context to show around matches
- `update_all` (boolean, default: false): Apply rewrites to all matches without confirmation
- `update_all` (boolean, default: false): Apply rewrites to all matches without confirmation


#### Filesystem Tools (type: "builtin")
- **view**: Read files, view directories, and search file content with pattern matching
- **text_editor**: Edit files with multiple operations (create, str_replace, insert, line_replace, undo_edit)
- **extract_lines**: Extract lines from source file and append to target file without modifying source (perfect for refactoring)
- **batch_edit**: Multiple file operations atomically


#### Web Tools (type: "builtin")
- **web_search**: Search the web using Brave Search API with configurable parameters
- **image_search**: Search for images using Brave Search API with metadata and thumbnails
- **video_search**: Search for videos using Brave Search API with duration, views, and creator info
- **news_search**: Search for news articles using Brave Search API with publication dates and breaking news flags
- **read_html**: Convert HTML content to Markdown format from URLs or local files

#### Web Search Tools Configuration

All web search tools require the `BRAVE_API_KEY` environment variable to be set with your Brave Search API key.

**Setup:**
```bash
export BRAVE_API_KEY="your_brave_api_key_here"
```

**Web Search (`web_search`)**
Search the web with comprehensive filtering options:

```json
{
  "query": "rust web framework",
  "count": 10,
  "country": "US",
  "search_lang": "en",
  "ui_lang": "en-US",
  "safesearch": "moderate",
  "freshness": "pw"
}
```

Parameters:
- `query` (required): Search query (max 400 chars, 50 words)
- `count`: Results to return (1-20, default: 20)
- `offset`: Results to skip for pagination (0-9, default: 0)
- `country`: Country code (e.g., "US", "GB", "DE")
- `search_lang`: Language code (e.g., "en", "es", "fr")
- `ui_lang`: UI language (e.g., "en-US", "es-ES")
- `safesearch`: "strict", "moderate", or "off"
- `freshness`: "pd" (day), "pw" (week), "pm" (month), "py" (year)

**Image Search (`image_search`)**
Find images with metadata and thumbnails:

```json
{
  "query": "golden retriever puppy",
  "count": 20,
  "country": "US",
  "search_lang": "en",
  "safesearch": "strict"
}
```

Parameters:
- `query` (required): Image search query
- `count`: Results to return (1-100, default: 50)
- `country`: Country code for localized results
- `search_lang`: Language for search results
- `safesearch`: "strict" or "off" (default: "strict")
- `spellcheck`: Enable spellcheck (default: true)

**Video Search (`video_search`)**
Search for videos with duration, views, and creator information:

```json
{
  "query": "rust programming tutorial",
  "count": 15,
  "offset": 0,
  "country": "US",
  "search_lang": "en",
  "ui_lang": "en-US",
  "safesearch": "moderate",
  "freshness": "pm"
}
```

Parameters:
- `query` (required): Video search query
- `count`: Results to return (1-50, default: 20)
- `offset`: Results to skip for pagination (0-9, default: 0)
- `country`: Country code for localized results
- `search_lang`: Language for search results
- `ui_lang`: UI language preference
- `safesearch`: "strict", "moderate", or "off"
- `freshness`: Time filter for recent videos
- `spellcheck`: Enable spellcheck (default: true)

**News Search (`news_search`)**
Find news articles with publication dates and breaking news flags:

```json
{
  "query": "artificial intelligence breakthrough",
  "count": 10,
  "country": "US",
  "search_lang": "en",
  "freshness": "pd",
  "extra_snippets": true
}
```

Parameters:
- `query` (required): News search query
- `count`: Results to return (1-50, default: 20)
- `offset`: Results to skip for pagination (0-9, default: 0)
- `country`: Country code for localized results
- `search_lang`: Language for search results
- `ui_lang`: UI language preference
- `safesearch`: "strict", "moderate", or "off"
- `freshness`: Time filter for recent news
- `spellcheck`: Enable spellcheck (default: true)
- `extra_snippets`: Get additional excerpts (default: false)

**Best Practices:**
- Use specific, targeted queries for better results
- Use quotes for exact phrase matching: `"machine learning"`
- Use site: operator for specific domains: `site:github.com`
- Use - operator to exclude terms: `python -django`
- For images: Use descriptive visual terms
- For videos: Include keywords like "tutorial", "review", "how to"
- For news: Include current event keywords and locations

### Agent Tools Reference

The agent system enables task delegation to specialized AI agents configured in your system. Each configured agent becomes a separate MCP tool that uses the same layer configuration system as commands and regular layers.

#### How It Works

1. **Configure Agents**: Define agents using the same `LayerConfig` structure as commands and layers
2. **Use Agent Tools**: Each agent becomes a tool like `agent_context_gatherer`, `agent_code_reviewer`, etc.
3. **Output Control**: The `output_mode` setting controls what the agent tool returns

#### Agent Configuration

Agents now use the **same configuration structure** as commands and layers. Define them in the `[[agents]]` section:

```toml
# Agent definitions - each becomes a separate MCP tool
[[agents]]
name = "context_gatherer"
description = "Gather detailed context from files and codebase. Reads files, searches code patterns, and provides comprehensive information about specific areas of the codebase for development tasks."
model = "openrouter:google/gemini-2.5-flash-preview"
max_tokens = 16384
system_prompt = """You are a comprehensive context gatherer and code analyst for development tasks. Your role is to thoroughly examine codebases, understand patterns, and provide detailed information about specific areas.

Your capabilities:
- Read and analyze multiple files simultaneously
- Search for code patterns semantically across the codebase
- Understand file structures and relationships
- Extract function signatures and code structure
- Provide comprehensive context for development decisions

Always provide comprehensive, detailed analysis that helps developers understand the codebase and make informed decisions."""
temperature = 0.2
input_mode = "last"
output_mode = "none"  # Return only the gathered context (cleanest for tool use)

[agents.mcp]
server_refs = ["filesystem", "octocode"]
allowed_tools = ["text_editor", "list_files"]

[[agents]]
name = "code_reviewer"
description = "Review code for performance, security, and best practices issues. Analyzes code quality and suggests improvements."
model = "openrouter:anthropic/claude-sonnet-4"
max_tokens = 8192
system_prompt = "You are a senior code reviewer. Analyze code for quality, performance, security, and best practices. Provide detailed feedback with specific suggestions for improvement."
temperature = 0.1
input_mode = "last"
output_mode = "none"  # Return only the review results (cleanest for tool use)

[agents.mcp]
server_refs = ["developer", "filesystem"]
allowed_tools = ["text_editor", "list_files"]
```

#### Output Mode Control

The `output_mode` setting controls what the agent tool returns:

- **`"none"`**: Returns only the final layer output (cleanest for tool use) - **Recommended**
- **`"append"`**: Returns layer output + session messages (for debugging)
- **`"replace"`**: Returns layer output (same as none for agents)
- **`"last"`**: Returns only the last layer output
- **`"restart"`**: Returns only the last layer output (same as last for agents)

**Best Practice**: Use `output_mode = "none"` for clean tool responses that integrate well with other MCP tools.

#### Usage Examples

Once configured, each agent becomes a separate tool:

**Context Gatherer Agent:**
```bash
# In session
agent_context_gatherer(task="Analyze the authentication system architecture and gather all relevant files and patterns")
```

**Code Review Agent:**
```bash
# In session
agent_code_reviewer(task="Review this function for performance issues and suggest improvements")
```

#### Tool Parameters

Each agent tool has the same parameter structure:

**Parameters:**
- `task` (string, required): Task description in human language for the agent to process

#### Key Features

- **Unified Configuration**: Agents use the same `LayerConfig` structure as commands and layers
- **Individual Tools**: Each agent becomes a separate MCP tool (e.g., `agent_context_gatherer`)
- **Output Control**: `output_mode` setting controls what the agent tool returns
- **Isolated Processing**: Each agent runs in its own session context
- **Tool Access**: Agents can use MCP tools based on their MCP configuration
- **Required Description**: Description field is required and used as MCP function description
- **Flexible**: Easy to add new specialized agents with complete layer configuration

#### call_llm - Direct LLM Call Tool

The `call_llm` tool enables direct LLM calls with runtime parameters, bypassing agent configuration:

**Parameters:**
- `prompt` (string, required): The input/prompt to process
- `model` (string, required): Model in 'provider:model' format (e.g., 'openai:gpt-4o', 'openrouter:anthropic/claude-sonnet-4')
- `system` (string, required): System prompt for the LLM
- `temperature` (number, optional): Temperature for randomness (0.0-2.0, default: 0.7)

**Usage Examples:**
```json
// Basic call
{"prompt": "Explain quantum computing", "model": "openai:gpt-4o", "system": "You are a helpful assistant"}

// With temperature for creative output
{"prompt": "Write a poem", "model": "openrouter:anthropic/claude-sonnet-4", "system": "You are a creative writer", "temperature": 1.2}
```

**Note:** Response size is controlled by global `mcp_response_tokens_threshold` setting.

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
- **Remove lines**: Use empty `new_str` ("") to remove lines completely
- **Refactoring workflow**: Extract code with `extract_lines`, then remove original with `line_replace` + empty `new_str`

**extract_lines** - Extract lines from source file and append to target file
```json
{"from_path": "src/utils.rs", "from_range": [10, 25], "append_path": "src/extracted.rs", "append_line": -1}
```
- **Parameters**:
  - `from_path`: Source file to extract from
  - `from_range`: [start, end] line numbers (1-indexed, inclusive)
  - `append_path`: Target file (auto-created if needed)
  - `append_line`: Insert position (0=beginning, -1=end, N=after line N)
- **Perfect for refactoring**: Move code blocks between files without modifying source

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

**Note**: `extract_lines` is not supported in batch operations as it's a standalone tool for file-to-file extraction.

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

[[mcp.servers]]
name = "web"
type = "builtin"
timeout_seconds = 30
args = []
tools = []  # Empty means all tools enabled

# External HTTP server example
[[mcp.servers]]
name = "web_search"
type = "http"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "optional_token"
timeout_seconds = 30
tools = []

# External command-based server example
[[mcp.servers]]
name = "local_tools"
type = "stdin"
command = "python"
args = ["-m", "my_mcp_server", "--port", "8008"]
timeout_seconds = 30
tools = ["custom_tool1", "custom_tool2"]  # Only these tools enabled
```

### Role-Based Server Access

Roles reference servers from the main MCP configuration and can limit tool access:

```toml
# Developer role with full access
[developer.mcp]
server_refs = ["developer", "filesystem", "web"]
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

- **developer**: Built-in development tools
  - `shell`: Terminal command execution with foreground/background support
  - `ast_grep`: AST-based code search and refactoring using ast-grep (sg)
  - `agent`: Task routing to specialized AI layers
- **filesystem**: Built-in file operations
  - `view`: Read files, view directories, and search file content
  - `text_editor`: Comprehensive file editing with batch operations
- **web**: Built-in web tools
  - `web_search`: Web search using Brave Search API
  - `image_search`, `video_search`, `news_search`: Specialized search tools
  - `read_html`: HTML to Markdown conversion
- **external**: External MCP servers (HTTP or command-based)


### External MCP Servers

#### HTTP-based Servers
```toml
[[mcp.servers]]
name = "web_tools"
type = "http"
url = "https://api.example.com/mcp"
auth_token = "your_token"
timeout_seconds = 30
tools = []
```

#### Command-based Servers
```toml
[[mcp.servers]]
name = "custom_tools"
type = "stdin"
command = "python"
args = ["/path/to/mcp_server.py"]
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

- **Input Mode**: How the layer receives input (`last`, `all`)
- **Output Mode**: How the layer affects the session (`none`, `append`, `replace`)
- **Model Selection**: Specific model for this layer
- **MCP Tools**: Which tools the layer can access
- **Custom Prompts**: Layer-specific system prompts

#### Output Modes Explained

- **`none`**: Intermediate layer that doesn't modify the session (like task_refiner)
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
name = "task_refiner"
model = "openrouter:openai/gpt-4.1-mini"
temperature = 0.2
input_mode = "Last"
output_mode = "none"  # Intermediate layer - doesn't modify session
builtin = true

[layers.mcp]
server_refs = []
allowed_tools = []

[[layers]]
name = "task_researcher"
model = "openrouter:google/gemini-2.5-flash-preview"
temperature = 0.2
input_mode = "Last"
output_mode = "replace"  # Replaces input with processed context
builtin = true

[layers.mcp]
server_refs = ["developer", "filesystem", "octocode"]
allowed_tools = ["list_files"]

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
model = "openrouter:anthropic/claude-sonnet-4"
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

### Session Commands for Workflows

- `/workflow [name]` - Execute workflows (list available with /workflow)
- `/done` - Manually trigger context optimization
- `/info` - View token usage by layer



## Token Management

### Smart Session Continuation System

Octomind features an advanced session continuation system that automatically preserves context when token limits are reached, using AI-driven file context selection.

#### Architecture

The continuation system uses a **modular architecture** with focused components:

- **`src/session/chat/continuation/`**: Core continuation system modules
  - `mod.rs`: Main module coordinator with public API re-exports
  - `detection.rs`: Continuation trigger logic and state checks
  - `injection.rs`: Summary request injection when limits reached
  - `processing.rs`: Response processing with user visibility
  - `file_context.rs`: File parsing, context generation, and tests
  - `constants.rs`: All prompts and message templates
- **`src/session/chat/session_continuation.rs`**: Compatibility layer - re-exports continuation API
- **`src/session/chat/response.rs`**: Integration point for continuation checks
- **`src/session/chat/context_truncation.rs`**: Continuation-aware context management
- **`src/session/chat/session/core.rs`**: Session state management with continuation tracking

#### How It Works

1. **Token Monitoring**: Every response processing checks against `max_session_tokens_threshold`
2. **Structured Summary**: AI receives a detailed prompt requesting:
   - Task objective and progress summary
   - Current state and next actions
   - Required file contexts in exact format: `filename:startline:endline`
3. **File Context Processing**: System parses AI response using regex `([^\s:]+):(\d+):(\d+)`
4. **Context Preservation**: Reads specified files with 1-indexed line numbers
5. **Session Reset**: Continues with preserved summary and file context

#### Configuration

```toml
# Smart continuation threshold (0 = disabled, >0 = enabled)
max_session_tokens_threshold = 20000

# The system automatically handles:
# - Summary request injection
# - File context parsing and reading
# - Session reset with preserved context
# - Visual feedback and error handling
```

#### File Context Format

The AI must specify required files using this exact format:
```
src/config/mod.rs:95:105
src/session/chat/response.rs:264:280
src/session/chat/session_continuation.rs:1:50
```

The system automatically:
- Parses these specifications using regex pattern matching
- Reads the specified line ranges (1-indexed)
- Includes formatted file content with line numbers
- Handles missing files gracefully with error messages

#### Advanced Features

**Error Resilience:**
- Graceful handling of missing or unreadable files
- Regex parsing with comprehensive error checking
- Fallback to original truncation if continuation fails

**Performance Optimization:**
- Maximum 10 file contexts per continuation
- Line limits to prevent excessive content (10k lines per file)
- Efficient file reading with range specification

**Integration Points:**
- Works during any token-consuming operation (user input, tool calls, etc.)
- Integrates with existing cache and cost tracking systems
- Maintains session state consistency across continuations

### Automatic Token Management

```toml
[developer]
# Warn when tool outputs exceed threshold
mcp_response_warning_threshold = 20000

# Smart session continuation when limit reached (0 = disabled)
max_session_tokens_threshold = 50000

# Cache management
cache_tokens_pct_threshold = 40
```

### Session Token Commands

- `/cache` - Mark cache checkpoint for cost savings
- `/info` - Display token usage and cost breakdown

## Smart Adaptive Compression System

Octomind features an intelligent compression system that automatically reduces conversation context when token usage grows, while maintaining cost-effectiveness through cache-aware decision making and discourse-aware semantic chunking.

### Architecture

The compression system is implemented across multiple modules:

**Core Compression:**
- **`src/session/chat/conversation_compression.rs`**: Main compression engine with cache-aware economics, semantic chunking, and AI-driven decision making
- **`src/session/cache.rs`**: Cache marker management system (2-marker system for cost optimization)
- **`src/mcp/dev/plan/compression.rs`**: Plan-specific compression for structured task workflows

**Supporting Systems:**
- **`src/session/chat/semantic_chunking.rs`**: Discourse-aware semantic chunking for preserving conversation structure
- **`src/session/chat/session/commands/cache.rs`**: `/cache` command implementation for manual cache checkpoints
- **`src/session/chat/token_counter.rs`**: Unified token counting for all message fields (content, tool_calls, thinking, images)

### How Compression Works

Compression operates through three key mechanisms:

#### 1. Token-Based Triggers

Unlike pressure-ratio systems, Octomind uses **absolute token count thresholds**:

```
Token Count → Compression Trigger
50,000 tokens → 2.0x compression (50% reduction)
100,000 tokens → 4.0x compression (75% reduction)
150,000 tokens → 8.0x compression (87.5% reduction)
```

The system monitors `session.get_full_context_tokens(config)` which includes:
- All conversation messages
- System prompt
- Tool definitions
- Safety margin for response generation

#### 2. Cache-Aware Decision Making

Before compressing, the system calculates if compression saves money by analyzing the net benefit:

```rust
// Actual calculation from calculate_compression_net_benefit()
net_benefit = (
    // Cost of current turn with full context
    current_tokens * current_model_cost

    // Plus: cost of remaining turns with full context
    + estimated_remaining_turns * current_tokens * current_model_cost

    // Minus: cost of compression (AI decision + summary)
    - compression_cost

    // Minus: cost of cache invalidation (forced rewrite at 1.25x)
    - (compressed_tokens * 1.25 * current_model_cost)

    // Plus: savings from smaller context in future turns
    + estimated_remaining_turns * (current_tokens - compressed_tokens) * current_model_cost

    // Plus: cache read savings (0.1x cost for cached content)
    + estimated_remaining_turns * compressed_tokens * 0.1 * current_model_cost
)

if net_benefit > 0.0 {
    compress()  // Saves money
} else {
    skip()      // Would cost money
}
```

**Cost Analysis Factors:**

- **Cache Write Cost**: 1.25x base token cost (Anthropic 5-minute TTL standard)
- **Cache Read Cost**: 0.1x base token cost (90% savings on cached content)
- **Compression Cost**: AI decision + summarization (typically 2-3k tokens)
- **Cache Invalidation**: Compression invalidates existing cache, forcing rewrite at 1.25x cost
- **Future Savings**: Smaller context = lower costs for all remaining conversation turns

**Future Turn Estimation:**

The system estimates remaining conversation turns using `estimate_future_turns()` with velocity-based analysis:

```
// Bootstrap phase (< 5 API calls)
if current_api_calls < 5:
    return 10.0  // Conservative baseline

// Calculate session metrics
session_duration_mins = (current_time - session_start) / 60
call_velocity = current_api_calls / session_duration_mins  // calls per minute

// Estimate remaining time based on session age
continuation_factor = {
    < 10 min:  0.8  // Early session: 80% more time likely
    < 30 min:  0.6  // Mid session: 60% more time likely
    >= 30 min: 0.4  // Long session: 40% more time (winding down)
}
estimated_remaining_mins = session_duration_mins * continuation_factor

// Apply velocity decay (sessions slow down over time)
velocity_decay = {
    > 2.0 calls/min: 0.6   // High velocity: expect 40% slowdown
    > 1.0 calls/min: 0.75  // Medium velocity: expect 25% slowdown
    <= 1.0 calls/min: 0.85 // Low velocity: expect 15% slowdown
}

// Calculate future calls
estimated_remaining = call_velocity * estimated_remaining_mins * velocity_decay

// Apply data-driven bounds
max_estimate = min(current_api_calls * 2.0, 100.0)
final_estimate = clamp(estimated_remaining, 5.0, max_estimate)
```

**Key characteristics:**
- Uses actual session velocity (calls/minute) instead of arbitrary assumptions
- Accounts for session lifecycle (early/mid/late phases)
- Applies realistic decay factors (sessions slow down over time)
- Bounded by historical data: minimum 5 calls, maximum 2x current calls or 100 calls
- Bootstrap phase uses conservative 10-call estimate until sufficient data available

This velocity-based approach ensures compression decisions reflect actual session behavior and account for the full cost-benefit over the session's expected lifetime.

#### 3. Discourse-Aware Semantic Chunking

Compression uses semantic chunking to preserve important information:

- **Preserves last 4 turns uncompressed**: Maintains recent context continuity
- **Semantic grouping**: Groups related messages for coherent compression
- **Importance weighting**: Prioritizes recent and tool-related messages
- **Discourse flow**: Maintains conversation structure and reasoning chains

### Configuration

```toml
[compression]
# Enable compression hints
hints_enabled = true
hints_pressure_threshold = 0.7
hints_min_interval = 5

# Enable adaptive token-based compression
adaptive_threshold = true

# Compression triggers at these token thresholds
[[compression.pressure_levels]]
threshold = 50000
target_ratio = 2.0  # Light: 50% reduction

[[compression.pressure_levels]]
threshold = 100000
target_ratio = 4.0  # Medium: 75% reduction

[[compression.pressure_levels]]
threshold = 150000
target_ratio = 8.0  # Aggressive: 87.5% reduction

# Optional: Use cheaper model for compression decisions
# Recommended: "openrouter:anthropic/claude-haiku" (10x cheaper)
# decision_model = "openrouter:anthropic/claude-haiku"
```

### Compression in Action

#### Example 1: Profitable Compression

```
Session state: 95,000 tokens
Threshold matched: 100,000 (target_ratio: 4.0x)

Cache-aware analysis:
  Current tokens: 95,000
  Estimated remaining turns: 5

  Without compression:
    5 turns × 95,000 tokens = 475,000 tokens
    Cost: 475,000 × $0.003 = $1.425

  With compression:
    Cache invalidation: 95,000 × 0.0025 = $0.2375
    Compressed size: 95,000 ÷ 4 = 23,750 tokens
    5 turns × 23,750 tokens = 118,750 tokens
    Cost: 118,750 × $0.003 = $0.3563
    Total: $0.2375 + $0.3563 = $0.5938

  Net benefit: $1.425 - $0.5938 = $0.8312 ✓ COMPRESS
```

#### Example 2: Skipped Compression

```
Session state: 55,000 tokens
Threshold matched: 50,000 (target_ratio: 2.0x)

Cache-aware analysis:
  Current tokens: 55,000
  Estimated remaining turns: 1

  Without compression:
    1 turn × 55,000 tokens = 55,000 tokens
    Cost: 55,000 × $0.003 = $0.165

  With compression:
    Cache invalidation: 55,000 × 0.0025 = $0.1375
    Compressed size: 55,000 ÷ 2 = 27,500 tokens
    1 turn × 27,500 tokens = 27,500 tokens
    Cost: 27,500 × $0.003 = $0.0825
    Total: $0.1375 + $0.0825 = $0.22

  Net benefit: $0.165 - $0.22 = -$0.055 ✗ SKIP (would cost money)
```

### Monitoring Compression

Use `/info` command to see compression statistics:

```
Compression Statistics:
  Total compressions: 3
  Average reduction: 72.5%
  Total tokens saved: 45,000
  Cost saved: $0.045

  Last compression:
    Before: 98,500 tokens
    After: 24,625 tokens (4.0x compression)
    Cost saved: $0.0225
```

### Compression Statistics in /info

The `/info` command displays:

```
Session Cost Report:
  ...
  Compression Statistics:
    Total compressions: 2
    Average reduction: 65%
    Tokens saved: 18,000
    Cost saved: $0.054
```

### Best Practices

1. **Monitor effectiveness**: Use `/info` to verify compression is saving money
2. **Use decision models**: Set `decision_model` to cheaper model for significant savings
3. **Adjust thresholds**: Start conservative (50k), adjust based on your workflow
4. **Preserve context**: Compression preserves last 4 turns for continuity
5. **Combine with caching**: Use `/cache` alongside compression for maximum savings

### Troubleshooting Compression

**Compression not triggering:**
- Verify `adaptive_threshold = true`
- Check `pressure_levels` array is not empty
- Use `/info` to see current token count vs. thresholds

**Compression too aggressive:**
- Lower `target_ratio` values (e.g., 2.0 instead of 4.0)
- Increase `threshold` values (e.g., 75,000 instead of 50,000)

**Compression not saving money:**
- Enable `decision_model` for cheaper decisions
- Increase thresholds to compress less frequently
- Consider disabling if sessions are short

### Integration with Session Continuation

Compression and continuation work together:

1. **Continuation** preserves context when token limits reached
2. **Compression** reduces context size to prevent future continuations
3. **Combined effect**: Longer sessions with lower costs

Example flow:
```
Session grows → Compression triggers → Context reduced
Session grows again → Compression triggers again → Context reduced further
Session reaches limit → Continuation preserves summary + file context
```

## Advanced Configuration Patterns

### Multi-Provider Setup
```toml
# Use different providers for different purposes
[developer]
model = "openrouter:anthropic/claude-sonnet-4"  # Main development
# Layer models are configured in individual [[layers]] sections
# See the layers configuration examples above for model assignments

[assistant]
model = "openrouter:anthropic/claude-3.5-haiku"  # Lightweight chat
```

### Role-Specific Tool Access
```toml
# Security-focused role
[security-reviewer]
model = "openrouter:anthropic/claude-sonnet-4"
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
allowed_tools = ["text_editor", "read_html"]  # Only doc-related tools
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
type = "http"
url = "https://mcp.so/server/web-dev-tools"
timeout_seconds = 30
tools = []
```
### OAuth 2.1 + PKCE Authentication for External Servers

HTTP MCP servers can be secured with OAuth 2.1 + PKCE (Proof Key for Code Exchange) authentication:

```toml
# HTTP MCP server with OAuth 2.1 + PKCE authentication
[[mcp.servers]]
name = "github_mcp"
type = "http"
url = "https://api.github.com/mcp"
timeout_seconds = 30
tools = []

# OAuth configuration
[mcp.servers.oauth]
client_id = "your-oauth-client-id"
client_secret = "your-oauth-client-secret"
authorization_url = "https://github.com/login/oauth/authorize"
token_url = "https://github.com/login/oauth/access_token"
callback_url = "http://localhost:34567/oauth/callback"
scopes = ["repo", "read:org"]
```

**How OAuth Flow Works:**
1. When Octomind connects to the server, it initiates OAuth flow
2. User is directed to authorization URL in browser
3. After authorization, token is exchanged and stored
4. Subsequent requests use the OAuth token automatically

**Benefits:**
- Secure authentication without storing credentials
- User-controlled authorization
- Automatic token refresh
- Support for multiple OAuth providers


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
- **Batch operations**: Use `view` for reading multiple files, `batch_edit` for modifying multiple files
- **Specific patterns**: Use `view` with patterns to filter results
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

#### Workflow Performance Issues
```bash
# Monitor workflow performance
/info

# List available workflows
/workflow

# Optimize context
/done
```

```

#### Token Limit Issues
```bash
# Mark cache checkpoint
/cache

# Check current usage
/info

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
type = "builtin"
timeout_seconds = 30
args = []
tools = []

[developer.mcp]
server_refs = ["developer"]
allowed_tools = []
```

### Provider Migration
**Old format:**
```toml
model = "anthropic/claude-sonnet-4"
```

**New format:**
```toml
model = "openrouter:anthropic/claude-sonnet-4"
```

Octomind automatically migrates legacy configurations, but manual updates provide better control and understanding of the new simplified structure.

## Structured Output (JSON Schema)

Octomind supports enforcing structured JSON output from the AI by passing a JSON Schema file via the `--schema` flag. This is useful for automation, pipelines, and any scenario where you need machine-readable, validated output.

### How It Works

When `--schema` is provided, Octomind passes the schema to the provider using the native structured output API (e.g., OpenAI's `response_format`, Anthropic's tool-based JSON mode). The AI is constrained to respond with JSON that matches the schema. The response is returned as raw JSON — markdown rendering is automatically disabled.

Strict mode is always enabled: the provider will reject responses that don't conform to the schema.

### Supported Commands

| Command | Flag | Notes |
|---------|------|-------|
| `octomind ask` | `--schema <path>` | Stateless single query |
| `octomind run` | `--schema <path>` | Non-interactive session run |
| `octomind session` | `--schema <path>` | Interactive session (all responses structured) |

### Provider Compatibility

Not all providers support structured output. Octomind fails fast with a clear error if the selected provider/model does not support it:

```
Provider 'cloudflare' does not support structured output for model 'llama-3.1-8b-instruct'.
Remove --schema or use a compatible provider.
```

Providers with known structured output support:
- **OpenAI** — `openai:gpt-4o`, `openai:gpt-4o-mini`, and other recent models
- **Anthropic** — `anthropic:claude-*` models (tool-based JSON mode)
- **OpenRouter** — depends on the underlying model; check provider docs

### Usage Examples

**Define a schema** (`schema.json`):
```json
{
  "type": "object",
  "properties": {
    "summary": { "type": "string" },
    "issues": {
      "type": "array",
      "items": { "type": "string" }
    },
    "severity": { "type": "string", "enum": ["low", "medium", "high"] }
  },
  "required": ["summary", "issues", "severity"],
  "additionalProperties": false
}
```

**Single query with structured output:**
```bash
octomind ask "Review this code for security issues" \
  --files src/auth.rs \
  --schema schema.json \
  --model openai:gpt-4o
```

**Output** (raw JSON, no markdown):
```json
{
  "summary": "The auth module has two potential vulnerabilities.",
  "issues": ["SQL injection in login query", "Missing rate limiting"],
  "severity": "high"
}
```

**Non-interactive run with structured output:**
```bash
octomind run developer "Analyze the codebase and list all TODO items" \
  --schema todos_schema.json \
  --format jsonl
```

**Interactive session with structured output:**
```bash
octomind session --schema schema.json --role assistant
```

### Pipeline Integration

Structured output is ideal for CI/CD pipelines:

```bash
# Extract structured analysis and pipe to jq
result=$(octomind ask "Summarize recent changes" \
  --schema analysis_schema.json \
  --model openai:gpt-4o)

severity=$(echo "$result" | jq -r '.severity')
if [ "$severity" = "high" ]; then
  echo "High severity issues found — blocking merge"
  exit 1
fi
```

### Implementation Notes

- Schema is loaded once at startup and validated as JSON before use
- `structured_output` field in `ProviderResponse` carries the parsed JSON value
- When structured output is present, it takes precedence over `content` for display
- Layers and context reduction calls always use `schema: None` — only the top-level session/ask uses the schema
- Continuation calls propagate the schema so multi-turn structured sessions work correctly
