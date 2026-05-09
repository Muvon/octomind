# Architecture

Internal architecture overview for Octomind contributors.

## Source Layout

```
src/
  main.rs                    # CLI entry point (clap)
  lib.rs                     # Library root, spinner macros
  directories.rs             # Cross-platform directory paths

  commands/
    mod.rs                   # Command definitions (run, server, acp, tap, etc.)
    common.rs                # Shared: config resolution, tap fetching
    run.rs                   # Interactive/non-interactive session
    server.rs                # WebSocket server
    acp.rs                   # ACP agent mode
    config.rs                # Config generation/validation
    tap.rs / untap.rs        # Tap management
    vars.rs                  # Variable display
    complete.rs              # Shell completion candidates

  config/
    mod.rs                   # Config struct, LogLevel, log macros
    loading.rs               # TOML parsing, multi-file merge, dedup
    validation.rs            # Config validation (thresholds, hooks, etc.)
    roles.rs                 # Role configuration
    mcp.rs                   # MCP server config (Builtin/Http/Stdio)
    hooks.rs                 # Webhook hook config
    workflows.rs             # Workflow + step definitions
    pipelines.rs             # Pipeline step definitions and parsing
    layers.rs                # Layer configuration
    agents.rs                # Agent (ACP) configuration
    env_source.rs            # .env tracking
    registry.rs              # Capability resolution

  learning/
    mod.rs                   # LearningConfig, Lesson struct, public API
    extract.rs               # Lesson extraction from conversations (LLM-based)
    inject.rs                # Lesson retrieval and system prompt injection
    backend/
      mod.rs                 # LearningBackend trait, factory
      file.rs                # File-based backend (markdown + YAML frontmatter)
      mcp.rs                 # MCP tool backend (e.g., octobrain)

  session/
    mod.rs                   # Message types, session entry points
    context.rs               # Session context with learning state
    persistence.rs           # Session save/restore
    inbox.rs                 # Unified inbox (schedule, agent, skill, inject, webhook)
    inject_listener.rs       # Unix socket for message injection
    webhook_listener.rs      # HTTP webhook → inbox injection
    completion.rs            # Chat completion orchestration
    chat_helper.rs           # Helper functions for chat
    report.rs                # Session usage reporting
    background_jobs.rs       # Async agent job tracking
    smart_summarizer.rs      # Smart text summarization
    modal.rs                 # Terminal modal overlay system
    output.rs                # OutputMode, OutputSink trait

    chat/
      mod.rs                 # Chat orchestration
      session/
        core.rs              # Session state management
        commands/           # 23 command handlers: help, info, model, role, loglevel, copy, clear, plan, context, image, video, prompt, done, list, run, workflow, mcp, report, session, skill, exit, utils
        setup.rs             # Session setup & initialization
        params.rs           # CLI parameter parsing
        main_loop.rs        # Interactive & non-interactive session loops
      response.rs            # Response processing, compression checks
      conversation_compression.rs  # Compression engine
      semantic_chunking.rs   # Discourse-aware chunking
      token_counter.rs       # Unified token counting
      context_truncation.rs  # Context trimming
      file_context.rs        # Active file tracking
      tool_error_tracker.rs  # Tool error tracking
      formatting.rs          # Response formatting
      markdown.rs            # Markdown processing
      syntax.rs              # Syntax highlighting
      tool_display.rs        # Tool output display
      history/
        mod.rs               # History management
      prompt.rs              # Prompt management

    cache.rs                 # Cache marker management
    layers/
      mod.rs                 # Layer trait & processor
      types/
        mod.rs              # Layer type definitions
    workflows/               # Workflow orchestrator (with step timing)
    pipelines/               # Deterministic script pipeline executor
    project_context.rs       # Project context management

  mcp/
    mod.rs                   # MCP coordinator
    server.rs                # HTTP/stdio server communication
    process.rs               # Process lifecycle, health, registries
    health_monitor.rs        # Background health checking
    workdir.rs               # Thread-local working directory
    hint_accumulator.rs      # MCP misuse hint accumulation
    tool_map.rs              # Global TOOL_MAP: tool name → server config
    shared_utils.rs          # Shared MCP utilities

    core/
      mod.rs                 # Core server: plan, mcp, agent, schedule, skill
      plan/                  # Plan tool + compression
        mod.rs, core.rs, compression.rs, storage.rs, memory_storage.rs, plan_tests.rs
      schedule/              # Schedule tool
        mod.rs, core.rs, storage.rs
      skill.rs               # Skill management
      skill_auto.rs          # Skill auto-activation & validation hooks
      dynamic.rs             # Dynamic MCP server management (add/remove at runtime)
      dynamic_agents.rs      # Dynamic agent tool registration

    agent/
      mod.rs                 # Agent server: agent_* tools
      functions.rs           # Agent tool implementations

    oauth/
      mod.rs                 # OAuth module
      discovery.rs           # OAuth provider discovery
      flow.rs                # Authorization flow
      callback_server.rs     # Local callback HTTP server
      cimd.rs                # CIMD flow
      token_store.rs         # Keyring + file fallback

  agent/
    taps.rs                  # Tap management (clone, symlink, list)
    registry.rs              # Agent discovery across taps
    deps.rs                  # Dependency resolution
    inputs.rs                # Input/env variable resolution

  acp/
    mod.rs                   # ACP agent implementation
    agent.rs                 # ACP session handler
    commands.rs              # ACP command routing

  websocket/
    mod.rs                   # WebSocket server
    protocol.rs              # Message types (Client/Server)

  providers.rs               # Provider abstraction (delegates to octolib)
  logging/                   # Log backends (CLI, file, ACP error sink)
```

## Key Patterns

### Global Registries (`src/mcp/process.rs`)

MCP server processes are managed via global statics:
- `SERVER_PROCESSES` -- active process handles
- `SERVER_RESTART_INFO` -- health, restart count, timestamps
- `SERVER_REF_COUNTS` -- reference counting for shared servers
- `SERVER_STDERR` -- recent stderr per server
- `SERVER_CAPABILITIES` -- parsed server capabilities

### Session Context

- `CLI_SESSION_CONTEXT` -- global (role, project) pair
- `CLI_NOTIFICATION_SENDER` -- channel for MCP notifications
- Session ID derived from SHA256 of git remote or CWD

### Output Abstraction (`src/session/output.rs`)

Zero-sized types for zero-cost output routing:
- `SilentSink` -- discards (CLI mode)
- `JsonlSink` -- JSON Lines to stdout
- `WebSocketSink` -- sends through channel

### Error Handling

- Primary: `anyhow::Result<T>` everywhere
- Validation: `anyhow!()` with descriptive messages
- MCP tools: `McpToolResult::error()` (never `Err()`)
- ACP: errors logged to JSONL file (stdout reserved for protocol)

### Config Loading (`src/config/loading.rs`)

1. Load `config.toml` (TOML -> `toml::Value`)
2. Load other `*.toml` files alphabetically
3. Deep-merge tables, concatenate arrays of tables
4. Deduplicate by `name` field (last wins)
5. Deserialize to `Config` struct
6. Validate all fields

### Compression Pipeline

1. Token monitor checks pressure levels
2. Exponential cooldown prevents loops
3. Cache-aware economics calculates net benefit
4. Decision model (cheap AI) decides whether to compress
5. Semantic chunking preserves conversation structure
6. Summary replaces compressed content
7. Knowledge entries retained across compressions

## Dependencies

Key external crates:
- `octolib` -- provider abstraction, LLM integration
- `rmcp` -- MCP protocol implementation
- `agent-client-protocol` -- ACP support
- `tokio` -- async runtime
- `clap` -- CLI argument parsing
- `serde` / `toml` -- serialization
- `oauth2` -- OAuth 2.1 + PKCE
- `keyring` -- secure token storage
- `hyper` -- HTTP server
- `reedline` -- interactive readline
- `syntect` -- syntax highlighting
