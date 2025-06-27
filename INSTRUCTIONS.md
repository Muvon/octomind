# OCTOMIND DEVELOPMENT GUIDE

## 🎯 CORE ARCHITECTURE

**Session-First Design**: Everything happens in interactive AI sessions with MCP tools
**Template-Based Config**: All defaults in `config-templates/default.toml`, NO hardcoded values
**Role-Based Access**: Developer (full tools), Assistant (chat only), Custom roles
**Layered Processing**: task_refiner → task_researcher → developer layers
**Custom Commands**: `/run <command>` executes configured layer-based commands
**Agent System**: `agent_<name>(task="...")` MCP tools route tasks to specialized AI layers
**Cache & Cost**: 2-marker cache system + automatic cost tracking across sessions/layers/tools

## 🚫 CRITICAL CODE QUALITY RULES

### **MCP PROTOCOL COMPLIANCE - MANDATORY**
- **NEVER return `Err()` from MCP tool functions** - use `Ok(McpToolResult::error())` instead
- **ALWAYS validate parameters** with clear error messages for missing/empty/wrong type
- **ALWAYS handle API key failures** gracefully with actionable error messages
- **ALWAYS handle cancellation** by returning proper MCP error results
- **FOLLOW MCP standard format**: `{content: [{type: "text", text: "..."}], isError: true/false}`
- **PRESERVE tool_id** in all result scenarios (success and error)
- **USE line_replace for precision** when str_replace loses accuracy

### **DEVELOPMENT RESTRICTIONS - NEVER TOUCH THESE**
- **NEVER run tests that affect global configuration** or create session files
- **NEVER create example configs** or modify existing config structures in system-wide files
- **Your job**: Build, test compilation, fix code issues ONLY
- **User handles**: All complex validation with checking if it works or not

### **DEVELOPMENT BUILD EFFICIENCY**
- **ALWAYS use `cargo check`** for syntax/compilation verification - fastest option
- **NEVER use `cargo build --release`** - extremely slow, wastes development time
- **Use `cargo build` (debug)** only when you need to run the actual binary
- **Focus on `cargo check`** for iterative development and validation
- **Run `cargo clippy --all-features --all-targets -- -D warnings`** to fix ALL code quality issues (treat warnings as errors)

### **NEVER HIDE ERRORS WITH FALLBACKS**
```rust
// ❌ SHIT CODE - hides real problems
let config = if let Ok(cfg) = load_config() {
    cfg
} else {
    default_config() // This hides the real error!
};

// ✅ GOOD CODE - exposes problems immediately
let config = load_config()
    .expect("CRITICAL: Failed to load config - fix the underlying issue");
```

### **NEVER USE println!() FOR DEBUG - USE PROPER LOGGING**
```rust
// ❌ SHIT CODE
println!("DEBUG: something happened");

// ✅ GOOD CODE
crate::log_debug!("Something happened");
```

### **FAIL FAST ON CONFIGURATION ERRORS**
- Use `.expect()` with clear error messages for critical operations
- Never fallback to defaults when the real config is needed
- Configuration errors should stop execution, not continue silently

### **HANDLE REMOTE SERVER FAILURES PROPERLY**
- If a remote HTTP server's `tools/list` fails, exclude it completely
- Don't include fallback tools that won't work
- Cache empty results to avoid repeated failures

## 📍 WHERE TO LOOK BY TASK

### 🔧 CONFIGURATION ISSUES
**Template & Loading:**
- `config-templates/default.toml` - ALL defaults and structure
- `src/config/loading.rs` - Config loading, template injection, env overrides
- `src/config/mod.rs` - Main Config struct and validation

**Specific Config Types:**
- **Roles**: `src/config/roles.rs` + template `[[roles]]` sections
- **MCP Servers**: `src/config/mcp.rs` + template `[[mcp.servers]]` sections
- **Layers**: `src/session/layers/layer_trait.rs` + template `[[layers]]` sections
- **Commands**: Template `[[commands]]` sections (use same layer system)
- **Agents**: `src/config/mod.rs` AgentConfig + template (route to layers via MCP)
- **Providers**: `src/providers/` directory

### 🎮 SESSION BEHAVIOR
**Core Session Logic:**
- `src/session/chat/session/runner.rs` - Main interactive session loop
- `src/session/chat/session/commands/` - All `/command` implementations
- `src/session/chat/response.rs` - Response processing orchestrator

**Context & Memory:**
- `src/session/chat/context_truncation.rs` - Smart context management
- `src/session/cache.rs` - Caching system (2-marker approach)
- `src/session/chat/input.rs` - User input handling with history

**Smart Session Continuation:**
- `src/session/chat/continuation/` - **REFACTORED MODULAR ARCHITECTURE**
  - `mod.rs` - Main module coordinator with public API re-exports
  - `detection.rs` - Continuation trigger logic and state checks
  - `injection.rs` - Summary request injection when limits reached
  - `processing.rs` - Response processing with **DISPLAY FIXES** for user visibility
  - `file_context.rs` - File parsing, context generation, and tests
  - `constants.rs` - All prompts and message templates
- `src/session/chat/session_continuation.rs` - **LEGACY COMPATIBILITY** - re-exports new API
- `src/session/chat/response.rs` - Integration point for continuation checks
- `src/session/chat/context_truncation.rs` - Continuation-aware truncation logic
- `src/session/chat/session/core.rs` - ChatSession structure with continuation state

**Cost & Performance:**
- `src/session/chat/cost_tracker.rs` - Cost accumulation across sessions/layers/tools
- `src/session/mod.rs` SessionInfo - Token/cost tracking per session
- Auto cache markers: system messages, tools (supports caching models only)
- 2-marker system: content cache management for cost optimization

### 🔧 MCP TOOLS - FULLY PROTOCOL COMPLIANT
**CRITICAL MCP COMPLIANCE**: All tools now return proper MCP-compliant error responses instead of crashing communication flow.

**Tool System Core:**
- `src/mcp/mod.rs` - Tool routing, execution, `try_execute_tool_call()` with proper error handling
- `src/mcp/tool_map.rs` - Static tool-to-server mapping for performance

**Built-in Tool Servers:**
- **Developer**: `src/mcp/dev/` (shell execution, ast_grep)
- **Filesystem**: `src/mcp/fs/` (text_editor, list_files)
- **Web**: `src/mcp/web/` (web_search, read_html, image_search, video_search, news_search)
- **Agent**: `src/mcp/agent/` (layer routing for AI tasks via `agent_<name>` tools)

**External Server Management:**
- `src/mcp/server.rs` - HTTP server communication
- `src/mcp/process.rs` - Stdin server process management
- `src/mcp/health_monitor.rs` - Server health monitoring

**MCP Protocol Standards:**
- ✅ All tools return `Ok(McpToolResult::error())` for failures (never `Err()`)
- ✅ Error format: `{content: [{type: "text", text: "error"}], isError: true}`
- ✅ Success format: `{content: [{type: "text", text: "result"}], isError: false}`
- ✅ Parameter validation with clear error messages
- ✅ Graceful handling of missing API keys, empty parameters, cancellation

### 🤖 AI PROVIDERS
**Provider System:**
- `src/providers/mod.rs` - Provider trait and factory
- `src/providers/*/` - Individual provider implementations (OpenRouter, OpenAI, Anthropic, etc.)

### 📊 LAYERED PROCESSING & COMMANDS
**Layer Architecture:**
- `src/session/layers/types/generic.rs` - Main layer implementation
- `src/session/layers/orchestrator.rs` - Layer execution orchestration
- `src/session/layers/layer_trait.rs` - Layer configuration and traits

**Custom Commands:**
- `src/session/chat/command_executor.rs` - `/run <command>` execution
- Template `[[commands]]` sections - Command definitions (use layer system)
- Commands execute layers without storing in session history

**Agent System:**
- `src/mcp/agent/functions.rs` - Dynamic `agent_<name>` tool generation
- `src/config/mod.rs` AgentConfig - Agent-to-layer routing configuration
- Template agents section - Agent definitions that route to layers via MCP

## 🐛 DEBUGGING BY SYMPTOM

### Tool Not Working
1. **Check tool routing**: `src/mcp/mod.rs` → `build_tool_server_map()`
2. **Check tool execution**: `src/mcp/mod.rs` → `try_execute_tool_call()`
3. **Check tool definitions**: `src/mcp/*/functions.rs` files
4. **Check allowed_tools patterns**: Role/layer config in template
5. **Check MCP compliance**: Tool should return `McpToolResult::error()` not `Err()`
6. **Check parameter validation**: Required parameters properly validated
3. **Check tool definitions**: `src/mcp/*/functions.rs` files
4. **Check allowed_tools patterns**: Role/layer config in template

### Configuration Not Loading
1. **Template injection**: `src/config/loading.rs` → `load()`
2. **Environment overrides**: Check `OCTOMIND_*` variables
3. **Validation errors**: `src/config/validation.rs` → `validate()`

### Session Commands Failing
1. **Command routing**: `src/session/chat/session/commands/mod.rs`
2. **Individual commands**: `src/session/chat/session/commands/*.rs`
3. **Command permissions**: Check role configuration

### Provider/Model Issues
1. **Provider implementation**: `src/providers/*/` specific provider
2. **API keys**: Check `*_API_KEY` environment variables
3. **Model format**: Must be `provider:model` format

### Layer Processing Issues
1. **Layer execution**: `src/session/layers/types/generic.rs` → `process()`
2. **Layer orchestration**: `src/session/layers/orchestrator.rs`
3. **Input/output modes**: Check layer config in template

### Cache/Cost Issues
1. **Cache markers**: `src/session/cache.rs` → `manage_content_cache_markers()`
2. **Cost tracking**: `src/session/chat/cost_tracker.rs` → `track_exchange_cost()`
3. **Token counting**: `src/session/mod.rs` SessionInfo struct
4. **Cache support**: Check if model supports caching (Anthropic Claude, etc.)

## 🚀 COMMON MODIFICATIONS

### Add New MCP Tool
1. **Function definition**: Add to `src/mcp/*/functions.rs`
2. **Implementation**: Add to `src/mcp/*/` (core.rs or new file)
3. **Routing**: Update `src/mcp/mod.rs` → `try_execute_tool_call()` with proper error handling
4. **CRITICAL**: Return `Ok(McpToolResult::error())` for all failures, never `Err()`
5. **Parameter validation**: Use proper MCP-compliant validation patterns

### Add New Provider
1. **Implementation**: Create `src/providers/new_provider.rs`
2. **Registration**: Add to `src/providers/mod.rs` factory
3. **Config**: Add provider section to template

### Add New Configuration
1. **Template first**: Add to `config-templates/default.toml`
2. **Struct**: Add to appropriate `src/config/*.rs`
3. **Validation**: Add validation rules

### Add New Layer
1. **Config**: Add to template `[[layers]]` section
2. **No code needed**: Uses generic layer implementation
3. **Configure**: Set input_mode, output_mode, MCP access

### Add New Custom Command
1. **Config**: Add to template `[[commands]]` section (same as layer config)
2. **Usage**: `/run <command_name>` executes the layer without storing in history
3. **No code needed**: Uses existing command executor + layer system

### Add New Agent
1. **Layer**: First create the layer in `[[layers]]` section
2. **Agent config**: Add to template agents section with layer name
3. **Usage**: `agent_<name>(task="...")` MCP tool routes to the layer

### Modify Session Continuation System
1. **Modular architecture**: `src/session/chat/continuation/` - **NEW REFACTORED STRUCTURE**
   - `processing.rs` - Main response processing with user display fixes
   - `detection.rs` - Trigger logic and state checks
   - `injection.rs` - Summary request injection
   - `file_context.rs` - File parsing and context generation
   - `constants.rs` - All prompts and templates
2. **Legacy compatibility**: `src/session/chat/session_continuation.rs` - Re-exports for backward compatibility
3. **Integration**: `src/session/chat/response.rs` - Entry point for continuation checks
4. **State management**: `src/session/chat/session/core.rs` - ChatSession continuation state
5. **Configuration**: `config-templates/default.toml` - `max_session_tokens_threshold` field (0=disabled)

## 📋 CRITICAL PATTERNS

### MCP Tool Error Handling Pattern
```rust
// ✅ CORRECT MCP-compliant parameter validation
let param = match call.parameters.get("param") {
    Some(Value::String(p)) => {
        if p.trim().is_empty() {
            return Ok(McpToolResult::error(
                call.tool_name.clone(),
                call.tool_id.clone(),
                "Parameter 'param' cannot be empty".to_string(),
            ));
        }
        p.clone()
    }
    Some(_) => {
        return Ok(McpToolResult::error(
            call.tool_name.clone(),
            call.tool_id.clone(),
            "Parameter 'param' must be a string".to_string(),
        ));
    }
    None => {
        return Ok(McpToolResult::error(
            call.tool_name.clone(),
            call.tool_id.clone(),
            "Missing required parameter 'param'".to_string(),
        ));
    }
};

// ✅ CORRECT MCP-compliant routing
match tool::execute_command(call, token).await {
    Ok(mut result) => {
        result.tool_id = call.tool_id.clone();
        return Ok(result);
    }
    Err(e) => {
        return Ok(McpToolResult::error(
            call.tool_name.clone(),
            call.tool_id.clone(),
            format!("Tool execution failed: {}", e),
        ));
    }
}
```

### File Patterns
- **Config**: `src/config/*.rs` + `config-templates/default.toml`
- **Tools**: `src/mcp/*/functions.rs` + `src/mcp/*/core.rs`
- **Sessions**: `src/session/chat/` + `src/session/layers/`
- **Continuation**: `src/session/chat/continuation/` (modular architecture)
- **Providers**: `src/providers/*/`

### Environment Variables
- **API Keys**: `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `BRAVE_API_KEY`, etc.
- **Config Overrides**: `OCTOMIND_*` for any config setting
- **Debug**: Use `/loglevel debug` in sessions

### Key Commands
- **Config**: `octomind config --show` to see current config
- **Sessions**: `octomind session` for interactive mode
- **Debug**: `/mcp info` to check tool status in sessions
- **Custom Commands**: `/run <command_name>` to execute configured layers
- **Agents**: Use `agent_<name>(task="description")` MCP tools for specialized AI tasks
- **Cache**: `/cache` to manually mark cache points, `/info` to see costs/tokens

### Development Workflow
- **Build Check**: `cargo check` - fastest compilation verification (PREFERRED)
- **Code Quality**: `cargo clippy --all-features --all-targets -- -D warnings` - fix ALL code quality issues (treat warnings as errors)
- **Debug Build**: `cargo build` - only when you need to run the actual binary
- **NEVER**: `cargo build --release` - extremely slow, avoid during development
- **NEVER**: Modify configs, create tests, or affect global configuration
