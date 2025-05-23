# OctoDev - Smart Codebase Assistant

OctoDev is a command-line tool that helps developers navigate and understand their codebase using semantic search capabilities and AI-powered assistance. It analyzes your code files, indexes their content, and allows you to search using natural language queries to find relevant code snippets across your project.

## Features

- **Semantic Code Search**: Find code by meaning rather than just keywords
- **Natural Language Queries**: Search your codebase with plain English questions
- **Multiple Language Support**: Works with Rust, PHP, Python, JavaScript, TypeScript, JSON, Go, C++, Bash, and Ruby
- **Symbol Awareness**: Understands code structure and can expand symbol references
- **Live File Watching**: Automatically updates the index when your code changes
- **Configurable Embedding Providers**: Works with either FastEmbed (offline) or Jina (cloud) for embeddings
- **AI-Powered Code Assistance**: Helps you understand and modify your codebase
- **Optimized Multi-layered Architecture**: Uses specialized AI models for different aspects of code assistance
- **Detailed Cost and Token Tracking**: Tracks usage by layer and optimizes token consumption
- **MCP Protocol Support**: Integrates with external MCP servers for additional tools and capabilities
- **Context Management**: Automatic context truncation to stay within token limits
- **Token Protection**: Warnings and confirmations for potentially costly operations
- **Interruptible Processing**: Ctrl+C instantly cancels operations for better user control

## Installation

### Prerequisites

- Rust and Cargo installed on your system
- No additional dependencies - embedded SurrealDB is used for storage

### Building from Source

```bash
# Clone the repository
git clone https://github.com/muvon/octodev.git
cd octodev

# Build the project
cargo build --release

# Optional: Add to your PATH
cp target/release/octodev /usr/local/bin/
```

## Usage

### Indexing Your Codebase

Before searching, you need to index your codebase:

```bash
# Index the current directory
octodev index

# Index a specific directory
octodev index /path/to/your/project
```

### Searching Your Codebase

Once indexed, you can search your codebase using natural language:

```bash
# Basic search
octodev search "how does authentication work"

# Search with expanded symbols (follows references)
octodev search --expand "user registration process"

# Get results in JSON format
octodev search --json "database connection setup"
```

### Interactive Sessions

OctoDev includes an AI coding assistant that can help you understand and modify your codebase:

```bash
# Start a new interactive session
octodev session

# Start with a specific name (or resume if exists)
octodev session -n my_session

# Resume an existing session
octodev session -r my_session

# Use a specific model
octodev session --model anthropic/claude-sonnet-4
```

#### Layered Architecture

OctoDev's first message in each session uses a specialized 3-layer AI architecture for enhanced code understanding and modification:

1. **Query Processor**: Analyzes and improves your initial query for clearer instructions (no tools)
2. **Context Generator**: Gathers all necessary code context using tools to explore the codebase
3. **Developer**: Executes the actual coding tasks and produces comprehensive responses using tools

The **Reducer** layer functionality is still available but now invoked manually through the `/done` command instead of automatically running after every interaction. This gives you control over when to optimize context.

After the first message, subsequent interactions go directly to the Developer model for more efficient conversation flow. You can explicitly request context optimization and restart the full layered pipeline at any time using the `/done` command.

This architecture brings several benefits:
- Improved task specialization with each layer focused on what it does best
- Optimized token usage through systematic context management
- Clearer responsibility boundaries between layers
- Better documentation maintenance with on-demand context reduction
- Cost efficiency by using simpler models for less complex tasks
- Enhanced tools utilization with tools available only to layers that need them

Enable layered processing with the `/layers` command in any session.

#### Session Commands

While in an interactive session, you can use the following commands:

- `/help` - Show help for all available commands
- `/exit` or `/quit` - Exit the session
- `/list` - List all available sessions
- `/session [name]` - Switch to another session or create a new one (empty creates fresh session)
- `/clear` - Clear the screen
- `/save` - Save the current session
- `/cache` - Mark a cache checkpoint for token saving
- `/done` - Optimize the session context and restart the layered processing for the next message
- `/layers` - Toggle layered processing architecture on/off
- `/truncate [threshold]` - Toggle automatic context truncation when token limit is reached
- `/info` - Display detailed token and cost breakdowns by layer
- `/debug` - Toggle debug mode for detailed logs

#### Session Caching

OctoDev supports token caching with providers like OpenRouter to save costs when reusing large prompts or context. The system prompt is automatically cached, and you can mark user messages for caching with the `/cache` command.

### Watch Mode

Start a watcher that automatically reindexes when files change:

```bash
octodev watch
```

### Configuration

OctoDev uses a configuration file stored in `.octodev/config.toml` in your project directory. You can create or modify this using the `config` command:

```bash
# Create default configuration
octodev config

# Set the embedding provider
octodev config --provider fastembed

# Configure Jina provider
octodev config --provider jina --jina-key YOUR_API_KEY

# Configure FastEmbed models
octodev config --fastembed-code-model all-MiniLM-L6-v2 --fastembed-text-model all-MiniLM-L6-v2
```

## Configuration Options

### Embedding Providers

OctoDev supports two embedding providers:

1. **FastEmbed** (default): Works offline, doesn't require API keys, but may have smaller context windows
2. **Jina**: Cloud-based, requires an API key, provides high-quality embeddings

### FastEmbed Models

Available models:
- `all-MiniLM-L6-v2` (default)
- `all-MiniLM-L12-v2`
- `multilingual-e5-small`
- `multilingual-e5-base`
- `multilingual-e5-large`

### Jina Models

Default models:
- Code: `jina-embeddings-v2-base-code`
- Text: `jina-embeddings-v3`

### Layered Architecture Configuration

OctoDev's layered architecture can be configured in your `.octodev/config.toml` file:

```toml
[openrouter]
model = "anthropic/claude-sonnet-4"  # Main model for Developer layer
enable_layers = true                   # Enable layered architecture

# Configure models for each layer (optional)
query_processor_model = "openai/gpt-4.1-nano"       # Model for query processing
context_generator_model = "openai/gpt-4.1-nano"     # Model for context gathering
developer_model = "anthropic/claude-sonnet-4" # Model for development tasks
reducer_model = "openai/gpt-4.1-nano"               # Model for context reduction

# Token management settings
mcp_response_warning_threshold = 20000        # Warn for large tool outputs (tokens)
max_request_tokens_threshold = 50000          # Max tokens before auto-truncation
enable_auto_truncation = false               # Auto context truncation setting
```

You can customize which model is used for each layer. If a specific layer model is not defined, it will use the main model specified in the `model` parameter. This allows you to optimize costs by using less expensive models for simpler tasks while reserving more powerful models for complex development work.

The token management settings help control costs and prevent token limits from being exceeded:
- `mcp_response_warning_threshold`: When an MCP tool (like shell commands or file operations) generates output larger than this threshold, the user will be prompted to confirm or reject the result.
- `max_request_tokens_threshold`: When context size exceeds this threshold and auto-truncation is enabled, older messages will be automatically trimmed.
- `enable_auto_truncation`: Toggle automatic context management (can also be toggled via the `/truncate` command).

### MCP Configuration

OctoDev supports the Model-Centric Programming (MCP) protocol, which allows integration with both local tools and external MCP servers. You can configure MCP in your `.octodev/config.toml` file:

```toml
[mcp]
enabled = true
providers = ["core"]

# External MCP server configuration - URL based
[[mcp.servers]]
enabled = true
name = "RemoteWebSearch"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "your_token_if_needed"  # Optional
tools = []  # Empty means all tools are enabled

# Local MCP server configuration - Running as a local process
[[mcp.servers]]
enabled = true
name = "LocalWebSearch"
command = "python"  # Command to execute
args = ["-m", "websearch_server", "--port", "8008"]  # Arguments to pass
tools = []  # Empty means all tools are enabled
```

#### Setting up a Local MCP Server

You can run an MCP server locally by providing the command and arguments to execute:

1. Create a `.octodev/config.toml` file if you don't have one (or run `octodev config`)
2. Add a local MCP server configuration:

```toml
[mcp]
enabled = true
providers = ["core"]

[[mcp.servers]]
enabled = true
name = "WebSearch"
command = "python"  # Or any other command to start your server
args = ["-m", "websearch_server", "--port", "8008"]
```

3. OctoDev will start the server process when needed and clean it up when the program exits.

#### Current MCP Providers

- **core**: Allows the AI to run shell commands, search code, and perform file operations in your terminal (enabled by adding "core" to providers list)
- **External MCP Servers**: Any MCP-compatible server can be added in the `[[mcp.servers]]` section

## How It Works

OctoDev uses a combination of techniques to build a searchable index of your codebase:

1. **Tree-sitter Parsing**: Analyzes code syntax to extract meaningful blocks and symbols
2. **Vector Embeddings**: Converts code blocks to numerical vectors capturing semantic meaning
3. **SurrealDB Database**: Stores and retrieves embeddings for efficient similarity search
4. **Symbol Tracking**: Maintains relationships between code symbols for reference expansion

For AI assistance, OctoDev uses a specialized 4-layer architecture:

```
User Input
    ↓
Query Processor (Improves the query, no tools)
    ↓
Context Generator (Gathers necessary information using tools)
    ↓
Developer (Implements solution and produces response using tools)
    ↓
Reducer (Updates documentation and optimizes context for next interaction)
    ↓
User Response
```

This architecture ensures optimal token usage and focused expertise at each stage of processing.

## Troubleshooting

### Common Issues

- **Slow Indexing**: For large codebases, initial indexing may take some time, especially when downloading models for the first time.
- **Missing Dependencies**: Make sure you have the required Rust version (use rustup to update if needed).
- **Storage Path**: Data is stored in the `.octodev/storage` directory using SurrealDB's RocksDB backend.
- **Token Limits**: If you encounter token limit issues, try:
  - Using the `/truncate` command to enable automatic context management
  - Setting a higher `max_request_tokens_threshold` in the config
  - Using `/cache` to mark system messages or large user inputs for caching
  - Using `/done` to optimize context between interactions
- **Large Tool Outputs**: When tools generate very large outputs, you'll be prompted to confirm. If you frequently encounter this:
  - Adjust the `mcp_response_warning_threshold` setting in your config
  - Modify your tool-usage patterns to be more specific (e.g., limit file listings, be specific with file paths)
  - Try using `grep` or other filtering tools to reduce output size

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT
