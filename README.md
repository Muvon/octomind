# Octomind 🤖 - AI-Powered Development Assistant

**© 2025 Muvon Un Limited** | [Complete Documentation](doc/README.md)

> **Session-based AI development assistant with conversational codebase interaction, multimodal vision support, built-in MCP tools, and multi-provider AI integration**

Octomind is a session-first AI development assistant that transforms how you interact with codebases through natural language conversations. Built on the Model Context Protocol (MCP), it provides seamless integration with development tools, multi-provider AI support, and intelligent cost optimization.

[![asciicast](https://asciinema.org/a/wpZmOSOgFXp8HRzTltncgN7e3.svg)](https://asciinema.org/a/wpZmOSOgFXp8HRzTltncgN7e3)

## ✨ Core Features

- 🎯 **Session-First Architecture** - Everything happens in interactive AI conversations with persistent context
- 🛠️ **Built-in MCP Tools** - File operations, code analysis, shell commands, web search via Model Context Protocol
- 🌐 **Multi-Provider AI Support** - OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek
- 🖼️ **Multimodal Vision Support** - Analyze images, screenshots, diagrams with AI vision capabilities
- 💰 **Cost Tracking & Optimization** - Real-time usage monitoring, caching, and detailed cost reporting
- 🧠 **Smart Adaptive Compression** - Automatic context reduction with cache-aware economics when tokens grow
- 🔧 **Role-Based Configuration** - Developer (full tools), Assistant (chat-only), and custom roles
- 🧠 **Smart Session Continuation** - Automatic context management when token limits are reached
- 🧠 **Brain-Inspired Workflows** - Multi-step planning system with validation, feedback loops, and conditional branching
- ⚡ **Tool Execution Animation** - Visual feedback during tool execution with progress indicators
- 🔍 **Reverse Search** - Ctrl+E to exit reverse search in interactive sessions


## 🚀 Quick Start

### Prerequisites
- **API Key** from supported AI provider

### Installation
```bash
# One-line install (recommended)
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Set your AI provider API key (choose one)
export OPENROUTER_API_KEY="your_key"     # Multi-provider access
export OPENAI_API_KEY="your_key"         # Direct OpenAI
export ANTHROPIC_API_KEY="your_key"      # Direct Anthropic

# Start your first session
octomind session
```

## 💬 How It Works

Octomind operates through interactive AI sessions with built-in development tools:

```
> "How does authentication work in this project?"
[AI analyzes project structure, finds auth-related files, explains implementation]

> "Add error handling to the login function"
[AI examines login code, implements error handling, shows changes]

> "Rename 'processData' to 'processUserData' across all files"
[AI finds all occurrences, performs batch edit across multiple files]

> /image screenshot.png
> "What's wrong with this UI layout?"
[AI analyzes the image, identifies layout issues, suggests CSS fixes]

> agent_context_gatherer(task="Analyze the authentication system architecture")
[Routes task to specialized context gathering AI agent with development tools]

> /report
[Shows: $0.02 spent, 3 requests, 5 tool calls, timing analysis]
```

### Built-in MCP Tools
- **Developer Tools**: `shell()`, `ast_grep()` - Execute commands and search code patterns
- **Filesystem Tools**: `text_editor()`, `list_files()`, `batch_edit()` - File operations
- **Web Tools**: `web_search()`, `read_html()` - Web research and content analysis
- **Agent Tools**: `agent_*()` - Route tasks to specialized AI processing layers

### Session Commands
- `/help` - Show available commands
- `/info` - Display token usage, costs, and compression statistics
- `/image <path>` - Attach images for AI analysis
- `/mcp info` - Check MCP server status
- `/model <model>` - Switch AI models
- `/role <role>` - Change role (developer/assistant)
- `/cache` - Add cache checkpoint for cost optimization
- `/context [filter]` - Display session context with optional filtering
- `/done` - Complete task with memorization and context preservation

## 🌐 Supported AI Providers

| Provider | Format | Features |
|----------|--------|----------|
| OpenRouter | `openrouter:provider/model` | Multi-provider access, caching, vision models |
| OpenAI | `openai:model-name` | Direct API, cost calculation, GPT-4o vision |
| Anthropic | `anthropic:model-name` | Claude models, caching, Claude 3+ vision |
| Google | `google:model-name` | Vertex AI, Gemini 1.5+ vision support |
| Amazon | `amazon:model-name` | Bedrock models, AWS integration, Claude vision |
| Cloudflare | `cloudflare:model-name` | Edge AI, fast inference, Llama 3.2 vision |
| DeepSeek | `deepseek:model-name` | Cost-effective models, competitive performance |

## 🛠️ Installation & Setup

### Prerequisites
- **Rust 1.82+** and Cargo
- **API Key** from supported AI provider

### Installation Options

```bash
# One-line install (recommended)
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Build from source (for development)
git clone https://github.com/muvon/octomind.git
cd octomind
cargo build --release

# Install via Cargo (when published)
cargo install octomind
```

### API Key Setup

Set your AI provider API key (choose one or more):

```bash
# Multi-provider access (recommended)
export OPENROUTER_API_KEY="sk-or-v1-..."

# Direct provider access
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
export GOOGLE_API_KEY="AIza..."
export AMAZON_ACCESS_KEY_ID="AKIA..."
export AMAZON_SECRET_ACCESS_KEY="..."
export CLOUDFLARE_API_TOKEN="..."
export DEEPSEEK_API_KEY="sk-..."

# Optional: Web search capability
export BRAVE_API_KEY="BSA..."
```

### First Run

```bash
# Generate default configuration (optional)
octomind config

# Start your first session
octomind session

# Within the session, try:
/help                    # Show all available commands
/info                    # Check token usage and costs
/mcp info               # Check MCP tool status
```

## 🎮 Session Commands

Essential commands for interactive sessions:

**Core Commands**
- `/help` - Show available commands
- `/info` - Display token usage and costs
- `/image <path>` - Attach images for AI analysis
- `/model [model]` - View or change AI model
- `/role [role]` - Change role (developer/assistant)

**Context Management**
- `/cache` - Add cache checkpoint for cost optimization
- `/context [filter]` - Display session context
- `/truncate` - Manually truncate context
- `/done` - Finalize task with memorization

**MCP Tools & Debugging**
- `/mcp info` - Check MCP server status
- `/workflow <name>` - Execute workflows
- `/run <command>` - Execute custom commands
- `/loglevel [level]` - Set logging level


**Session Management**
- `/save` - Save current session
- `/clear` - Clear terminal screen
- `/exit` - Exit session


## 🏗️ Architecture

**Session-First Design**: Everything happens in interactive AI conversations with persistent context and built-in development tools.

**Core Components:**
- **MCP Tools**: Built-in servers for development (shell, ast_grep), filesystem (text_editor, batch_edit), web (search, html), and agent routing
- **Multi-Provider AI**: Seamless switching between OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek
- **Role-Based Access**: Developer (full tools), Assistant (chat-only), and custom role configurations
- **Smart Caching**: Automatic cost optimization with cache markers and intelligent context management
- **Layered Processing**: AI pipeline system for complex task decomposition and specialized processing

## 🔧 Configuration

Octomind uses a template-based configuration system with smart defaults:

```bash
# Generate default config (optional)
octomind config

# View current settings
octomind config --show

# Validate configuration
octomind config --validate
```

**Configuration Features:**
- **Template-Based**: All defaults in `config-templates/default.toml`
- **Environment Overrides**: Any setting can be overridden with `OCTOMIND_*` variables
- **Role-Based**: Different configurations for developer/assistant/custom roles
- **MCP Integration**: Built-in and external MCP server configurations
- **Cost Controls**: Spending thresholds and performance tuning

## 📖 Documentation

📚 **[Complete Documentation](./doc/README.md)** - Comprehensive guides and references

### Quick Navigation
- **[Installation Guide](./doc/01-installation.md)** - Setup, prerequisites, and development
- **[Overview](./doc/02-overview.md)** - Architecture and core concepts
- **[Configuration Guide](./doc/03-configuration.md)** - Configuration system and customization
- **[AI Providers](./doc/04-providers.md)** - Provider setup and model selection
- **[Sessions Guide](./doc/05-sessions.md)** - Interactive sessions and commands
- **[Advanced Features](./doc/06-advanced.md)** - MCP tools and extensibility
- **[Command Layers & Workflows](./doc/07-command-layers.md)** - AI processing pipeline and brain-inspired planning
- **[Workflows](./doc/10-workflows.md)** - Multi-step planning system
- **[MCP Development](./doc/08-mcp-server-development.md)** - Tool development


## 🚀 Contributing

Contributions are welcome! Help make Octomind better for the development community.

**Development Setup:**
```bash
git clone https://github.com/muvon/octomind.git
cd octomind
cargo check --message-format=short    # Fast compilation check
cargo clippy --all-features --all-targets -- -D warnings  # Fix code quality
cargo build                           # Build when needed
```

**Development Areas:**
- **AI Providers**: Add new providers in `src/providers/`
- **MCP Tools**: Extend built-in tools in `src/mcp/`
- **Session Features**: Enhance session management in `src/session/`
- **Documentation**: Improve guides and examples

**Requirements:** Rust 1.82+, API key from supported providers

## 🆘 Troubleshooting

**Common Issues:**
- **Build Errors**: Use `cargo check --message-format=short` for fast syntax checking
- **Missing API Keys**: Set `OPENROUTER_API_KEY` or provider-specific keys
- **Invalid Model Format**: Use `provider:model` format (e.g., `openrouter:anthropic/claude-sonnet-4`)
- **MCP Tool Issues**: Check `/mcp info` for server status
- **Session Problems**: Use `/loglevel debug` for detailed logging

**Getting Help:**
- 🐛 **Issues**: [GitHub Issues](https://github.com/muvon/octomind/issues)
- 📖 **Documentation**: [Complete Documentation](./doc/README.md)
- 💬 **Discussions**: [GitHub Discussions](https://github.com/muvon/octomind/discussions)
- ✉️ **Email**: [opensource@muvon.io](mailto:opensource@muvon.io)

## 📞 Support & Contact

- **🏢 Company**: Muvon Un Limited (Hong Kong)
- **🌐 Website**: [muvon.io](https://muvon.io)
- **📦 Product Page**: [octomind.muvon.io](https://octomind.muvon.io)
- **📧 Email**: [opensource@muvon.io](mailto:opensource@muvon.io)
- **🐛 Issues**: [GitHub Issues](https://github.com/muvon/octomind/issues)

## ⚖️ License

**Apache License 2.0**
Copyright © 2025 Muvon Un Limited
