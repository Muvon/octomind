# OctoDev - Smart Codebase Assistant

OctoDev is a command-line tool that helps developers navigate and understand their codebase using semantic search capabilities. It analyzes your code files, indexes their content, and allows you to search using natural language queries to find relevant code snippets across your project.

## Features

- **Semantic Code Search**: Find code by meaning rather than just keywords
- **Natural Language Queries**: Search your codebase with plain English questions
- **Multiple Language Support**: Works with Rust, PHP, Python, JavaScript, TypeScript, JSON, Go, C++, Bash, and Ruby
- **Symbol Awareness**: Understands code structure and can expand symbol references
- **Live File Watching**: Automatically updates the index when your code changes
- **Configurable Embedding Providers**: Works with either FastEmbed (offline) or Jina (cloud) for embeddings
- **MCP Protocol Support**: Integrates with external MCP servers for additional tools and capabilities

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
octodev session --model anthropic/claude-3.5-sonnet
```

#### Session Commands

While in an interactive session, you can use the following commands:

- `/help` - Show help for all available commands
- `/exit` or `/quit` - Exit the session
- `/list` - List all available sessions
- `/session [name]` - Switch to another session or create a new one (empty creates fresh session)
- `/clear` - Clear the screen
- `/save` - Save the current session
- `/cache` - Mark a cache checkpoint for token saving

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

### MCP Configuration

OctoDev supports the Model-Centric Programming (MCP) protocol, which allows integration with both local tools and external MCP servers. You can configure MCP in your `.octodev/config.toml` file:

```toml
[mcp]
enabled = true
providers = ["shell"]

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
providers = ["shell"]

[[mcp.servers]]
enabled = true
name = "WebSearch"
command = "python"  # Or any other command to start your server
args = ["-m", "websearch_server", "--port", "8008"]
```

3. OctoDev will start the server process when needed and clean it up when the program exits.

#### Current MCP Providers

- **shell**: Allows the AI to run shell commands in your terminal (enabled by adding "shell" to providers list)
- **External MCP Servers**: Any MCP-compatible server can be added in the `[[mcp.servers]]` section

## How It Works

OctoDev uses a combination of techniques to build a searchable index of your codebase:

1. **Tree-sitter Parsing**: Analyzes code syntax to extract meaningful blocks and symbols
2. **Vector Embeddings**: Converts code blocks to numerical vectors capturing semantic meaning
3. **SurrealDB Database**: Stores and retrieves embeddings for efficient similarity search
4. **Symbol Tracking**: Maintains relationships between code symbols for reference expansion

When you search, OctoDev converts your natural language query into the same vector space and finds the closest matching code blocks.

## Troubleshooting

### Common Issues

- **Slow Indexing**: For large codebases, initial indexing may take some time, especially when downloading models for the first time.
- **Missing Dependencies**: Make sure you have the required Rust version (use rustup to update if needed).
- **Storage Path**: Data is stored in the `.octodev/storage` directory using SurrealDB's RocksDB backend.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

MIT
