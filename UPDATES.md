# OctoDev - Smart Codebase Assistant

## New Features and Changes

### OpenRouter Integration
- OpenRouter is now the default provider for AI interactions (no need for `--openrouter` flag)
- API key can be stored in configuration file (or still use OPENROUTER_API_KEY env var) 
- Model choice is configurable through config (`octodev config --openrouter-model "anthropic/claude-3-sonnet-20240229"`)

### Model Control Protocol (MCP) Support
- Added support for MCP tools, similar to Claude Sonnet's function calling
- Shell commands available as a tool when MCP is enabled
- Configure MCP with: `octodev config --mcp-enable true --mcp-providers "shell"`
- Tools are used automatically during sessions with results visible to the user
- External MCP servers can now be integrated for additional capabilities
- Web search and browsing functionality available through external MCP servers

### Configuration Updates
- Enhanced configuration system to include OpenRouter and MCP settings
- Use `octodev config` to see and modify all available options
- Configuration stored in `.octodev/config.toml`

## Usage Examples

### Setting Up Configuration
```bash
# Create default configuration
octodev config

# Set OpenRouter API key
octodev config --openrouter-key "your-api-key-here"

# Change model
octodev config --openrouter-model "anthropic/claude-3-opus-20240229"

# Enable MCP
octodev config --mcp-enable true --mcp-providers "shell"
```

### Starting a Session
```bash
# Start a new session (OpenRouter is now default)
octodev session

# Start with custom model
octodev session --model "anthropic/claude-3-haiku-20240307"
```

### Using MCP in Sessions
When MCP is enabled, the AI can use tools like shell commands automatically.
Example interaction:

```
> list files in the current directory

AI is using tools:
- Executing: shell

**Tool Call**: shell
**Result**:
{
  "success": true,
  "output": "Cargo.lock\nCargo.toml\nsrc\ntarget\n",
  "code": 0
}

AI: Here are the files in the current directory:
...
```

### Using External MCP Servers
You can integrate external MCP servers like the webSearch-Tools from mcp.so or run them locally.
Add to your `.octodev/config.toml`:

```toml
[mcp]
enabled = true
providers = ["shell"]

# Remote server
[[mcp.servers]]
enabled = true
name = "RemoteWebSearch"
url = "https://mcp.so/server/webSearch-Tools"

# Or local server
[[mcp.servers]]
enabled = true
name = "LocalWebSearch"
command = "python"
args = ["-m", "websearch_server", "--port", "8008"]
```

Example interaction with web search:

```
> What are the new features in Rust 1.75?

AI is using tools:
- Executing: webSearch

**Tool Call**: webSearch | web
**Result**:
{
  "success": true,
  "output": "[Search results for 'Rust 1.75 new features']",
  "parameters": {
    "query": "Rust 1.75 new features"
  }
}

AI: Rust 1.75, released on December 28, 2023, introduced several new features:
...
```