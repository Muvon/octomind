# Use Case: Multi-Agent Task Delegation

Split complex tasks across specialized AI agents that work independently and report back to a coordinator.

## The Problem

A single AI call struggles with large tasks: "Refactor the authentication system." It tries to do everything at once, loses context, and produces incomplete results. You want specialized agents -- one gathers context, one reviews code, one plans architecture -- working in parallel.

## Solution

Configure multiple agents, each with its own role and tools. The main session delegates to them and synthesizes results.

### Step 1: Define Agent Roles

```toml
# Roles for each agent (in config.toml)

[[roles]]
name = "context_gatherer"
temperature = 0.2
system = """
You are a codebase researcher. Your job is to:
1. Find all relevant files for the given task
2. Read key interfaces and function signatures
3. Note patterns, conventions, and dependencies
4. Report findings concisely

Use tools to search and read code. Be thorough but focused.
{{CWD}}
"""

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:ast_grep"]

[[roles]]
name = "code_reviewer"
temperature = 0.1
system = """
You are a senior code reviewer. Analyze code for:
- Security vulnerabilities
- Performance issues
- Design pattern violations
- Error handling gaps

Be specific: file, line, issue, suggestion.
{{CWD}}
"""

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"]
```

### Step 2: Configure Agents

```toml
[[agents]]
name = "context_gatherer"
description = "Gathers codebase context: files, interfaces, patterns, dependencies."
command = "octomind acp context_gatherer"
workdir = "."

[[agents]]
name = "code_reviewer"
description = "Reviews code for security, performance, and design issues."
command = "octomind acp code_reviewer"
workdir = "."
```

### Step 3: Use in Session

Start a session and delegate:

```bash
octomind run developer
```

The main AI can now use these agents as tools:

```
> Refactor the authentication module to support OAuth2

AI thinking: "This is complex. Let me gather context first."

# AI calls agent_context_gatherer(task="Find all auth-related files,
#   interfaces, and patterns in the codebase")
# Agent runs independently, reads files, returns findings

# AI calls agent_code_reviewer(task="Review src/auth/ for security
#   issues that should be addressed during the refactor")
# Agent runs independently, reviews code, returns issues

# Main AI now has:
# - Full context from context_gatherer
# - Security issues from code_reviewer
# - Can produce a comprehensive refactoring plan
```

### Parallel Execution with Async Agents

For large tasks, run agents in parallel:

```
> Analyze the entire codebase for the quarterly security audit

AI:
# Dispatches agents concurrently:
agent_context_gatherer(task="Map all external API endpoints", async=true)
agent_code_reviewer(task="Scan for OWASP Top 10 vulnerabilities", async=true)

# While agents work, AI continues with other analysis
# Results appear as inbox messages when agents complete:
# "[Async agent 'context_gatherer' completed]"
# "[Async agent 'code_reviewer' completed]"
```

### Tap Roles (no config needed)

If a tap registry already provides a specialist role for the sub-task, use the `tap` core tool instead of defining your own `[[agents]]`:

```json
// Discover, then delegate — no config edits, no subprocess setup.
{"action": "discover", "intent": "review code for OWASP Top 10 issues"}
{"action": "run", "role": "security:owasp", "prompt": "Audit src/auth/ for OWASP issues", "background": true}
```

Tap roles share their own system prompt + model + tool kit. `background: true` returns the run id immediately and the reply lands as a user message in the next turn. Resume with `{"action": "run", "session": "<id>", "prompt": "follow-up question"}`. See [Tap System](../integration/04-tap-system.md) and [MCP Tools — `tap`](../usage/07-mcp-tools.md#tap----run-specialist-roles-from-taps).

Use `[[agents]]` (this page) when the role doesn't exist in any tap or you need a custom local-only agent. Use `tap` when a community-maintained role already covers the task.

### Dynamic Agents

Create agents on the fly during a session using the `agent` runtime tool (formerly under `core`):

```json
// AI creates a specialized agent at runtime
{"action": "add", "name": "test_writer",
 "description": "Writes unit tests for given code",
 "system": "You write comprehensive unit tests. Focus on edge cases and error paths.",
 "server_refs": ["filesystem"],
 "allowed_tools": ["filesystem:view", "filesystem:text_editor"]}

{"action": "enable", "name": "test_writer"}

// Now agent_test_writer is available as a tool
```

## Example: Full Development Pipeline

```
User: "Add rate limiting to the API endpoints"

Main AI:
  1. Calls agent_context_gatherer:
     "Find all API endpoint handlers, middleware patterns, and existing rate limiting code"
     -> Returns: file list, handler signatures, middleware chain pattern

  2. Calls agent_code_reviewer:
     "Review the current API middleware for potential issues with adding rate limiting"
     -> Returns: thread-safety concerns, shared state patterns, test coverage gaps

  3. Synthesizes findings:
     "Based on the context and review, here's the implementation plan:
      - Add RateLimiter middleware in src/middleware/rate_limit.rs
      - Use existing SharedState pattern from src/middleware/mod.rs
      - Add per-endpoint config in src/config/api.rs
      - Fix thread-safety issue in connection pool (flagged by reviewer)"

  4. Implements the changes with full context
```

## Agent Configuration Tips

**Cheap models for simple agents:**
```toml
[[roles]]
name = "context_gatherer"
model = "openrouter:google/gemini-2.5-flash-preview"  # Fast, cheap, large context
```

**Powerful models for complex analysis:**
```toml
[[roles]]
name = "code_reviewer"
model = "anthropic:claude-sonnet-4"  # Best reasoning
```

**Tool restrictions for safety:**
```toml
# Read-only agent (can't modify files)
[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:ast_grep"]

# Full-access agent (can edit and run commands)
[roles.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["core:*", "filesystem:*"]
```

## Key Points

- Each agent runs as an isolated subprocess via ACP protocol
- Agents have their own role, tools, and model -- fully independent
- `async: true` runs agents in parallel (results arrive via inbox)
- Dynamic agents can be created at runtime for ad-hoc tasks
- Max concurrent async jobs = CPU cores (minimum 4)
- The main session orchestrates; agents do focused work
- Use cheap models for simple agents, powerful models where reasoning matters
