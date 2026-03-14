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

    D --> J[Core Server<br/>plan]
    D --> K[Filesystem Server<br/>text_editor, batch_edit, extract_lines]
    D --> L[Agent Server<br/>Specialized AI routing]

    E --> M[OpenRouter<br/>Multi-provider]
    E --> N[OpenAI<br/>Direct API]
    E --> O[Anthropic<br/>Claude models]
    E --> P[Google/Amazon<br/>Cloudflare/DeepSeek]

    F --> Q[Context Management<br/>Conversation compression]
    F --> R[Cost Tracking<br/>Real-time monitoring]
    F --> S[Caching System<br/>Automatic optimization]
```

## Core Components


### 1. Session-First Design

**Interactive AI Conversations**: All functionality is accessed through natural language sessions with persistent context and intelligent management.

**Key Features:**
- **Persistent Context**: Sessions maintain conversation history with smart truncation
- **Conversation Compression**: Automatic context compression when token limits approached
- **Cost Tracking**: Real-time monitoring of usage and costs across sessions
- **Session Management**: Save, resume, and organize conversations by name
### 2. MCP Tool System

**Built-in MCP Servers** provide comprehensive development capabilities:

**Core Server** (`src/mcp/core/`):
- `plan(command="start|step|next|list|done|reset", ...)` - Structured task management with progress tracking

**Filesystem Server** (`src/mcp/fs/`):
- `view(path="...", lines=[start, end], pattern="...", content="...", ...)` - Read files, view directories, and search file content
- `text_editor(command="create|str_replace|undo_edit", path="...", ...)` - Create files or make targeted string replacements
- `batch_edit(path="...", operations=[...])` - Multiple file operations atomically
- `extract_lines(from_path="...", from_range=[start, end], append_path="...", append_line=N)` - Extract and move code blocks
- `shell(command="...", background=false)` - Execute shell commands with output capture, foreground/background execution
- `workdir(path="...", reset=false)` - Get or set working directory for parallel execution isolation
- `ast_grep(pattern="...", language="...", rewrite="...", ...)` - Search and refactor code using AST patterns

**Agent Server** (`src/mcp/agent/`):
- `agent_*()` tools - Delegate tasks to configured ACP sub-agents (each spawns an ACP subprocess via the configured `command`)

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

**Developer Role** (`config-templates/default.toml` lines 119-301):
- Full access to all MCP tools
- Optimized system prompts for development tasks
- Layer processing enabled for complex workflows
- Server references: core, filesystem, octocode, agent

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

**Adaptive Context Compression**:
- Token-based compression triggers at configurable thresholds (50k, 100k, 150k tokens)
- Pressure-level system with graduated compression ratios (2x, 4x, 8x)
- Hierarchical compression: task → phase → project levels
- Preserves architectural information and file references during compression
- Decision model (Haiku) for cost-efficient compression decisions

**Context Awareness**:
- Real-time context percentage display in prompt (e.g., `[0.02|45.3%]`)
- Shows estimated cost and context usage percentage
- Unified token calculation including messages, system prompt, and tool definitions

**Real-Time Tracking**:
- Per-session cost monitoring
- Per-request spending thresholds
- Detailed usage reporting with `/report` command

**Context Management**:
- Smart truncation when limits approached
- Conversation compression for efficient token usage
- Efficient token usage optimization

### Multimodal Vision Support

**Image Analysis** with `/image <path>` command:
- Support for PNG, JPEG, GIF, WebP, BMP, and other formats
- Vision capabilities across all supported providers
- Intelligent file completion and path resolution
- Use cases: UI analysis, diagram interpretation, code screenshots

### Conversation Compression

**Adaptive Context Management** (`src/session/chat/conversation_compression.rs`):
- AI-driven context compression when token limits approached
- Token-based compression triggers at configurable thresholds (50k, 100k, 150k)
- Hierarchical compression: task → phase → project levels
- Visual feedback showing compression status
- Zero configuration required
### Layered Processing System

**AI Pipeline Architecture** (`src/session/layers/`):
- Task refinement and research layers
- Custom command execution with `/run <command>`
- Agent system for specialized AI routing
- Configurable input/output modes

### Brain-Inspired Workflow System

**Multi-Step Planning and Execution** (`src/session/workflows/`):
- **Planner vs Executor**: Workflows act as planners (prefrontal cortex), main model executes (motor cortex)
- **Control Flow Primitives**: Once, Loop, Foreach, Conditional, Parallel step types
- **Validation and Feedback**: Built-in validation loops and iterative refinement
- **Non-intrusive**: Workflows don't affect session history
- **Execution**: Use `/workflow <name>` command to execute configured workflows

**Workflow Architecture**:
```
User Input
    ↓
┌─────────────────────────────────────┐
│  WORKFLOW (Planning)                │
│  • Decomposes tasks                 │
│  • Validates approaches             │
│  • Predicts impacts                 │
│  • Evaluates quality                │
│  • Generates enhanced plan          │
└──────────────┬──────────────────────┘
               ↓ Enhanced Plan
┌─────────────────────────────────────┐
│  MAIN MODEL (Execution)             │
│  • Receives refined plan            │
│  • Uses MCP tools                   │
│  • Implements changes               │
└─────────────────────────────────────┘
               ↓
            Result
```

For comprehensive workflow documentation, see [doc/10-workflows.md](./10-workflows.md).

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
octomind run

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
3. **Start Session**: `octomind run`
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

**Octomind v0.20.0** - Session-first AI development assistant with built-in MCP tools and multi-provider support.
