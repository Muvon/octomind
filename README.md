# Octomind 🤖 - AI-Powered Development Assistant

**© 2025 Muvon Un Limited** | [Complete Documentation](doc/README.md)

> **Transform your development workflow with AI conversations that understand your codebase**

Octomind is an AI-powered development assistant that helps you understand, analyze, and interact with your codebase through natural language conversations. No complex setup, no indexing—just intelligent AI sessions with built-in development tools.

[![asciicast](https://asciinema.org/a/wpZmOSOgFXp8HRzTltncgN7e3.svg)](https://asciinema.org/a/wpZmOSOgFXp8HRzTltncgN7e3)

## ✨ Why Octomind?

- 🎯 **Session-First Architecture** - Everything happens in interactive AI conversations
- 🛠️ **Built-in Development Tools** - File operations, batch editing, code analysis, shell commands via MCP
- 🌐 **Multi-Provider AI Support** - OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare
- 🖼️ **Multimodal Vision Support** - Analyze images, screenshots, diagrams with AI across all providers
- 💰 **Cost Tracking & Optimization** - Real-time usage monitoring with detailed reporting
- 🔧 **Role-Based Configuration** - Developer (full tools) and Assistant (chat-only) modes

## 🚀 Quick Start

```bash
# Install Octomind
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/main/install.sh | bash

# Set your AI provider API key
export OPENROUTER_API_KEY="your_key"

# Start coding with AI
octomind session
```

## 💬 How It Works

Instead of complex command-line tools, simply talk to Octomind:

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

> "Why is the build failing?"
[AI checks build errors, analyzes code, suggests fixes]

> agent_code_reviewer(task="Review this function for performance issues")
[Routes task to specialized code review AI agent with development tools]

> /report
[Shows: $0.02 spent, 3 requests, 5 tool calls, timing analysis]
```

## 🌐 Supported AI Providers

| Provider | Format | Features |
|----------|--------|----------|
| OpenRouter | `openrouter:provider/model` | Multi-provider access, caching, vision models |
| OpenAI | `openai:model-name` | Direct API, cost calculation, GPT-4o vision |
| Anthropic | `anthropic:model-name` | Claude models, caching, Claude 3+ vision |
| Google | `google:model-name` | Vertex AI, Gemini 1.5+ vision support |
| Amazon | `amazon:model-name` | Bedrock models, AWS integration, Claude vision |
| Cloudflare | `cloudflare:model-name` | Edge AI, fast inference, Llama 3.2 vision |

## 🛠️ Installation & Setup

### Installation Options

```bash
# One-line install (recommended)
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/main/install.sh | bash

# Build from source
cargo install --git https://github.com/muvon/octomind.git

# Manual download from releases
# See: https://github.com/muvon/octomind/releases
```

### Basic Setup

```bash
# Set your AI provider API key
export OPENROUTER_API_KEY="your_key"  # or OPENAI_API_KEY, ANTHROPIC_API_KEY, etc.

# Create configuration (optional - uses smart defaults)
octomind config

# Start your first session
octomind session
```

### Essential Commands

```bash
# Development session (full tools)
octomind session

# Chat-only session
octomind session --role=assistant

# Resume previous session
octomind session --resume my_session

# Use specific model
octomind session --model "openrouter:anthropic/claude-3.5-sonnet"
```

## 🎮 Session Commands

Within any session, use these commands:
- `/help` - Show available commands and features
- `/image <path>` - Attach image to your next message (PNG, JPEG, GIF, WebP, BMP)
- `/model [model]` - View or change current AI model
- `/info` - Display token usage and costs
- `/report` - Generate detailed usage report with cost breakdown
- `/cache` - Mark cache checkpoint for cost savings
- `/layers` - Toggle layered processing on/off
- `/reduce` - Compress session history using cheaper reducer model (manual context optimization)
- `/done` - Finalize task with memorization, comprehensive summarization, and auto-commit
- `/loglevel [debug|info|none]` - Set log level
- `/exit` - Exit current session

## 🎯 Context Management Commands

Octomind provides two distinct commands for managing conversation context:

### `/done` - Task Completion & Finalization
**Purpose**: Complete and finalize a development task (like `git commit` for conversations)
- **When to use**: When you've finished a task/feature and want to preserve the work
- **What it does**:
  - Creates comprehensive task summary with all file changes and technical details
  - Uses your current model (preserves quality and context understanding)
  - Memorizes critical information for future reference
  - Auto-commits changes with octocode if available
  - Preserves complete context for task continuation
- **Result**: Clean session start with rich task summary as context

### `/reduce` - Manual Context Optimization
**Purpose**: Reduce context size during ongoing work (cost optimization)
- **When to use**: When context is getting large but task isn't finished
- **What it does**:
  - Compresses session history using cheaper reducer model
  - Focuses on context size reduction, not task completion
  - No memorization or auto-commit
  - Quick optimization for continued work
- **Result**: Smaller context, lower costs, work continues

**Key Difference**: `/done` finalizes work with full context preservation, `/reduce` optimizes ongoing work for cost efficiency.

## 🔧 Configuration

Octomind uses a flexible configuration system with smart defaults. Configuration is optional for basic usage.

**View Configuration Template**: [`config-templates/default.toml`](config-templates/default.toml)

```bash
# Generate default config
octomind config

# Validate configuration
octomind config --validate

# View current settings
octomind config --show
```

**Key Configuration Features:**
- Environment variable precedence for security
- Role-based configurations (developer/assistant)
- MCP server registry for tool integration
- Cost thresholds and performance tuning

## 📖 Documentation

📚 **[Complete Documentation](./doc/README.md)** - Comprehensive guides and references

### Quick Navigation
- **[Installation Guide](./doc/01-installation.md)** - Detailed installation methods and building from source
- **[Overview](./doc/02-overview.md)** - Introduction and core concepts
- **[Configuration Guide](./doc/03-configuration.md)** - Configuration system, templates, and customization
- **[AI Providers](./doc/04-providers.md)** - Provider setup, API keys, and model selection
- **[Sessions Guide](./doc/05-sessions.md)** - Interactive sessions, commands, and workflow
- **[Advanced Features](./doc/06-advanced.md)** - MCP tools, layered architecture, and extensibility
- **[Command Layers](./doc/07-command-layers.md)** - Specialized AI helpers and command processing

## 🚀 Contributing

Contributions are welcome! We appreciate your help in making Octomind better.

**Development Areas:**
- **AI Providers**: Add new providers in `src/session/providers/`
- **MCP Tools**: Extend tool capabilities via MCP server registry
- **Documentation**: Improve guides and examples

```bash
# Development setup
git clone https://github.com/muvon/octomind
cd octomind
cargo build --release
cargo test
```

**Requirements:** Rust 1.70+, Cargo, API key from supported providers

## 🆘 Troubleshooting

**Common Issues:**
- **Configuration Errors**: Check system config directory or regenerate with `octomind config`
- **Missing API Keys**: Set environment variables for your AI provider
- **Invalid Model Format**: Use `provider:model` format (e.g., `openrouter:anthropic/claude-3.5-sonnet`)
- **Session Issues**: Use `/loglevel debug` to enable detailed logging

**Getting Help:**
- 🐛 **Issues**: [GitHub Issues](https://github.com/muvon/octomind/issues)
- 📖 **Documentation**: [Complete Documentation](./doc/README.md)
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
