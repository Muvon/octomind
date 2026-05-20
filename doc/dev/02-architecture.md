# Architecture

Internal architecture overview for Octomind contributors.

## Source Layout

```
src/
  main.rs                    # CLI entry point (clap)
  lib.rs                     # Library root, spinner-aware print macros
  directories.rs             # Cross-platform directory paths
  branding.rs                # Branding assets
  proctitle.rs               # Process title management
  state.rs                   # IndexState (current_directory, indexed_files, embedding_calls, graphrag_blocks)

  commands/
    mod.rs                   # Command definitions (run, server, acp, tap, etc.)
    common.rs                # Shared: config resolution, tap fetching
    run.rs                    # Interactive/non-interactive session
    server.rs                 # WebSocket server
    acp.rs                    # ACP agent mode
    config.rs                 # Config generation/validation
    tap.rs / untap.rs         # Tap management
    send.rs                   # Send message to daemon session
    vars.rs                   # Variable display
    complete.rs               # Shell completion candidates

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
    providers.rs             # Provider token config
    migrations.rs            # Config format upgrades
    env_source.rs            # .env tracking
    registry.rs              # Capability resolution
    runtime_overlay.rs       # Runtime config overlay for tap agents

  embeddings/
    mod.rs                   # Local embedding engine (octolib FastEmbedProviderImpl, muvon/octomind-embed)

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
    chat_helper.rs           # CommandCompleter (fuzzy autocomplete for reedline)
    report.rs                # Session usage reporting
    background_jobs.rs       # Async agent job tracking
    smart_summarizer.rs      # Smart text summarization
    modal.rs                 # Terminal modal overlay system
    output.rs                # OutputMode, OutputSink trait (JSONL, WebSocket, Silent)
    cache.rs                 # Cache marker management
    cache_keepalive.rs       # Cache keepalive logic
    token_counter.rs         # Token counting
    model_utils.rs           # Model utilities
    image.rs / video.rs      # Image & video attachment processing
    anchor.rs                # Session anchor management
    dedup.rs                 # Deduplication utilities
    helper_functions.rs      # Context summarization helpers
    logger.rs                # Session logging
    cancellation.rs          # Cancellation tokens
    project_context.rs       # Project context management
    prompt.rs                # Session-level prompt management
    tap_runs.rs              # Tap run tracking

    chat/
      mod.rs                 # Chat orchestration
      commands.rs            # Command constants (28 entries, COMMANDS array)
      animation.rs / animation_manager.rs  # Spinner & animation
      status_prefix.rs       # Shared status formatting (prompt + spinner)
      assistant_output.rs    # Assistant output formatting
      command_executor.rs    # Command execution
      response.rs            # Response processing orchestrator
      response/
        tool_execution.rs    # Tool execution orchestration
        tool_result_processor.rs  # Tool result post-processing
      conversation_compression/  # Compression engine
        mod.rs               # Compression orchestration
        ai.rs                # AI-based summarization
        apply.rs             # Apply compression to messages
        decision.rs          # Compression decision logic
        knowledge.rs         # Knowledge retention across compressions
        prompt.rs            # Compression prompts
        range.rs             # Range selection for compression
        tests.rs             # Compression tests
      cost_tracker.rs        # Token cost tracking
      edit_mode.rs           # Edit mode handling
      file_context.rs        # Active file tracking
      formatting.rs          # Response formatting
      input.rs               # User input handling
      layered_response.rs    # Layered response processing
      markdown.rs            # Markdown processing
      message_handler.rs     # Message handling
      prompt.rs              # Prompt management
      reedline_adapter.rs    # Reedline line editor
      syntax.rs              # Syntax highlighting
      thinking_display.rs    # Thinking/reasoning display
      tool_display.rs        # Tool output display
      tool_error_tracker.rs  # Tool error tracking
      tool_processor.rs      # Tool call processing
      session/
        core.rs              # ChatSession struct, SessionInitParams builder
        api_executor.rs      # API call execution
        api_prep.rs          # API call preparation (compression, auto-activation)
        commands/            # 28 command handler modules
        display.rs           # Session display
        error_utils.rs       # Error utilities
        layer_processor.rs   # Layer processing in session context
        main_loop.rs         # Interactive & non-interactive session loops
        messages.rs          # Message management
        params.rs            # CLI parameter parsing
        prompt_setup.rs      # Prompt setup
        setup.rs             # Session setup & initialization
        utils.rs             # Session utilities

    share/
      mod.rs                   # /share: upload session JSONL → octomind.run/r/<id>
      bridge.rs                # HTTP bridge to octomind.run
      upload.rs                # Upload logic

    history/
      mod.rs                   # Role-based history management (per-role files, legacy migration)

    layers/
      mod.rs                   # Layer trait & processor
    workflows/               # Workflow orchestrator (with step timing)
    pipelines/               # Deterministic script pipeline executor

  mcp/
    mod.rs                   # MCP coordinator, tool routing (try_execute_tool_call)
    server.rs                # HTTP/stdio server communication
    process.rs               # Process lifecycle, health, registries
    health_monitor.rs        # Background health checking
    workdir.rs               # Thread-local working directory
    hint_accumulator.rs      # MCP misuse hint accumulation
    tool_map.rs              # Global TOOL_MAP: tool name → server config
    utils.rs / shared_utils.rs  # Tool call parsing, response formatting

    core/
      mod.rs                 # Core server coordination
      functions.rs           # Core builtin tool definitions
      capability.rs          # Capability system (list/enable/disable/discover/auto-activate)
      tap.rs                 # Tap tool (run/list/stop/discover)
      dynamic.rs             # Dynamic MCP server management (add/remove at runtime)
      dynamic_agents.rs      # Dynamic agent tool registration
      local_tool.rs          # Local tool (shebang scripts) support
      skill.rs               # Skill management
      skill_auto.rs          # Skill auto-activation & validation hooks
      skill_tests.rs         # Skill unit tests
      plan/                  # Plan tool
        mod.rs, core.rs, compression.rs, storage.rs, memory_storage.rs
        plan_tests.rs        # Plan tool unit tests
      schedule/              # Schedule tool (with persistence)
        mod.rs, core.rs, storage.rs

    runtime/
      mod.rs                 # Runtime server: mcp, agent, skill tools

    agent/
      mod.rs                 # Agent server: agent_* tools
      functions.rs           # Agent tool implementations

    oauth/
      mod.rs                 # OAuth 2.1 + PKCE module
      discovery.rs           # OAuth provider discovery (RFC 9728)
      flow.rs                # Authorization flow
      callback_server.rs     # Local callback HTTP server
      cimd.rs                # CIMD/DCR flow
      token_store.rs         # Keyring + file fallback

  agent/
    mod.rs                   # Agent module
    taps.rs                  # Tap management (clone, symlink, list)
    registry.rs              # Agent discovery across taps, parse_capability_toml
    deps.rs                  # Dependency resolution
    inputs.rs                # Input/env variable resolution ({{INPUT:KEY}}, {{ENV:KEY}})
    resolver.rs              # Agent config/role resolution

  acp/
    mod.rs                   # ACP agent implementation
    agent.rs                 # ACP session handler
    commands.rs              # ACP command routing

  websocket/
    mod.rs                   # WebSocket server
    protocol.rs              # Message types (Client/Server)
    server.rs                # WebSocket connection handler

  sandbox/
    mod.rs                   # Platform sandboxing
    linux.rs                 # Linux Landlock/seccomp (kernel 5.13+)
    macos.rs                 # macOS Seatbelt (sandbox-exec)

  logging/
    mod.rs                   # Log module
    acp_error.rs             # ACP JSONL error sink
    tracing_setup.rs         # Tracing subscriber setup

  providers.rs               # Provider abstraction (delegates to octolib)

  utils/
    mod.rs                   # Utils module
    file_parser.rs           # File reading/parsing helpers
    file_renderer.rs         # File rendering helpers
    glob.rs                  # Glob pattern matching
    terminal_output.rs       # Terminal output utilities
    time.rs                  # Time formatting
    truncation.rs            # Text truncation helpers
    term_echo.rs             # Terminal echo control
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
- `octolib` -- local embedding engine (FastEmbedProviderImpl)
