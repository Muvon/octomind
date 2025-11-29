# OCTOMIND DEVELOPMENT GUIDE

> **Octomind** - Session-based AI development assistant with conversational codebase interaction, multimodal vision support, built-in MCP tools, and multi-provider AI integration.

## 🚀 QUICK START FOR NEW DEVELOPERS

### Prerequisites
- **Rust 1.82+** and Cargo
- **API Key** from supported AI provider (OpenRouter, OpenAI, Anthropic, etc.)
- **Git** for version control
- **Basic Rust knowledge** for development contributions

### 5-Minute Setup
```bash
# 1. Clone the repository
git clone https://github.com/muvon/octomind.git
cd octomind

# 2. Set your AI provider API key
export OPENROUTER_API_KEY="your_key_here"
# OR: export OPENAI_API_KEY="your_key" / ANTHROPIC_API_KEY="your_key"

# 3. Quick compilation check (fastest - ALWAYS use this first)
cargo check --message-format=short

# 4. Fix any code quality issues (treat warnings as errors)
cargo clippy --all-features --all-targets -- -D warnings

# 5. Build for development (only when you need the binary)
cargo build

# 6. Test the installation
./target/debug/octomind --version

# 7. Start your first development session
./target/debug/octomind session
```

### Essential First Commands
Once in a session, try these commands to understand the system:
```
/help                    # Show all available commands
/info                    # Check token usage and costs
/mcp info               # Check MCP tool status
/model                  # See current AI model
/role                   # Check current role (developer/assistant)
/config --show          # View current configuration
```

### Development Workflow
```bash
# Daily development cycle
cargo check --message-format=short    # Fast syntax check (PREFERRED)
cargo clippy --all-features --all-targets -- -D warnings  # Fix quality issues
cargo build                          # Build when needed (debug mode)

# NEVER use these during development (too slow):
# cargo build --release              # Extremely slow
# cargo test --release               # Unnecessary for development
```

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
- **ALWAYS use `cargo check --message-format=short`** for syntax/compilation verification - fastest option
- **NEVER use `cargo build --release`** - extremely slow, wastes development time
- **Use `cargo build` (debug)** only when you need to run the actual binary
- **Focus on `cargo check --message-format=short`** for iterative development and validation
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
**Provider System (octolib-powered):**
- `src/providers/mod.rs` - Provider adapter bridge to octolib library
- All provider implementations now in octolib crate (OpenRouter, OpenAI, Anthropic, etc.)

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

### Configuration Not Loading
1. **Template injection**: `src/config/loading.rs` → `load()`
2. **Environment overrides**: Check `OCTOMIND_*` variables
3. **Validation errors**: `src/config/validation.rs` → `validate()`

### Session Commands Failing
1. **Command routing**: `src/session/chat/session/commands/mod.rs`
2. **Individual commands**: `src/session/chat/session/commands/*.rs`
3. **Command permissions**: Check role configuration

### Provider/Model Issues
1. **Provider bridge**: `src/providers/mod.rs` - octolib adapter layer
2. **API keys**: Check `*_API_KEY` environment variables
3. **Model format**: Must be `provider:model` format
4. **octolib dependency**: Check Cargo.toml for octolib version

### Layer Processing Issues
1. **Layer execution**: `src/session/layers/types/generic.rs` → `process()`
2. **Layer orchestration**: `src/session/layers/orchestrator.rs`
3. **Input/output modes**: Check layer config in template

### Cache/Cost Issues
1. **Cache markers**: `src/session/cache.rs` → `manage_content_cache_markers()`
2. **Cost tracking**: `src/session/chat/cost_tracker.rs` → `track_exchange_cost()`
3. **Token counting**: `src/session/mod.rs` SessionInfo struct
4. **Cache support**: Check if model supports caching (Anthropic Claude, etc.)

## 🆘 COMPREHENSIVE TROUBLESHOOTING

### Common Error Patterns & Solutions

**1. Compilation Errors**
```bash
# Error: "cannot find crate"
cargo clean && cargo check --message-format=short

# Error: "trait bound not satisfied"
# Check Cargo.toml dependencies and feature flags
cargo update

# Error: "macro not found"
# Check use statements and macro imports
```

**2. Configuration Issues**
```bash
# Error: "Failed to load config"
octomind config --validate                        # Check config syntax
octomind config --show                           # View current config
rm ~/.config/octomind/config.toml                # Reset to defaults

# Error: "Invalid model format"
# Use: provider:model (e.g., "openrouter:anthropic/claude-sonnet-4")
# Not: just "claude-sonnet-4"

# Error: "API key not found"
export OPENROUTER_API_KEY="your_key"             # Set API key
octomind vars                                     # Check environment variables
```

**3. Session Problems**
```bash
# Error: "Session failed to start"
/loglevel debug                                   # Enable debug logging
/mcp info                                         # Check MCP server status
/role developer                                   # Ensure correct role

# Error: "Tool not found"
/mcp list                                         # List available tools
# Check allowed_tools in role configuration

# Error: "Context too large"
/truncate                                         # Manually truncate context
/cache                                            # Add cache checkpoint
# Adjust max_session_tokens_threshold in config
```

**4. MCP Tool Issues**
```bash
# Error: "MCP server not responding"
/mcp health                                       # Check server health
/mcp validate                                     # Validate server configs
# Check server logs in ~/.local/share/octomind/logs/

# Error: "Tool execution failed"
# Check tool parameter validation in src/mcp/*/functions.rs
# Ensure proper McpToolResult::error() returns

# Error: "Permission denied"
# Check allowed_tools patterns in role/layer config
# Verify server_refs include required servers
```

**5. Provider/API Issues**
```bash
# Error: "Rate limit exceeded"
# Wait and retry, or switch to different provider
export OCTOMIND_MODEL="deepseek:deepseek-chat"   # Use cheaper model

# Error: "Invalid API key"
# Verify API key format and permissions
curl -H "Authorization: Bearer $OPENROUTER_API_KEY" https://openrouter.ai/api/v1/models

# Error: "Model not found"
# Check available models for your provider
# Use exact model names from provider documentation
```

**6. Performance Issues**
```bash
# Slow compilation
cargo check --message-format=short               # Use instead of cargo build
export CARGO_INCREMENTAL=1                       # Enable incremental compilation

# High memory usage
/info                                             # Check token usage
/truncate                                         # Reduce context size
# Lower max_tokens in config

# Slow responses
# Use faster models for development
export OCTOMIND_MODEL="openrouter:anthropic/claude-haiku"
```

### Debug Commands & Techniques

**Enable Debug Logging:**
```bash
# In session
/loglevel debug

# Environment variable
export OCTOMIND_LOG_LEVEL="debug"

# Check logs
tail -f ~/.local/share/octomind/logs/octomind.log
```

**MCP Debugging:**
```bash
/mcp info                                         # Server status overview
/mcp list                                         # Available tools
/mcp full                                         # Detailed server info
/mcp health                                       # Health check all servers
/mcp dump                                         # Full diagnostic dump
/mcp validate                                     # Validate configurations
```

**Session Debugging:**
```bash
/info                                             # Token usage and costs
/context all                                      # View full context
/context tool                                     # View tool calls only
/report                                           # Detailed usage report
```

**Configuration Debugging:**
```bash
octomind config --show                           # Current configuration
octomind config --validate                       # Validate config
octomind vars                                     # Environment variables
```

### Recovery Procedures

**Reset Configuration:**
```bash
# Backup current config
cp ~/.config/octomind/config.toml ~/.config/octomind/config.toml.backup

# Reset to defaults
rm ~/.config/octomind/config.toml
octomind config

# Restore specific settings
octomind config --model "your_preferred_model"
octomind config --api-key "provider:your_key"
```

**Clean Session State:**
```bash
# Clear session cache
rm -rf ~/.local/share/octomind/sessions/

# Clear logs
rm -rf ~/.local/share/octomind/logs/

# Fresh start
octomind session --name fresh_start
```

**Emergency Debugging:**
```bash
# Maximum debug information
export OCTOMIND_LOG_LEVEL="debug"
export RUST_BACKTRACE=1
export RUST_LOG=debug

# Run with full logging
octomind session --name debug_session 2>&1 | tee debug.log
```

## ⚠️ COMMON PITFALLS FOR NEW DEVELOPERS

### Code Quality Pitfalls
```rust
// ❌ NEVER DO - Hiding errors with fallbacks
let config = if let Ok(cfg) = load_config() {
    cfg
} else {
    default_config() // This hides the real error!
};

// ✅ ALWAYS DO - Expose problems immediately
let config = load_config()
    .expect("CRITICAL: Failed to load config - fix the underlying issue");

// ❌ NEVER DO - Using println!() for debug
println!("DEBUG: something happened");

// ✅ ALWAYS DO - Use proper logging
crate::log_debug!("Something happened");

// ❌ NEVER DO - Returning Err() from MCP tools
pub async fn my_tool(call: &McpToolCall) -> Result<McpToolResult, anyhow::Error> {
    if validation_fails {
        return Err(anyhow::anyhow!("Validation failed")); // WRONG!
    }
}

// ✅ ALWAYS DO - Return Ok(McpToolResult::error())
pub async fn my_tool(call: &McpToolCall) -> Result<McpToolResult, anyhow::Error> {
    if validation_fails {
        return Ok(McpToolResult::error(
            call.tool_name.clone(),
            call.tool_id.clone(),
            "Validation failed".to_string(),
        ));
    }
}
```

### Development Workflow Pitfalls
```bash
# ❌ NEVER DO - Slow development commands
cargo build --release              # Extremely slow, avoid during development
cargo test --release               # Unnecessary for development

# ✅ ALWAYS DO - Fast development cycle
cargo check --message-format=short # Fastest syntax/compilation check (PREFERRED)
cargo clippy --all-features --all-targets -- -D warnings  # Fix ALL warnings
cargo build                        # Only when you need the actual binary

# ❌ NEVER DO - Ignore clippy warnings
cargo clippy                       # Allows warnings to pass

# ✅ ALWAYS DO - Treat warnings as errors
cargo clippy --all-features --all-targets -- -D warnings
```

### Configuration Pitfalls
```bash
# ❌ NEVER DO - Hardcode configuration values
const DEFAULT_MODEL = "gpt-4";     # Hardcoded in source

# ✅ ALWAYS DO - Use config-templates/default.toml
model = "openrouter:anthropic/claude-sonnet-4"  # In template

# ❌ NEVER DO - Modify system-wide configs during development
# Don't create example configs or modify existing structures

# ✅ ALWAYS DO - Test with environment overrides
export OCTOMIND_MODEL="test:model"
export OCTOMIND_LOG_LEVEL="debug"
```

### Session Development Pitfalls
```bash
# ❌ NEVER DO - Run tests that affect global configuration
cargo test config_integration      # May create session files

# ✅ ALWAYS DO - Focus on compilation and code quality
cargo check --message-format=short
cargo clippy --all-features --all-targets -- -D warnings

# ❌ NEVER DO - Assume file paths without checking
# File paths change - always verify against actual codebase

# ✅ ALWAYS DO - Use the project structure guide in this document
```

### MCP Tool Development Pitfalls
```rust
// ❌ NEVER DO - Skip parameter validation
let param = call.parameters.get("param").unwrap().as_str().unwrap();

// ✅ ALWAYS DO - Proper MCP-compliant validation
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
```

## 🎓 NEW DEVELOPER CHECKLIST

### Before You Start
- [ ] Rust 1.82+ installed (`rustc --version`)
- [ ] AI provider API key set (`echo $OPENROUTER_API_KEY`)
- [ ] Git configured for commits
- [ ] Read this entire INSTRUCTIONS.md document
- [ ] Understand the core architecture principles

### First Day Setup
- [ ] Clone repository: `git clone https://github.com/muvon/octomind.git`
- [ ] Quick compilation check: `cargo check --message-format=short`
- [ ] Fix any warnings: `cargo clippy --all-features --all-targets -- -D warnings`
- [ ] Build for testing: `cargo build`
- [ ] Test installation: `./target/debug/octomind --version`
- [ ] Start first session: `./target/debug/octomind session`
- [ ] Try basic commands: `/help`, `/info`, `/mcp info`

### Daily Development Routine
- [ ] Start with: `cargo check --message-format=short`
- [ ] Fix warnings: `cargo clippy --all-features --all-targets -- -D warnings`
- [ ] Make changes following the patterns in this guide
- [ ] Test changes: `cargo build` (only when needed)
- [ ] Commit with clear messages

### Before Submitting Changes
- [ ] All clippy warnings fixed (treat as errors)
- [ ] No hardcoded values (use config-templates/default.toml)
- [ ] MCP tools return proper McpToolResult::error() for failures
- [ ] No println!() debug statements (use proper logging)
- [ ] Configuration errors fail fast with .expect()
- [ ] File paths validated against actual codebase
- [ ] Changes tested in actual session

### Understanding the Codebase
- [ ] Read `src/main.rs` - CLI entry point
- [ ] Understand `src/config/mod.rs` - Configuration system
- [ ] Explore `src/session/mod.rs` - Session management
- [ ] Study `src/mcp/mod.rs` - Tool system
- [ ] Review `config-templates/default.toml` - All defaults
- [ ] Check `src/providers/mod.rs` - AI provider system

### Getting Help
- [ ] Use `/loglevel debug` in sessions for detailed logging
- [ ] Check `~/.local/share/octomind/logs/` for log files
- [ ] Use `/mcp info` to debug tool issues
- [ ] Reference this INSTRUCTIONS.md for patterns
- [ ] Ask specific questions with error messages and context

## 🚀 COMMON MODIFICATIONS

### Add New MCP Tool
1. **Function definition**: Add to `src/mcp/*/functions.rs`
2. **Implementation**: Add to `src/mcp/*/` (core.rs or new file)
3. **Routing**: Update `src/mcp/mod.rs` → `try_execute_tool_call()` with proper error handling
4. **CRITICAL**: Return `Ok(McpToolResult::error())` for all failures, never `Err()`
5. **Parameter validation**: Use proper MCP-compliant validation patterns

**Example: Adding a New Tool to Filesystem Server**

**Step 1: Define Function in `src/mcp/fs/functions.rs`**
```rust
pub fn get_my_new_tool_function() -> McpFunction {
    McpFunction {
        name: "my_new_tool".to_string(),
        description: "Description of what this tool does.

        Parameters:
        - `required_param`: Description of required parameter
        - `optional_param`: Description of optional parameter (default: value)

        Examples:
        - Basic usage: `{\"required_param\": \"value\"}`
        - With options: `{\"required_param\": \"value\", \"optional_param\": \"custom\"}`
        ".to_string(),
        parameters: json!({
            "type": "object",
            "required": ["required_param"],
            "properties": {
                "required_param": {
                    "type": "string",
                    "description": "Required parameter description"
                },
                "optional_param": {
                    "type": "string",
                    "default": "default_value",
                    "description": "Optional parameter description"
                }
            }
        }),
    }
}

// Add to get_all_functions()
pub fn get_all_functions() -> Vec<McpFunction> {
    vec![
        get_text_editor_function(),
        get_list_files_function(),
        get_my_new_tool_function(),  // Add here
        // ... other functions
    ]
}
```

**Step 2: Implement Tool Logic in `src/mcp/fs/core.rs`**
```rust
use crate::mcp::McpToolResult;
use serde_json::Value;

pub async fn execute_my_new_tool(
    call: &crate::mcp::McpToolCall,
    _cancellation_token: tokio::sync::watch::Receiver<bool>,
) -> Result<McpToolResult, anyhow::Error> {
    // ✅ CORRECT MCP-compliant parameter validation
    let required_param = match call.parameters.get("required_param") {
        Some(Value::String(p)) => {
            if p.trim().is_empty() {
                return Ok(McpToolResult::error(
                    call.tool_name.clone(),
                    call.tool_id.clone(),
                    "Parameter 'required_param' cannot be empty".to_string(),
                ));
            }
            p.clone()
        }
        Some(_) => {
            return Ok(McpToolResult::error(
                call.tool_name.clone(),
                call.tool_id.clone(),
                "Parameter 'required_param' must be a string".to_string(),
            ));
        }
        None => {
            return Ok(McpToolResult::error(
                call.tool_name.clone(),
                call.tool_id.clone(),
                "Missing required parameter 'required_param'".to_string(),
            ));
        }
    };

    let optional_param = call.parameters.get("optional_param")
        .and_then(|v| v.as_str())
        .unwrap_or("default_value");

    // Implement your tool logic here
    match perform_tool_operation(&required_param, optional_param).await {
        Ok(result) => {
            Ok(McpToolResult::success(
                call.tool_name.clone(),
                call.tool_id.clone(),
                result,
            ))
        }
        Err(e) => {
            Ok(McpToolResult::error(
                call.tool_name.clone(),
                call.tool_id.clone(),
                format!("Tool execution failed: {}", e),
            ))
        }
    }
}

async fn perform_tool_operation(
    required_param: &str,
    optional_param: &str,
) -> Result<String, anyhow::Error> {
    // Your actual tool implementation
    Ok(format!("Tool executed with {} and {}", required_param, optional_param))
}
```

**Step 3: Add Tool Routing in `src/mcp/mod.rs`**
```rust
// In try_execute_tool_call function, add your tool case:
match (server_name, tool_name) {
    // ... existing cases
    ("filesystem", "my_new_tool") => {
        match fs::core::execute_my_new_tool(call, cancellation_token).await {
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
    }
    // ... other cases
}
```

**Step 4: Update Tool Server Map in `src/mcp/tool_map.rs`**
```rust
// Add your tool to the appropriate server mapping
pub fn build_tool_server_map() -> HashMap<String, String> {
    let mut map = HashMap::new();

    // Filesystem tools
    map.insert("text_editor".to_string(), "filesystem".to_string());
    map.insert("list_files".to_string(), "filesystem".to_string());
    map.insert("my_new_tool".to_string(), "filesystem".to_string());  // Add here

    // ... other mappings
    map
}
```

**Step 5: Test Your Tool**
```bash
# Build and test
cargo check --message-format=short
cargo clippy --all-features --all-targets -- -D warnings

# Start session and test
octomind session
/mcp list                                         # Verify tool is listed
my_new_tool(required_param="test")               # Test the tool
```

**Real Tool Examples from Codebase:**

**1. Shell Tool (Developer Server)**
```rust
// From src/mcp/dev/shell.rs
pub fn get_shell_function() -> McpFunction {
    McpFunction {
        name: "shell".to_string(),
        description: "Execute a command in the shell...".to_string(),
        parameters: json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "background": {
                    "type": "boolean",
                    "default": false,
                    "description": "Run command in background"
                }
            }
        }),
    }
}
```

**2. AST Grep Tool (Developer Server)**
```rust
// From src/mcp/dev/ast_grep.rs
pub fn get_ast_grep_function() -> McpFunction {
    McpFunction {
        name: "ast_grep".to_string(),
        description: "Search and refactor code using AST patterns...".to_string(),
        parameters: json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The AST pattern to search for"
                },
                "language": {
                    "type": "string",
                    "description": "Optional language of the code"
                },
                "rewrite": {
                    "type": "string",
                    "description": "Optional rewrite pattern"
                }
            }
        }),
    }
}
```

**3. List Files Tool (Filesystem Server)**
```rust
// From src/mcp/fs/functions.rs
pub fn get_list_files_function() -> McpFunction {
    McpFunction {
        name: "list_files".to_string(),
        description: "List files in a directory, with optional pattern matching...".to_string(),
        parameters: json!({
            "type": "object",
            "required": ["directory"],
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "The directory to list files from"
                },
                "pattern": {
                    "type": "string",
                    "description": "Optional pattern to match filenames"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum depth of directories to descend"
                }
            }
        }),
    }
}
```

### Add New Provider
**Note**: Providers are now managed through the octolib library. To add a new provider:
1. **Contribute to octolib**: Add provider implementation to the octolib crate
2. **Update Octomind**: Update octolib dependency version in Cargo.toml
3. **Re-export**: Add provider to re-exports in `src/providers/mod.rs`
4. **Config**: Add provider section to template if needed

### Add New Configuration
1. **Template first**: Add to `config-templates/default.toml`
2. **Struct**: Add to appropriate `src/config/*.rs`
3. **Validation**: Add validation rules

### Add New Layer
1. **Config**: Add to template `[[layers]]` section
2. **No code needed**: Uses generic layer implementation
3. **Configure**: Set input_mode, output_mode, MCP access

### Add New Session Command
1. **Constant**: Add to `src/session/chat/commands.rs` and update COMMANDS array count
2. **Handler**: Create `src/session/chat/session/commands/[name].rs` with handle_[name]() function
3. **Module**: Add `mod [name];` to `src/session/chat/session/commands/mod.rs`
4. **Routing**: Add command to process_command match statement
5. **Help**: Update handle_unknown_command() help text
6. **Persistence** (optional): Add to SessionRuntimeState and apply_command_to_runtime_state()

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

### Use File Parsing and Rendering Utilities
1. **File Parser**: `src/utils/file_parser.rs` - **REUSABLE ACROSS CODEBASE**
   - `parse_file_references(content: &str) -> HashMap<String, Vec<LineRange>>` - Parse filepath:start:end patterns
   - `read_file_lines(filepath: &str, range: &LineRange) -> FileContent` - Read specific line ranges
   - `read_multiple_files(file_refs: &HashMap<String, Vec<LineRange>>) -> HashMap<String, Vec<FileContent>>` - Batch processing
   - `has_context_blocks(text: &str) -> bool` - **NEW**: Fast detection of `<context>` tags using pre-compiled regex
   - Supports code blocks, sections, inline patterns with comprehensive error handling
2. **File Renderer**: `src/utils/file_renderer.rs` - **XML AND TEXT FORMATS**
   - `render_files_as_xml(file_contents: &HashMap<String, Vec<FileContent>>) -> String` - XML format with escaping
   - `render_files_as_text(file_contents: &HashMap<String, Vec<FileContent>>) -> String` - Traditional text format
   - `render_files_with_options(file_contents, options: &RenderOptions) -> String` - Configurable rendering
   - `expand_context_blocks(text: &str) -> String` - **NEW**: Expand `<context>file:1:3</context>` to full XML
   - XML format: `<content path="file.rs" lines="10:20">content</content>` with proper escaping
3. **Integration**: Import and use in any module that needs file content display
   ```rust
   use crate::utils::file_parser::{parse_file_references, read_multiple_files, has_context_blocks};
   use crate::utils::file_renderer::{render_files_as_xml, expand_context_blocks};
   ```

### Unified Context Protocol
**NEW FEATURE**: Automatic expansion of file references in layer and agent responses.

**Format**: `<context>src/main.rs:10:20\nsrc/lib.rs:30:40</context>`
**Output**: Full XML with actual file content

**Where It Works**:
- ✅ **Layer responses** - Expanded in `src/session/layers/orchestrator.rs`
- ✅ **MCP agent tools** - Expanded in `src/mcp/agent/functions.rs`
- ❌ **Main session** - No expansion (not needed)
- ❌ **Continuation** - Generates full XML directly

**Implementation**:
```rust
// Fast detection
if has_context_blocks(response) {
    let expanded = expand_context_blocks(response);
    // Use expanded content
}
```

**Key Files**:
- `src/utils/file_parser.rs` - Detection helper
- `src/utils/file_renderer.rs` - Expansion processor
- `src/session/layers/orchestrator.rs` - Layer integration
- `src/mcp/agent/functions.rs` - Agent integration
- `src/session/chat/continuation/constants.rs` - Updated prompt format

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

### Project Structure Overview
```
octomind/
├── src/
│   ├── main.rs                    # CLI entry point
│   ├── lib.rs                     # Library exports
│   ├── state.rs                   # Global application state
│   ├── directories.rs             # System directory management
│   ├── commands/                  # CLI command implementations
│   │   ├── mod.rs                 # Command routing
│   │   ├── config.rs              # Config management commands
│   │   ├── session.rs             # Session management commands
│   │   └── completion.rs          # Shell completion generation
│   ├── config/                    # Configuration system
│   │   ├── mod.rs                 # Main Config struct
│   │   ├── loading.rs             # Config loading & template injection
│   │   ├── validation.rs          # Config validation rules
│   │   ├── migrations.rs          # Config version migrations
│   │   ├── roles.rs               # Role-based configurations
│   │   ├── layers.rs              # Layer configurations
│   │   ├── mcp.rs                 # MCP server configurations
│   │   ├── providers.rs           # AI provider configurations
│   │   └── env_source.rs          # Environment variable handling
│   ├── session/                   # Interactive session system
│   │   ├── mod.rs                 # Session management & utilities
│   │   ├── cache.rs               # 2-marker caching system
│   │   ├── cancellation.rs        # Cancellation token management
│   │   ├── image.rs               # Image processing utilities
│   │   ├── report.rs              # Usage reporting
│   │   ├── smart_summarizer.rs    # Context summarization
│   │   ├── token_counter.rs       # Token counting utilities
│   │   ├── chat/                  # Chat session implementation
│   │   │   ├── mod.rs             # Chat module exports
│   │   │   ├── response.rs        # Response processing orchestrator
│   │   │   ├── cost_tracker.rs    # Cost accumulation system
│   │   │   ├── context_truncation.rs # Smart context management
│   │   │   ├── session_continuation.rs # Legacy continuation API
│   │   │   ├── continuation/      # NEW: Modular continuation system
│   │   │   │   ├── mod.rs         # Module coordinator
│   │   │   │   ├── detection.rs   # Continuation trigger logic
│   │   │   │   ├── injection.rs   # Summary request injection
│   │   │   │   ├── processing.rs  # Response processing
│   │   │   │   ├── file_context.rs # File parsing & context
│   │   │   │   └── constants.rs   # Prompts & templates
│   │   │   └── session/           # Core session logic
│   │   │       ├── core.rs        # ChatSession structure
│   │   │       ├── runner.rs      # Main interactive loop
│   │   │       └── commands/      # Session command handlers
│   │   └── layers/                # Layered AI processing
│   │       ├── mod.rs             # Layer system exports
│   │       ├── orchestrator.rs    # Layer execution orchestration
│   │       ├── layer_trait.rs     # Layer configuration traits
│   │       └── types/
│   │           └── generic.rs     # Generic layer implementation
│   ├── mcp/                       # MCP (Muvon Copilot Protocol) tools
│   │   ├── mod.rs                 # Tool routing & execution
│   │   ├── tool_map.rs            # Static tool-to-server mapping
│   │   ├── server.rs              # HTTP server communication
│   │   ├── process.rs             # Stdin server process management
│   │   ├── health_monitor.rs      # Server health monitoring
│   │   ├── shared_utils.rs        # Common MCP utilities
│   │   ├── agent/                 # Agent system (routes to layers)
│   │   │   └── functions.rs       # Dynamic agent_<name> tools
│   │   ├── dev/                   # Developer tools
│   │   │   ├── functions.rs       # Tool definitions
│   │   │   └── core.rs            # Shell, ast_grep implementations
│   │   ├── fs/                    # Filesystem tools
│   │   │   ├── functions.rs       # Tool definitions
│   │   │   └── core.rs            # text_editor, list_files, etc.
│   │   └── web/                   # Web tools
│   │       ├── functions.rs       # Tool definitions
│   │       └── core.rs            # web_search, read_html, etc.
│   ├── providers/                 # AI provider bridge to octolib
│   │   └── mod.rs                 # Provider adapter & octolib integration
│   └── utils/                     # Utility modules
│       ├── mod.rs                 # Common utilities exports
│       ├── file_parser.rs         # File reference parsing utilities
│       ├── file_renderer.rs       # XML/text file content rendering
│       ├── glob.rs                # Glob pattern utilities
│       └── truncation.rs          # Text truncation utilities
├── config-templates/              # Configuration templates
│   └── default.toml               # ALL default settings & structure
├── doc/                          # Comprehensive documentation
│   ├── README.md                 # Documentation index
│   ├── 01-installation.md        # Installation guide
│   ├── 02-overview.md            # Project overview
│   ├── 03-configuration.md       # Configuration guide
│   ├── 04-providers.md           # AI provider setup
│   ├── 05-sessions.md            # Session usage guide
│   ├── 06-advanced.md            # Advanced features
│   ├── 07-command-layers.md      # Command & layer system
│   └── 08-mcp-server-development.md # MCP development
├── Cargo.toml                    # Rust project configuration
├── README.md                     # Project overview & quick start
├── INSTRUCTIONS.md               # This developer guide
└── CHANGELOG.md                  # Version history
```

### File Patterns
- **Config**: `src/config/*.rs` + `config-templates/default.toml`
- **Tools**: `src/mcp/*/functions.rs` + `src/mcp/*/core.rs`
- **Sessions**: `src/session/chat/` + `src/session/layers/`
- **Continuation**: `src/session/chat/continuation/` (modular architecture)
- **Providers**: `src/providers/mod.rs` (octolib bridge)
- **Commands**: `src/commands/*.rs` + `src/session/chat/session/commands/*.rs`

### Environment Variables
**AI Provider API Keys:**
```bash
# Primary providers (choose one or more)
export OPENROUTER_API_KEY="sk-or-v1-..."           # Multi-provider access
export OPENAI_API_KEY="sk-..."                     # Direct OpenAI API
export ANTHROPIC_API_KEY="sk-ant-..."              # Direct Anthropic API
export GOOGLE_API_KEY="AIza..."                    # Google Vertex AI
export AMAZON_ACCESS_KEY_ID="AKIA..."              # Amazon Bedrock
export AMAZON_SECRET_ACCESS_KEY="..."              # Amazon Bedrock
export CLOUDFLARE_API_TOKEN="..."                  # Cloudflare Workers AI
export DEEPSEEK_API_KEY="sk-..."                   # DeepSeek API

# Web search (optional)
export BRAVE_API_KEY="BSA..."                      # For web_search tool
```

**Configuration Overrides:**
Any setting in `config-templates/default.toml` can be overridden with `OCTOMIND_*` variables:
```bash
# System-wide settings
export OCTOMIND_LOG_LEVEL="debug"                  # none, info, debug
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_MAX_TOKENS="8192"
export OCTOMIND_CUSTOM_INSTRUCTIONS_FILE_NAME="MY_INSTRUCTIONS.md"

# Performance settings
export OCTOMIND_MCP_RESPONSE_WARNING_THRESHOLD="5000"
export OCTOMIND_MAX_SESSION_TOKENS_THRESHOLD="100000"
export OCTOMIND_CACHE_TOKENS_THRESHOLD="1024"

# Role-specific overrides (nested with double underscores)
export OCTOMIND_ROLES__DEVELOPER__MODEL="openai:gpt-4o"
export OCTOMIND_ROLES__DEVELOPER__TEMPERATURE="0.1"
export OCTOMIND_ROLES__ASSISTANT__MODEL="deepseek:deepseek-chat"

# Layer overrides
export OCTOMIND_LAYERS__TASK_REFINER__MODEL="openrouter:anthropic/claude-haiku"
export OCTOMIND_LAYERS__TASK_RESEARCHER__MAX_TOKENS="4096"

# MCP server overrides
export OCTOMIND_MCP__SERVERS__OCTOCODE__TIMEOUT_SECONDS="300"
```

**Configuration Examples:**
```toml
# Example: Custom developer role with specific model
[[roles]]
name = "senior_dev"
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.1
max_tokens = 16384
enable_layers = true
system = "You are a senior developer focused on code quality and best practices."
layer_refs = ["task_refiner", "task_researcher"]

[roles.senior_dev.mcp]
server_refs = ["developer", "filesystem", "web", "octocode"]
allowed_tools = ["developer:*", "filesystem:*", "web_search", "octocode:*"]

# Example: Custom layer for code review
[[layers]]
name = "code_reviewer"
model = "openrouter:anthropic/claude-sonnet-4"
temperature = 0.0
max_tokens = 8192
input_mode = "direct"
output_mode = "direct"
system = "You are a code reviewer focused on security, performance, and maintainability."

[layers.code_reviewer.mcp]
server_refs = ["filesystem", "developer"]
allowed_tools = ["text_editor", "view_signatures", "ast_grep"]

# Example: Custom command using layers
[[commands]]
name = "review"
layer_name = "code_reviewer"
description = "Perform comprehensive code review"
```

### Key Commands

**CLI Commands:**
```bash
# Configuration management
octomind config                                    # Generate default config
octomind config --show                            # View current configuration
octomind config --validate                        # Validate configuration
octomind config --model "openrouter:anthropic/claude-sonnet-4"  # Set default model
octomind config --api-key "openrouter:your-key"   # Set API key
octomind config --log-level debug                 # Set log level
octomind config --system "Custom system prompt"   # Set system prompt
octomind config --markdown-enable true            # Enable markdown rendering
octomind config --mcp-server "myserver,url=http://localhost:3000/mcp"  # Add MCP server

# Session management
octomind session                                   # Start new session (developer role)
octomind session --role assistant                 # Start assistant session (chat-only)
octomind session --name my_project               # Start named session
octomind session --resume my_project             # Resume existing session
octomind session --model "openai:gpt-4o"         # Use specific model
octomind session --temperature 0.1               # Set temperature
octomind session --max-tokens 8192               # Set max tokens
octomind session --max-retries 5                 # Set retry limit

# Other commands
octomind ask "How does authentication work?"       # One-shot question
octomind run <layer_name> "Task description"      # Execute specific layer
octomind shell                                     # Interactive shell mode
octomind server                                    # Start WebSocket server (default: localhost:8080)
octomind server --host 0.0.0.0 --port 3000        # WebSocket server with custom host/port
octomind vars                                      # Show environment variables
octomind completion bash > ~/.bash_completion.d/octomind  # Shell completion
```

**Session Commands (within interactive sessions):**
```bash
# Core session commands
/help                                             # Show available commands
/info                                             # Display token usage and costs
/report                                           # Generate detailed usage report
/context [filter]                                 # Display session context (all, assistant, user, tool, large)
/model [model]                                    # View or change current AI model
/role [role_name]                                 # View or change current role

# File and image operations
/image <path>                                     # Attach image to next message (PNG, JPEG, GIF, WebP, BMP)
/save                                             # Save current session
/clear                                            # Clear terminal screen
/copy                                             # Copy last assistant response to clipboard

# Context and memory management
/cache                                            # Mark cache checkpoint for cost savings
/summarize                                        # Generate session summary
/truncate                                         # Manually truncate session context
/done                                             # Finalize task with memorization and auto-commit

# Layer and tool management
/layers                                           # Toggle layered processing on/off
/run <command>                                    # Execute configured custom commands
/mcp [info|list|full|health|dump|validate]       # MCP server management and debugging

# System and debugging
/loglevel [debug|info|none]                      # Set log level
/exit                                             # Exit current session
```

**MCP Tool Usage (within sessions):**
```bash
# Built-in developer tools
shell(command="ls -la")                           # Execute shell commands
ast_grep(pattern="fn $NAME($ARGS)", language="rust")  # Search code patterns

# Filesystem tools
text_editor(command="view", path="src/main.rs")   # View/edit files
list_files(directory="src", pattern="*.rs")       # List files
batch_edit(path="file.rs", operations=[...])      # Batch file operations

# Web tools
web_search(query="rust async programming")        # Search the web
read_html(sources=["https://example.com"])        # Convert HTML to markdown

# Agent tools (route to specialized AI layers)
agent_context_gatherer(task="Analyze auth system")  # Context gathering
agent_task_refiner(task="Improve error handling")   # Task refinement
agent_task_researcher(task="Research best practices")  # Research tasks
```

### Development Workflow
- **Build Check**: `cargo check --message-format=short` - fastest compilation verification (PREFERRED)
- **Code Quality**: `cargo clippy --all-features --all-targets -- -D warnings` - fix ALL code quality issues (treat warnings as errors)
- **Debug Build**: `cargo build` - only when you need to run the actual binary
- **NEVER**: `cargo build --release` - extremely slow, avoid during development
- **NEVER**: Modify configs, create tests, or affect global configuration
