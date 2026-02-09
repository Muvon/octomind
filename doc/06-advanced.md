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
- `text_editor(command="view|create|str_replace|insert|line_replace|undo_edit|view_many", path="...", ...)` - Comprehensive file operations
- `list_files(directory="...", pattern="...", content="...", ...)` - Directory listing with filtering and content search
- `batch_edit(path="...", operations=[...])` - Multiple file operations atomically
- `extract_lines(from_path="...", from_range=[start, end], append_path="...", append_line=N)` - Extract and move code blocks
- `semantic_search(query="...", ...)` - Semantic code search using descriptive queries
- `view_signatures(files=[...])` - Extract function signatures and class definitions
- `graphrag(operation="...", ...)` - Advanced relationship-aware code analysis

**Memory Server** (`src/mcp/memory/`):
- `memorize(title="...", content="...", ...)` - Store important information for future reference
- `remember(query="...", limit=5, ...)` - Search and retrieve stored memories
- `forget(confirm=true, query="...", ...)` - Permanently remove specific memories

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

See `src/mcp/dev/plan/` for code, and test integration in `src/session/chat/session/runner.rs`.
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

#### semantic_search — Semantic Code Search

Search codebase using semantic search to find relevant code snippets by describing what the code does, not exact symbol names:

```json
// Find authentication patterns
{"query": ["user authentication flow", "login validation", "jwt token handling"]}

// Find database patterns
{"query": ["database connection pooling", "query result caching"], "max_results": 5}
```

**Parameters:**
- `query` (string or array, required): Descriptive search terms about functionality (not symbol names)
- `max_results` (integer, default: 3): Maximum number of results to return
- `mode` (string, default: "all"): Scope of search - "code", "text", "docs", or "all"
- `language` (string, optional): Filter by programming language
- `detail_level` (string, default: "partial"): "signatures", "partial", or "full"
- `threshold` (number, 0.0-1.0): Similarity threshold for results

**Best Practices:**
- Use descriptive phrases about functionality: "user authentication" not "login_user"
- Multiple related terms improve results: ["database patterns", "query optimization"]
- Use "signatures" mode for quick overview, "full" for complete implementations

#### view_signatures — Extract Code Structure

Extract and view function signatures, class definitions, and other meaningful code structures:

```json
{"files": ["src/main.rs", "src/lib.rs"]}
{"files": ["**/*.py"], "max_tokens": 2000}
```

**Parameters:**
- `files` (array, required): File paths or glob patterns to analyze
- `max_tokens` (integer, default: 2000): Maximum tokens in output before truncation

**Supported Languages:**
Rust, JavaScript, TypeScript, Python, Go, C++, PHP, Ruby, Bash, JSON, CSS, Svelte, Markdown

#### graphrag — Relationship-Aware Code Analysis

Advanced GraphRAG operations for understanding code relationships and architecture:

```json
{"operation": "search", "query": "How does user authentication flow through the system?"}
{"operation": "get-node", "node_id": "src/main.rs"}
{"operation": "get-relationships", "node_id": "src/auth/mod.rs"}
{"operation": "find-path", "source_id": "src/main.rs", "target_id": "src/db/mod.rs"}
{"operation": "overview", "max_depth": 3}
```

**Parameters:**
- `operation` (string, required): "search", "get-node", "get-relationships", "find-path", or "overview"
- `query` (string, required for search): Semantic query about code functionality
- `node_id` (string, required for get-node/get-relationships): File path or file/symbol path
- `source_id` (string, required for find-path): Starting node identifier
- `target_id` (string, required for find-path): Target node identifier
- `max_depth` (integer, default: 3): Maximum path depth for find-path/overview
- `max_tokens` (integer, default: 2000): Maximum tokens allowed in output

**Use Cases:**
- `search`: Find files by describing what they do
- `get-node`: Get detailed information about a specific file or symbol
- `get-relationships`: See what components depend on or are related to a file
- `find-path`: Trace connection paths between two components
- `overview`: Get graph statistics and structure

**Pattern Syntax Examples:**

*JavaScript/TypeScript:*
- Function calls: `console.log($$$)`, `$OBJ.$METHOD($$$)`
- Functions: `function $NAME($ARGS) { $$$ }`
- Arrow functions: `($ARGS) => $BODY`
- Variables: `const $VAR = $VALUE`

*PHP:*
- Function calls: `$NAME($$$)`
- Method calls: `$OBJ->$METHOD($$$)`
- Classes: `class $NAME { $$$ }`

*Rust:*
- Macros: `println!($$$)`
- Functions: `fn $NAME($ARGS) { $$$ }`
- Structs: `struct $NAME { $$$ }`

#### Memory Tools (type: "builtin")
- **memorize**: Store important information, insights, or context for future reference
- **remember**: Search and retrieve stored memories using semantic search
- **forget**: Permanently remove specific memories

#### Filesystem Tools (type: "builtin")
- **text_editor**: Read, write, edit files with multiple operations (view, create, str_replace, insert, line_replace, undo_edit, view_many, batch_edit)
- **extract_lines**: Extract lines from source file and append to target file without modifying source (perfect for refactoring)
- **list_files**: Browse directory structures with pattern matching and content search
- **semantic_search**: Search codebase using semantic search to find relevant code snippets by describing functionality
- **view_signatures**: Extract and view function signatures, class definitions, and other meaningful code structures
- **graphrag**: Advanced relationship-aware GraphRAG operations for code analysis (search, get-node, get-relationships, find-path, overview)

#### Memory Tools (type: "builtin")
- **memorize**: Store important information, insights, or context for future reference with title, content, importance, and tags
- **remember**: Search and retrieve stored memories using semantic search with limit and memory_type filters
- **forget**: Permanently remove specific memories by ID or query matching

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
allowed_tools = ["text_editor", "list_files", "semantic_search", "view_signatures"]

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
- `model` (string, required): Model in 'provider:model' format (e.g., 'openai:gpt-4o', 'openrouter:anthropic/claude-3.5-sonnet')
- `system` (string, required): System prompt for the LLM
- `temperature` (number, optional): Temperature for randomness (0.0-2.0, default: 0.7)

**Usage Examples:**
```json
// Basic call
{"prompt": "Explain quantum computing", "model": "openai:gpt-4o", "system": "You are a helpful assistant"}

// With temperature for creative output
{"prompt": "Write a poem", "model": "openrouter:anthropic/claude-3.5-sonnet", "system": "You are a creative writer", "temperature": 1.2}
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
  - `text_editor`: Comprehensive file editing with batch operations
  - `list_files`: Directory browsing with pattern matching and content search
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

- **`src/session/chat/continuation/`**: **NEW MODULAR STRUCTURE**
  - `mod.rs`: Main module coordinator with public API re-exports
  - `detection.rs`: Continuation trigger logic and state checks
  - `injection.rs`: Summary request injection when limits reached
  - `processing.rs`: Response processing with **DISPLAY FIXES** for user visibility
  - `file_context.rs`: File parsing, context generation, and tests
  - `constants.rs`: All prompts and message templates
- **`src/session/chat/session_continuation.rs`**: **LEGACY COMPATIBILITY** - re-exports new API
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

The compression system is implemented in:
- **`src/session/chat/conversation_compression.rs`**: Main compression logic with cache-aware decision making
- **`src/mcp/dev/plan/compression.rs`**: Plan-specific compression for structured tasks
- **`src/session/token_counter.rs`**: Unified token counting used by compression decisions
- **`src/session/chat/session/core.rs`**: Integration with ChatSession for token estimation

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

Before compressing, the system calculates if compression saves money:

```rust
// Pseudocode of cache-aware analysis
net_benefit = calculate_compression_net_benefit(
    current_tokens,
    target_ratio,
    estimated_remaining_turns
)

if net_benefit > 0.0 {
    compress()  // Saves money
} else {
    skip()      // Would cost money
}
```

**Cost Analysis Factors:**
- Cache write cost: 1.25x base (Anthropic 5-minute TTL)
- Cache read cost: 0.1x base (90% savings)
- Compression invalidates cache, forcing rewrite
- Smaller context = lower costs for future turns

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
