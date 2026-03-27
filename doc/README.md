# Octomind Manual

Welcome to the comprehensive Octomind documentation. This manual provides detailed guidance on Octomind's session-first AI development assistant with built-in MCP tools and multi-provider support.

> **📝 Note**: The main README focuses on quick start and core features. This documentation provides comprehensive guides for installation, configuration, and advanced usage.

## Table of Contents
- [01-installation.md](./01-installation.md) — Installation methods, prerequisites, and development setup
- [02-overview.md](./02-overview.md) — Architecture, session-first design, roles, and core concepts
- [03-configuration.md](./03-configuration.md) — Configuration system, templates, compression, and customization
- [04-providers.md](./04-providers.md) — AI providers, model formats, token counting, caching, and cost tracking
- [05-sessions.md](./05-sessions.md) — Interactive sessions, commands, compression, and workflow
- [06-advanced.md](./06-advanced.md) — MCP protocol, tools, compression system, multimodal vision, and extensibility
- [07-command-layers.md](./07-command-layers.md) — Layered processing, custom commands, and agents
- [08-mcp-server-development.md](./08-mcp-server-development.md) — MCP server development and protocol compliance
- [09-websocket-server.md](./09-websocket-server.md) — WebSocket API for programmatic access
- [10-workflows.md](./10-workflows.md) — Brain-inspired MAP planning system for complex multi-step tasks
- [11-acp-editor-integration.md](./11-acp-editor-integration.md) — ACP editor integration: Neovim, Zed, JetBrains setup

## Current Architecture Overview

**Octomind** implements a session-first architecture with the following core components:

### Session-First Design
- **Interactive AI Conversations**: All functionality accessed through natural language sessions
- **Persistent Context**: Smart context management with automatic compression when limits reached
- **Role-Based Access**: Developer (full tools), Assistant (chat-only), and custom role configurations
### Built-in MCP Tools
- **Core Server**: `plan()`, `mcp()`, `agent()`, `schedule()`, `skill()` - Structured task management, dynamic server and agent management, scheduled messages, skill management
- **Filesystem Server**: `view()`, `text_editor()`, `batch_edit()`, `extract_lines()`, `shell()`, `workdir()`, `ast_grep()` - File operations, command execution, and code analysis
- **Agent Server**: `agent_*()` - Specialized AI task routing (delegates to configured ACP sub-agents or in-process dynamic agents)

### Multi-Provider AI Support
- **7 Providers**: OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek
- **Unified Interface**: Consistent `provider:model` format across all providers
- **Smart Caching**: Automatic cost optimization with cache markers
- **Vision Support**: Multimodal capabilities across all supported providers

### Advanced Features
- **Conversation Compression**: Automatic context compression when token limits approached
- **Cost Tracking**: Real-time usage monitoring with detailed reporting
- **Layered Processing**: AI pipeline system for complex task decomposition
- **MAP Planning**: Brain-inspired Modular Agentic Planner architecture for multi-step tasks
- **Template-Based Configuration**: All defaults in `config-templates/default.toml`, including MAP planning template at `config-templates/map-planner.toml`
### Webhook Integration
- **HTTP Webhook Hooks**: External systems trigger actions via HTTP webhooks
- **Script Processing**: Webhook payloads processed through custom scripts
- **Session Injection**: Script output injected as user messages
- **Environment Context**: Rich metadata available to hook scripts
### Unified Inbox System
- **Session-Scoped Queues**: Each session has isolated message queue
- **Multiple Sources**: Schedule, BackgroundAgent, Skill, Inject, Webhook
- **Thread-Safe**: RwLock<HashMap<SessionId, InboxQueue>> for concurrent access
- **Async Wakeup**: Arc<Notify> for efficient session loop integration


- **Structured Output**: `--schema` flag enforces JSON Schema-validated responses for pipelines and automation

### Smart Adaptive Compression System
- **Token-Based Triggers**: Compression activates at absolute token count thresholds (50k, 100k, 150k)
- **Cache-Aware Economics**: Calculates net benefit before compressing, considering cache invalidation costs
- **Discourse-Aware Chunking**: Preserves important information while reducing context size
- **Automatic Decision Making**: Skips compression if it would cost money, only compresses when profitable
- **Compression Statistics**: `/info` command shows compression metrics, tokens saved, and cost savings
- **Last 4 Turns Preservation**: Maintains recent context uncompressed for continuity

### Unified Token Calculation System
- **Single Source of Truth**: `estimate_full_context_tokens()` used by compression and display
- **Accurate Estimation**: Includes system prompt, tool definitions, and safety margins
- **Consistent Reporting**: All systems use same token counting for reliable cost tracking
- **Cost Breakdown**: `/info` shows detailed token usage and cost per request
### Tool Execution Animation
- **Visual Feedback**: Animated indicator during tool execution showing progress
- **User Experience**: Clear indication that tools are running and system is responsive
- **Parallel Tool Support**: Animation works correctly with parallel tool execution

### Conversation Compression System
- **Token-Based Triggers**: Compression activates at absolute token count thresholds (50k, 100k, 150k)
- **Cache-Aware Economics**: Calculates net benefit before compressing, considering cache invalidation costs
- **Discourse-Aware Chunking**: Preserves important information while reducing context size
- **Automatic Decision Making**: Skips compression if it would cost money, only compresses when profitable
- **Compression Statistics**: `/info` command shows compression metrics, tokens saved, and cost savings
- **Last 4 Turns Preservation**: Maintains recent context uncompressed for continuity
### MCP Tool System
- **Built-in Servers**: Core, Filesystem, and Agent servers with comprehensive tool sets
- **Protocol Compliance**: Full MCP standard implementation with proper error handling
- **Tool Routing**: Efficient tool-to-server mapping for instant execution
- **Health Monitoring**: Server health checks and automatic recovery
- **External Server Support**: HTTP and stdin-based external MCP servers
### Multi-Provider AI Integration
- **7 Providers Supported**: OpenRouter, OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek
- **Unified Model Format**: Consistent `provider:model` syntax across all providers
- **Vision Capabilities**: Multimodal support with automatic format detection
- **Cost Optimization**: Smart caching with provider-specific cache support
- **Retry Logic**: Robust error handling with exponential backoff

### Role-Based Configuration
- **Developer Role**: Full tool access with optimized system prompts for development tasks
- **Assistant Role**: Chat-only mode with limited tool access for general assistance
- **Custom Roles**: Flexible role definition with specific tool permissions and configurations
- **Layer Integration**: Role-specific layer processing for specialized AI workflows

### Advanced Session Management
- **Context Filtering**: `/context` command with multiple filter options (all, assistant, user, tool, large)
- **Cost Tracking**: Real-time usage monitoring with detailed per-session and per-request reporting
- **Image Support**: `/image` command with intelligent file completion and format detection
- **Session Persistence**: Automatic session saving with resume capabilities
- **Model Compatibility**: Automatic vision support detection for current model

## Quick Reference

### Installation
```bash
# One-line install (recommended)
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Set API key
export OPENROUTER_API_KEY="your_key"

# Start session
octomind run
```

### Essential Commands
```bash
# Configuration
octomind config                    # Generate default config
octomind config --show            # View current settings

# Sessions
octomind run                       # Developer session (full tools)
octomind run assistant             # Chat-only session
octomind run developer:rust        # Registry agent (fetches manifest)

# Background agents (daemon mode)
octomind run --name myagent --daemon --format plain   # Start daemon
echo "task" | octomind send --name myagent              # Send message

# Within sessions
/help                             # Show available commands
/info                             # Token usage and costs
/image <path>                     # Attach images for AI analysis
/mcp list                         # List active MCP servers
/plan                             # View current task plan
```

### Key Architecture Concepts

#### **Session-First Design**
Everything happens within interactive AI conversations. No separate indexing, configuration files are optional, and all functionality is accessed through natural language.

#### **MCP Tool Integration**
Built-in servers provide development tools:
- **Core**: `plan()`, `mcp()`, `agent()`, `schedule()`, `skill()` - Structured task management, dynamic server and agent management, scheduled messages, skill management
- **Filesystem**: `view()`, `text_editor()`, `batch_edit()`, `extract_lines()`, `shell()`, `workdir()`, `ast_grep()` - File operations, command execution, and code analysis
- **Agent**: `agent_*()` - Specialized AI task routing


#### **Role-Based Configuration**
- **Developer Role**: Full tool access, optimized for development tasks
- **Assistant Role**: Chat-only mode for general assistance
- **Custom Roles**: Define specific tool permissions and configurations

#### **Multi-Provider AI**
Unified interface supporting 7 providers with consistent `provider:model` format:
- OpenRouter (multi-provider access)
- OpenAI, Anthropic, Google, Amazon, Cloudflare, DeepSeek




#### **Layered Architecture**
Multi-stage AI processing for complex tasks:
- Query Processor → Context Generator → Developer → (Optional Reducer)

### Configuration Hierarchy

```
Environment Variables
    ↓
Role-specific config [developer] / [assistant] / [custom-role]
    ↓
Global config [providers] / [mcp]
    ↓
Default values
```

**Role Inheritance**: Custom roles inherit from assistant role, then apply overrides

### Supported Providers

| Provider | Format | Features |
|----------|--------|----------|
| OpenRouter | `openrouter:provider/model` | Multi-provider access, caching, vision models |
| OpenAI | `openai:model-name` | Direct API, cost calculation, GPT-4o vision |
| Anthropic | `anthropic:model-name` | Claude models, caching, Claude 3+ vision |
| Google | `google:model-name` | Vertex AI, Gemini 1.5+ vision support |
| Amazon | `amazon:model-name` | Bedrock models, AWS integration, Claude vision |
| Cloudflare | `cloudflare:model-name` | Edge AI, fast inference, Llama 3.2 vision |
| DeepSeek | `deepseek:model-name` | Cost-effective models, competitive performance |

### File Structure

```
.octomind/
├── config.toml          # Configuration file
├── sessions/            # Session history
└── logs/               # Debug logs
```

## Getting Help

### Documentation Navigation
- **[Installation](./01-installation.md)** - Setup methods, prerequisites, and development environment
- **[Overview](./02-overview.md)** - Architecture, core concepts, and session-first design
- **[Configuration](./03-configuration.md)** - Configuration system, templates, and customization
- **[Providers](./04-providers.md)** - AI provider setup, model formats, and cost optimization
- **[Sessions](./05-sessions.md)** - Interactive sessions, commands, and workflow
- **[Advanced Features](./06-advanced.md)** - MCP tools, multimodal vision, and extensibility
- **[Command Layers](./07-command-layers.md)** - Layered processing and custom commands
- **[MCP Development](./08-mcp-server-development.md)** - Tool development and protocol compliance
- **[WebSocket API](./09-websocket-server.md)** - Programmatic access and HTTP API
- **[MAP Workflows](./10-workflows.md)** — Brain-inspired MAP planning system for complex multi-step tasks
- **[ACP Editor Integration](./11-acp-editor-integration.md)** — Use Octomind inside Neovim, Zed, and JetBrains via ACP
### Support Resources
- **GitHub Issues**: [Report bugs and request features](https://github.com/muvon/octomind/issues)
- **GitHub Discussions**: [Community support and questions](https://github.com/muvon/octomind/discussions)
- **Main Repository**: [Source code and releases](https://github.com/muvon/octomind)

### Session Help Commands
```bash
# Within any session
/help                    # Show available commands
/info                    # Display token usage and costs
/mcp info               # Check MCP server status
/loglevel debug         # Enable detailed logging
```

---

**Octomind** - Session-first AI development assistant with built-in MCP tools and multi-provider support.

**© 2026 Muvon Un Limited** | Apache License 2.0
