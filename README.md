# Octodev - Smart Codebase Assistant

Octodev is a command-line tool that helps developers navigate and understand their codebase using semantic search capabilities and AI-powered assistance. It analyzes your code files, indexes their content, and allows you to search using natural language queries to find relevant code snippets across your project.

## Features

- **Semantic Code Search**: Find code by meaning rather than just keywords
- **Natural Language Queries**: Search your codebase with plain English questions
- **Multiple Language Support**: Works with Rust, PHP, Python, JavaScript, TypeScript, JSON, Go, C++, Bash, and Ruby
- **Symbol Awareness**: Understands code structure and can expand symbol references
- **Live File Watching**: Automatically updates the index when your code changes
- **Configurable Embedding Providers**: Works with either FastEmbed (offline) or Jina (cloud) for embeddings
- **Multi-Provider AI Support**: Works with OpenRouter, OpenAI, and more (extensible architecture)
- **AI-Powered Code Assistance**: Helps you understand and modify your codebase
- **Optimized Multi-layered Architecture**: Uses specialized AI models for different aspects of code assistance
- **Detailed Cost and Token Tracking**: Tracks usage by layer and optimizes token consumption
- **MCP Protocol Support**: Integrates with external MCP servers for additional tools and capabilities
- **Context Management**: Automatic context truncation to stay within token limits
- **Token Protection**: Warnings and confirmations for potentially costly operations
- **Interruptible Processing**: Ctrl+C instantly cancels operations for better user control
- **Enhanced Tool Output Rendering**: Improved display and handling of tool outputs with better formatting and user control
- **MCP Server Registry**: New centralized server registry approach that reduces configuration duplication
- **Additional AI Providers**: Support for Amazon Bedrock and Cloudflare Workers AI
- **Improved Configuration Management**: Better global configuration with centralized provider settings

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

## AI Providers

Octodev supports multiple AI providers through an extensible architecture. You can use different providers and models by specifying them in the `provider:model` format.

### Supported Providers

#### OpenRouter (Default)
- **Models**: All OpenRouter models (Anthropic, OpenAI, Google, etc.)
- **API Key**: Set `OPENROUTER_API_KEY` environment variable or configure in `.octodev/config.toml`
- **Features**: Full tool support, caching (for Claude models), cost tracking

#### OpenAI
- **Models**: GPT-4, GPT-3.5, O1, and other OpenAI models
- **API Key**: Set `OPENAI_API_KEY` environment variable
- **Features**: Full tool support, built-in cost calculation

#### Anthropic
- **Models**: Claude 3.5, Claude 3, Claude 2, and Claude Instant models
- **API Key**: Set `ANTHROPIC_API_KEY` environment variable
- **Features**: Full tool support, built-in cost calculation, caching support

#### Google Vertex AI
- **Models**: Gemini 1.5, Gemini 1.0, and Bison models
- **Authentication**: Service account authentication (see setup below)
- **Features**: Full tool support, built-in cost calculation

#### Amazon Bedrock
- **Models**: Claude, Llama, Command, and other Bedrock models
- **Authentication**: AWS credentials (access key/secret key)
- **Features**: Full tool support, built-in cost calculation, AWS integration

#### Cloudflare Workers AI
- **Models**: Llama, Mistral, and other Workers AI models
- **Authentication**: Cloudflare API token and account ID
- **Features**: Edge AI inference, fast response times, cost-effective

### Model Format

All models must now be specified with the `provider:model` format:

```bash
# OpenRouter models
octodev session --model "openrouter:anthropic/claude-3.5-sonnet"
octodev session --model "openrouter:openai/gpt-4o"

# OpenAI models (direct)
octodev session --model "openai:gpt-4o"
octodev session --model "openai:o1-preview"

# Anthropic models (direct)
octodev session --model "anthropic:claude-3-5-sonnet"
octodev session --model "anthropic:claude-3-opus"

# Google Vertex AI models
octodev session --model "google:gemini-1.5-pro"
octodev session --model "google:gemini-1.5-flash"

# Amazon Bedrock models
octodev session --model "amazon:claude-3-5-sonnet"
octodev session --model "amazon:llama-3.1-70b-instruct"

# Cloudflare Workers AI models
octodev session --model "cloudflare:llama-3.1-8b-instruct"
octodev session --model "cloudflare:mistral-7b-instruct"
```

### Configuration

Configure providers in your `.octodev/config.toml`:

```toml
# Centralized provider configuration (recommended)
[providers.openrouter]
api_key = "your_openrouter_key"  # Optional, can use env var

[providers.openai]
api_key = "your_openai_key"

[providers.anthropic]
api_key = "your_anthropic_key"

[providers.amazon]
region = "us-east-1"
access_key_id = "your_access_key"
secret_access_key = "your_secret_key"

[providers.cloudflare]
account_id = "your_account_id"
api_token = "your_api_token"

# Role-specific model configuration
[developer]
model = "openrouter:anthropic/claude-sonnet-4"

[assistant]
model = "openai:gpt-4o-mini"  # Use OpenAI for assistant mode

# Legacy configuration (still supported)
[openrouter]
model = "openrouter:anthropic/claude-sonnet-4"
api_key = "your_openrouter_key"  # Optional, can use env var
```

### Environment Variables

Set the appropriate API keys:

```bash
# For OpenRouter
export OPENROUTER_API_KEY="your_openrouter_key"

# For OpenAI  
export OPENAI_API_KEY="your_openai_key"

# For Anthropic
export ANTHROPIC_API_KEY="your_anthropic_key"

# For Google Vertex AI (requires service account setup)
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/service-account.json"
export GOOGLE_PROJECT_ID="your-gcp-project-id"
export GOOGLE_REGION="us-central1"  # Optional, defaults to us-central1

# For Amazon Bedrock
export AWS_ACCESS_KEY_ID="your_access_key"
export AWS_SECRET_ACCESS_KEY="your_secret_key"
export AWS_REGION="us-east-1"  # Optional, defaults to us-east-1

# For Cloudflare Workers AI
export CLOUDFLARE_API_TOKEN="your_api_token"
export CLOUDFLARE_ACCOUNT_ID="your_account_id"
```

#### Google Vertex AI Setup

Google Vertex AI requires service account authentication:

1. **Create a Service Account** in Google Cloud Console
2. **Download the JSON key file**
3. **Set environment variables**:
   ```bash
   export GOOGLE_APPLICATION_CREDENTIALS="/path/to/your/service-account.json"
   export GOOGLE_PROJECT_ID="your-project-id"
   ```
4. **Enable the Vertex AI API** in your Google Cloud project

Note: The Google provider currently requires additional OAuth2 implementation for full functionality.

#### Amazon Bedrock Setup

Amazon Bedrock requires AWS credentials:

1. **Create AWS IAM user** with Bedrock access permissions
2. **Generate access keys** for the IAM user
3. **Set environment variables**:
   ```bash
   export AWS_ACCESS_KEY_ID="your_access_key"
   export AWS_SECRET_ACCESS_KEY="your_secret_key"
   export AWS_REGION="us-east-1"
   ```
4. **Enable model access** in the Bedrock console for the models you want to use

#### Cloudflare Workers AI Setup

Cloudflare Workers AI requires API credentials:

1. **Get your Account ID** from the Cloudflare dashboard
2. **Create an API token** with Workers AI permissions
3. **Set environment variables**:
   ```bash
   export CLOUDFLARE_ACCOUNT_ID="your_account_id"
   export CLOUDFLARE_API_TOKEN="your_api_token"
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

Octodev includes an AI coding assistant with two distinct modes that can help you understand and modify your codebase:

```bash
# Start a new interactive session in developer role (default)
octodev session

# Start in assistant role for simple conversation
octodev session --role=assistant

# Start with a specific name (or resume if exists)
octodev session -n my_session

# Resume an existing session
octodev session -r my_session

# Use a specific model with provider
octodev session --model "openai:gpt-4o"
octodev session --model "openrouter:anthropic/claude-sonnet-4"
octodev session --model "anthropic:claude-3-5-sonnet"
octodev session --model "google:gemini-1.5-pro"

# Combine options
octodev session --role=assistant --model="openai:gpt-4o-mini" -n chat_session
```

#### Session Roles

Octodev supports flexible session roles for different use cases, with two defaults provided:

**Developer Role (Default)** - Full development environment:
- Complete codebase indexing and analysis
- All development tools enabled (file operations, shell commands, code search)
- Project context collection (README, git info, file structure)
- Layered architecture support enabled by default for complex tasks
- Complex developer-focused system prompts
- File watching for code changes

**Assistant Role** - Simple conversation:
- No codebase indexing (faster startup)
- Tools disabled by default (configurable)
- Simple assistant system prompts
- Direct model interaction (layers disabled by default)
- Lighter resource usage

**Custom Roles** - Extensible system:
- Any custom role can be defined in the configuration
- All custom roles inherit from the assistant role as a base
- Custom configurations override the inherited settings
- Use `--role=your-custom-role` to use any configured role

#### Role Configuration

Each role can be configured independently with its own model, tool settings, and behavior. Roles follow an inheritance pattern where custom roles inherit from the assistant role first, then apply their own overrides:

```toml
# Global MCP configuration (fallback for all roles)
[mcp]
enabled = true

[[mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"

[[mcp.servers]]
enabled = true
name = "filesystem"
server_type = "filesystem"

# Developer role configuration (inherits from global MCP by default)
[developer]
model = "openrouter:anthropic/claude-sonnet-4"
enable_layers = true
system = "You are an Octodev AI developer assistant with full access to development tools."

# Assistant role configuration (tools disabled by default)
[assistant]
model = "openrouter:anthropic/claude-3.5-haiku"  # Faster/cheaper model
enable_layers = false
system = "You are a helpful assistant."

[assistant.mcp]
enabled = false  # Override global MCP to disable tools

# Custom role configuration (inherits from assistant, then applies overrides)
[my-custom-role]
model = "openrouter:openai/gpt-4o"
enable_layers = true
system = "You are a specialized assistant for my specific use case."

[my-custom-role.mcp]
enabled = true  # Enable tools for this custom role

[[my-custom-role.mcp.servers]]
enabled = true
name = "developer"
server_type = "developer"
tools = ["shell", "text_editor"]  # Limit to specific tools
```

#### Layered Architecture

Octodev's first message in each session uses a specialized 3-layer AI architecture for enhanced code understanding and modification:

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
- `/model [model]` - Show current model or change to a different model
- `/clear` - Clear the screen
- `/save` - Save the current session
- `/cache` - Mark a cache checkpoint for token saving
- `/done` - Optimize the session context and restart the layered processing for the next message
- `/layers` - Toggle layered processing architecture on/off
- `/truncate [threshold]` - Toggle automatic context truncation when token limit is reached
- `/info` - Display detailed token and cost breakdowns by layer
- `/debug` - Toggle debug mode for detailed logs

#### Session Caching

Octodev supports token caching with providers like OpenRouter to save costs when reusing large prompts or context. The system prompt is automatically cached, and you can mark user messages for caching with the `/cache` command.

### Watch Mode

Start a watcher that automatically reindexes when files change:

```bash
octodev watch
```

### Configuration

Octodev uses a configuration file stored in `.octodev/config.toml` in your project directory. You can create or modify this using the `config` command:

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

Octodev supports two embedding providers:

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

Octodev's layered architecture can be configured in your `.octodev/config.toml` file:

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

Octodev supports the Model-Centric Programming (MCP) protocol, which allows integration with both local tools and external MCP servers. The configuration has been significantly improved with a new server registry approach that reduces duplication and provides better organization.

#### New Server Registry Approach

The MCP system now uses a centralized server registry where servers are defined once and referenced by roles and commands. This eliminates configuration duplication and provides better maintainability.

#### Configuration Hierarchy

```
[role.mcp] → [global.mcp] → [mcp_server_registry] → defaults
```

#### New Server Registry Configuration

```toml
# MCP Server Registry - Define servers once, reference everywhere
[mcp_server_registry]

# Built-in servers (defined by default but can be customized)
[mcp_server_registry.developer]
enabled = true
name = "developer"
server_type = "developer"
tools = []  # Empty means all tools enabled

[mcp_server_registry.filesystem]
enabled = true
name = "filesystem"
server_type = "filesystem"
tools = []  # Empty means all tools enabled

# External server example
[mcp_server_registry.web_search]
enabled = true
name = "web_search"
server_type = "external"
url = "https://api.example.com/mcp/websearch"
auth_token = "your-auth-token"  # Optional
tools = []  # Empty means all tools enabled

# Role configurations now reference servers from registry
[developer.mcp]
enabled = true
server_refs = ["developer", "filesystem"]  # Reference servers by name
allowed_tools = []  # Empty means all tools from referenced servers

[assistant.mcp]
enabled = true
server_refs = ["filesystem"]  # Only filesystem tools
allowed_tools = ["text_editor", "list_files"]  # Limit to specific tools

# Global MCP fallback
[mcp]
enabled = true
server_refs = ["developer", "filesystem"]  # Default servers
```

#### Legacy Configuration Support

The old server-based configuration is still supported for backward compatibility, but the new server registry approach is recommended:

**Legacy format (still supported):**
```toml
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

**New registry format (recommended):**
```toml
# Define once in registry
[mcp_server_registry.developer]
enabled = true
name = "developer"
server_type = "developer"

# Reference from roles
[developer.mcp]
enabled = true
server_refs = ["developer", "filesystem"]
```

#### External Server Configuration

External servers are now configured in the server registry:

```toml
# External HTTP server
[mcp_server_registry.web_search]
enabled = true
name = "web_search"
server_type = "external"
url = "https://mcp.so/server/webSearch-Tools"
auth_token = "your_token_if_needed"  # Optional
mode = "http"
timeout_seconds = 30
tools = []  # Empty means all tools enabled

# Local MCP server - Running as a local process
[mcp_server_registry.local_tools]
enabled = true
name = "local_tools"
server_type = "external"
command = "python"  # Command to execute
args = ["-m", "websearch_server", "--port", "8008"]
mode = "stdin"  # Communication mode: "http" or "stdin"
timeout_seconds = 30
tools = ["custom_tool1", "custom_tool2"]  # Only these tools enabled
```

#### Setting up a Local MCP Server

You can run an MCP server locally by providing the command and arguments to execute:

1. Create a `.octodev/config.toml` file if you don't have one (or run `octodev config`)
2. Add a local MCP server configuration to the server registry:

```toml
# MCP Server Registry
[mcp_server_registry.developer]
enabled = true
name = "developer"
server_type = "developer"

[mcp_server_registry.web_search]
enabled = true
name = "web_search"
server_type = "external"
command = "python"  # Or any other command to start your server
args = ["-m", "websearch_server", "--port", "8008"]
mode = "stdin"
timeout_seconds = 30

# Reference the server in your role configuration
[developer.mcp]
enabled = true
server_refs = ["developer", "web_search"]
```

3. Octodev will start the server process when needed and clean it up when the program exits.

#### Server Types

- **developer**: Built-in developer tools (shell commands, code search, file operations)
- **filesystem**: Built-in filesystem tools (file reading, writing, listing)
- **external**: External MCP servers (HTTP or command-based)

#### Migration from Legacy Configuration

The MCP configuration has evolved through several iterations:

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
server_type = "developer"
```

**New registry format (recommended):**
```toml
# Define servers once in registry
[mcp_server_registry.developer]
enabled = true
name = "developer"
server_type = "developer"

# Reference from roles
[developer.mcp]
enabled = true
server_refs = ["developer"]
```

The new server registry approach eliminates duplication when multiple roles or commands need the same servers.

## How It Works

Octodev uses a combination of techniques to build a searchable index of your codebase:

1. **Tree-sitter Parsing**: Analyzes code syntax to extract meaningful blocks and symbols
2. **Vector Embeddings**: Converts code blocks to numerical vectors capturing semantic meaning
3. **SurrealDB Database**: Stores and retrieves embeddings for efficient similarity search
4. **Symbol Tracking**: Maintains relationships between code symbols for reference expansion

For AI assistance, Octodev uses a specialized 4-layer architecture:

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

- **Slow Indexing**: For large codebases, initial indexing may take some time, especially when downloading models for the first time. Recent optimizations have reduced the number of indexed file types for better performance.
- **Missing Dependencies**: Make sure you have the required Rust version (use rustup to update if needed).
- **Storage Path**: Data is stored in the `.octodev/storage` directory using SurrealDB's RocksDB backend.
- **Token Limits**: If you encounter token limit issues, try:
  - Using the `/truncate` command to enable automatic context management
  - Setting a higher `max_request_tokens_threshold` in the config
  - Using `/cache` to mark system messages or large user inputs for caching
  - Using `/done` to optimize context between interactions
- **Large Tool Outputs**: When tools generate very large outputs, you'll be prompted to confirm. The system now provides better handling with:
  - **Enhanced rendering**: Improved display formatting for tool outputs
  - **User control**: Better prompts and options for handling large outputs
  - **Configurable thresholds**: Adjust the `mcp_response_warning_threshold` setting in your config
  - **Smart filtering**: Modify your tool-usage patterns to be more specific (e.g., limit file listings, be specific with file paths)
  - **Tool optimization**: Try using `grep` or other filtering tools to reduce output size
- **MCP Configuration Issues**: If you encounter MCP-related errors:
  - Ensure you're using the new server-based configuration format
  - Migrate from old `providers = ["core"]` to `[[mcp.servers]]` format
  - Check that server types are correctly specified (`developer`, `filesystem`, or `external`)
  - Verify external server URLs and commands are accessible

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

### Adding New AI Providers

Octodev uses an extensible provider architecture that makes it easy to add support for new AI providers. Here's how to add a new provider:

1. **Create the provider file**: Create `src/session/providers/your_provider.rs`
2. **Implement the AiProvider trait**:
   ```rust
   use super::{AiProvider, ProviderResponse};
   
   pub struct YourProvider;
   
   #[async_trait::async_trait]
   impl AiProvider for YourProvider {
       fn name(&self) -> &str { "your_provider" }
       fn supports_model(&self, model: &str) -> bool { /* your logic */ }
       async fn chat_completion(&self, ...) -> Result<ProviderResponse> { /* implementation */ }
       // ... other required methods
   }
   ```
3. **Register the provider**: Add it to `ProviderFactory::create_provider()` in `src/session/providers/mod.rs`
4. **Add to exports**: Include your provider in the module exports

The provider system handles:
- Model string parsing (`provider:model` format)
- Message format conversion
- Tool call integration
- Token usage tracking
- Error handling

Example providers to reference:
- `openrouter.rs` - Full-featured provider with caching and cost tracking
- `openai.rs` - Standard provider implementation

## Changelog

### Recent Updates (January 2025)

#### Major Features
- **MCP Server Registry**: New centralized server registry approach that eliminates configuration duplication
- **New AI Providers**: Added support for Amazon Bedrock and Cloudflare Workers AI
- **Enhanced Tool Output Rendering**: Improved display and handling of tool outputs with better formatting and user control
- **Centralized Provider Configuration**: New `[providers.*]` sections for better API key management

#### Improvements
- **Configuration Validation**: Better validation and error messages for configuration issues
- **File Indexing Optimization**: Reduced allowed text extensions for more focused indexing
- **Global Configuration Management**: Better system-wide configuration with proper inheritance
- **Backward Compatibility**: All legacy configuration formats remain supported

#### Migration Notes
- **MCP Configuration**: New server registry format is recommended but old formats still work
- **Provider Configuration**: New centralized `[providers.*]` sections are recommended
- **Model Format**: All models must use `provider:model` format (e.g., `openrouter:anthropic/claude-sonnet-4`)

## License

MIT