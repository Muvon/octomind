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
    guardrails.rs           # Guardrails (pipe) configuration
    layers.rs                # Layer configuration
    agents.rs                # Agent (ACP) configuration
    providers.rs             # Provider token config
    migrations.rs            # Config format upgrades
    env_source.rs            # .env tracking
    registry.rs              # RegistryConfig: [registry] manifest cache TTL
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
    guardrails.rs            # Project-local tool deny/hook/validator rules (.agents/guardrails.toml)
    pipe.rs                  # Pipe execution (pre-model input transform)
    hooks.rs                 # Webhook hook listener management (--hook flag)
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
      commands.rs            # COMMANDS array ([&str; 27]: 25 distinct commands + /? and /quit aliases)
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
        range.rs             # Range selection for compression (find_compression_range)
        schema.rs            # Typed compression summary schema (CompressionSummary, KeyEntities, FileContextEntry)
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
        commands/            # 25 command handler modules (28 files incl. mod.rs, utils.rs, display.rs)
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
      mod.rs                   # Layer module root
      layer_trait.rs           # Layer trait
      processor.rs             # Layer processor

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
      plan_tests.rs          # Plan tool unit tests
      plan/                  # Plan tool
        mod.rs, core.rs, compression.rs, storage.rs, memory_storage.rs
      schedule/              # Schedule tool (with persistence)
        mod.rs, core.rs, storage.rs

    runtime/
      mod.rs                 # Runtime server: mcp, agent, skill, schedule, capability tools

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

  workflow/
    mod.rs                   # Module root, re-exports (execute_workflow, WorkflowDef)
    schema.rs                # WorkflowDef, Step (enum), Condition, Sequential/Parallel/Loop steps
    validate.rs              # Pre-flight validation (validate)
    run.rs                   # Orchestrator: execute(), stats aggregation, progress to stderr
    proc.rs                  # Per-step subprocess (octomind run --format jsonl), JSONL stream parsing

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

### Tool Routing & Builtin Servers (`src/mcp/mod.rs`)

A tool call flows `execute_tool_call` -> `try_execute_tool_call` (resolves the target server from `TOOL_MAP`) -> for builtin servers, `route_builtin_tool` dispatches by server name; for external servers it forwards to `server::execute_tool_call`. `execute_tool_calls` runs a batch.

Builtin tools are split across three in-process servers:
- `core` -- hosts `plan` and `tap`
- `runtime` -- hosts `mcp`, `agent`, `skill`, `schedule`, `capability` (via `runtime::execute_runtime_tool`)
- `agent` -- hosts the dynamic per-agent execution tools

Dynamic per-agent tool: each registered agent yields a distinct runtime tool named `agent_<name>` (generated by `core/dynamic_agents.rs`) that EXECUTES that agent. It is separate from the `agent` management tool (which lists/enables/disables agents). Dynamic MCP servers and local shebang tools are similarly contributed at runtime (`core/dynamic.rs`, `core/local_tool.rs`).

### Session Context

- `CLI_SESSION_CONTEXT` -- global (role, project, workdir) triple (set via `set_session_context`, `src/mcp/process.rs`)
- `CLI_NOTIFICATION_SENDER` -- channel for MCP notifications
- Project ID derived from SHA-256 of the git remote origin URL (else CWD) via `derive_project_id()`; session NAMES are generated separately as `YYMMDD-basename-HHMM-uuid4short` (`generate_session_name`, `src/session/chat/session/core.rs`)

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
2. Load other `*.toml` files alphabetically, EXCEPT `mcp-*.toml` (dash) files, which are loaded last as override files (`is_mcp_extension_file`); `mcp.toml` without a dash loads in normal alphabetical order
3. Deep-merge tables, concatenate arrays of tables
4. Deduplicate by `name` field (last wins)
5. Deserialize to `Config` struct
6. Validate all fields

### Compression Pipeline (`src/session/chat/conversation_compression/`)

Entry points: `should_check_compression` gates whether a check runs (returns a pressure ratio); `check_and_compress_conversation` is the orchestrator (called from `api_prep.rs` before an API call and from the tool-result path); `apply_compression` rewrites the message list.

1. Token monitor checks pressure levels (`should_check_compression`)
2. Exponential cooldown prevents loops
3. Cache-aware economics calculates net benefit
4. Decision model (cheap AI, separate from the main model: `[compression.decision] model`, default `openai:gpt-5-mini`) decides whether to compress
5. Range selected by anchor (latest `<instructions>` user message, else first user message); messages after the anchor up to the end are drained and replaced (`find_compression_range`, `range.rs`)
6. A typed summary (`CompressionSummary`, `schema.rs`) replaces the drained content
7. Knowledge entries retained across compressions

Persistence model (`src/session/logger.rs`, `src/session/persistence.rs`): the session JSONL log records `SUMMARY` (session metadata, source of truth on resume) plus marker entries `COMPRESSION_POINT`, `RESTORATION_POINT`, and `KNOWLEDGE_ENTRY`. On restore, `COMPRESSION_POINT` clears the corresponding messages to reflect the compressed state.

## Dependencies

Key external crates:
- `octolib` -- provider abstraction, LLM integration, and local embedding engine (FastEmbedProviderImpl)
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
