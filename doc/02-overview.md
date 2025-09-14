# Octomind Overview

## What is Octomind?

Octomind is a session-first AI development assistant that transforms how you interact with codebases through natural language conversations. Built on the Model Context Protocol (MCP), it provides seamless integration with development tools, multi-provider AI support, and intelligent cost optimization.

**Key Principles:**
- **Session-First**: Everything happens in interactive AI conversations
- **Tool Integration**: Built-in MCP tools for development, filesystem, web, and agent operations
- **Multi-Provider**: Unified interface across 7 AI providers
- **Cost Optimization**: Smart caching and real-time usage tracking
- **Role-Based**: Developer (full tools) and Assistant (chat-only) configurations

## Architecture Overview

Octomind implements a clean, session-first architecture:

```mermaid
graph TB
    A[User] --> B[Interactive Session]
    B --> C[Role Configuration]
    B --> D[MCP Tools]
    B --> E[AI Providers]
    B --> F[Session Management]

    C --> G[Developer Role<br/>Full Tools]
    C --> H[Assistant Role<br/>Chat Only]
    C --> I[Custom Roles<br/>Configurable]

    D --> J[Developer Server<br/>shell, ast_grep]
    D --> K[Filesystem Server<br/>text_editor, batch_edit, extract_lines]
    D --> L[Web Server<br/>web_search, read_html]
    D --> M[Agent Server<br/>Specialized AI routing]

    E --> N[OpenRouter<br/>Multi-provider]
    E --> O[OpenAI<br/>Direct API]
    E --> P[Anthropic<br/>Claude models]
    E --> Q[Google/Amazon<br/>Cloudflare/DeepSeek]

    F --> R[Context Management<br/>Smart continuation]
    F --> S[Cost Tracking<br/>Real-time monitoring]
    F --> T[Caching System<br/>Automatic optimization]
```

## Core Components

### 1. Session-First Design

**Interactive AI Conversations**: All functionality is accessed through natural language sessions with persistent context and intelligent management.

**Key Features:**
- **Persistent Context**: Sessions maintain conversation history with smart truncation
- **Smart Continuation**: Automatic context preservation when token limits are reached
- **Cost Tracking**: Real-time monitoring of usage and costs across sessions
- **Session Management**: Save, resume, and organize conversations by name

### 2. MCP Tool System

**Built-in MCP Servers** provide comprehensive development capabilities:

**Developer Server** (`src/mcp/dev/`):
- `shell(command="...")` - Execute shell commands with output capture
- `ast_grep(pattern="...", language="...")` - Search and refactor code using AST patterns

**Filesystem Server** (`src/mcp/fs/`):
- `text_editor(command="view|create|str_replace", path="...")` - File operations
- `list_files(directory="...", pattern="...")` - Directory listing with filtering
- `batch_edit(path="...", operations=[...])` - Multiple file operations atomically
- `extract_lines(from_path="...", from_range=[start, end], append_path="...")` - Extract and move code blocks

**Web Server** (`src/mcp/web/`):
- `web_search(query="...")` - Search the web using Brave Search API
- `read_html(sources=["..."])` - Convert HTML content to Markdown

**Agent Server** (`src/mcp/agent/`):
- `agent_*()` tools - Route tasks to specialized AI processing layers
- Dynamic tool generation based on configuration

### 3. Multi-Provider AI Support

**Unified Interface** across 7 AI providers with consistent `provider:model` format:

| Provider | Format | Key Features |
|----------|--------|--------------|
| **OpenRouter** | `openrouter:provider/model` | Multi-provider access, caching, vision |
| **OpenAI** | `openai:model-name` | Direct API, GPT-4o vision, cost calculation |
| **Anthropic** | `anthropic:model-name` | Claude models, caching, vision support |
| **Google** | `google:model-name` | Vertex AI, Gemini 1.5+ vision |
| **Amazon** | `amazon:model-name` | Bedrock models, AWS integration |
| **Cloudflare** | `cloudflare:model-name` | Edge AI, fast inference |
| **DeepSeek** | `deepseek:model-name` | Cost-effective models |

### 4. Role-Based Configuration

**Developer Role** (`config-templates/default.toml` lines 156-337):
- Full access to all MCP tools
- Optimized system prompts for development tasks
- Layer processing enabled for complex workflows
- Server references: developer, filesystem, web, agent

**Assistant Role** (lines 340-355):
- Chat-only mode with limited tool access
- General assistance system prompts
- Minimal tool permissions for safety

**Custom Roles**: Define specific tool permissions and configurations

### 5. Smart Cost Optimization

**Automatic Caching**:
- Cache markers for expensive operations
- Provider-specific cache support (Anthropic Claude, etc.)
- Configurable cache thresholds and timeouts

**Real-Time Tracking**:
- Per-session cost monitoring
- Per-request spending thresholds
- Detailed usage reporting with `/report` command

**Context Management**:
- Smart truncation when limits approached
- AI-driven context preservation during continuation
- Efficient token usage optimization
## Key Features

### Multimodal Vision Support

**Image Analysis** with `/image <path>` command:
- Support for PNG, JPEG, GIF, WebP, BMP, and other formats
- Vision capabilities across all supported providers
- Intelligent file completion and path resolution
- Use cases: UI analysis, diagram interpretation, code screenshots

### Smart Session Continuation

**Automatic Context Management** (`src/session/chat/continuation/`):
- AI-driven context preservation when token limits reached
- File context parsing with `filename:startline:endline` format
- Visual feedback showing continuation status
- Zero configuration required

### Layered Processing System

**AI Pipeline Architecture** (`src/session/layers/`):
- Task refinement and research layers
- Custom command execution with `/run <command>`
- Agent system for specialized AI routing
- Configurable input/output modes

### Advanced Development Tools

**AST-Based Code Operations**:
- `ast_grep()` for structural code search and refactoring
- Pattern-based transformations using AST understanding
- Language-aware code analysis

**Batch File Operations**:
- `batch_edit()` for atomic multi-line changes
- Original line number preservation
- Conflict detection and rollback

## Usage Patterns

### Development Workflow

```bash
# Start development session
octomind session

# Natural language interactions
> "How does authentication work in this project?"
> "Add error handling to the login function"
> "Refactor the database connection code"

# Use built-in tools
> shell(command="cargo test")
> text_editor(command="view", path="src/main.rs", view_range=[1, 50])
> ast_grep(pattern="fn $NAME($ARGS)", language="rust")
```

### Session Commands

Essential commands for session management:
- `/help` - Show available commands
- `/info` - Display token usage and costs
- `/image <path>` - Attach images for AI analysis
- `/mcp info` - Check MCP server status
- `/model <model>` - Switch AI models
- `/cache` - Add cache checkpoint
- `/done` - Finalize task with memorization

### Cost Management

**Real-Time Monitoring**:
- `/info` - Current session costs and token usage
- `/report` - Detailed cost breakdown by request
- Automatic cache markers for optimization
- Spending thresholds and warnings

## Configuration System

### Template-Based Configuration

**Default Template** (`config-templates/default.toml`):
- All defaults defined in template file
- Environment variable overrides with `OCTOMIND_*` prefix
- Role-specific configurations
- MCP server definitions

**Key Configuration Sections**:
- System-wide settings (model, max_tokens, log_level)
- Role configurations (developer, assistant, custom)
- MCP server definitions (built-in and external)
- Layer configurations for AI processing pipeline

### Environment Overrides

```bash
# System settings
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_LOG_LEVEL="debug"

# Role-specific settings
export OCTOMIND_ROLES__DEVELOPER__TEMPERATURE="0.1"
export OCTOMIND_ROLES__DEVELOPER__MAX_TOKENS="16384"
```

## Getting Started

### Quick Start

1. **Install**: `curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash`
2. **Set API Key**: `export OPENROUTER_API_KEY="your_key"`
3. **Start Session**: `octomind session`
4. **Try Commands**: `/help`, `/info`, `/mcp info`

### Next Steps

- **[Installation Guide](./01-installation.md)** - Detailed setup and prerequisites
- **[Configuration Guide](./03-configuration.md)** - Customize settings and roles
- **[Provider Setup](./04-providers.md)** - Configure AI models and keys
- **[Sessions Guide](./05-sessions.md)** - Master interactive sessions
- **[Advanced Features](./06-advanced.md)** - MCP tools and extensibility
- **[Command Layers](./07-command-layers.md)** - AI processing pipeline
- **[MCP Development](./08-mcp-server-development.md)** - Tool development

---

**Octomind v0.13.0** - Session-first AI development assistant with built-in MCP tools and multi-provider support.
