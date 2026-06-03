# Use Case: Multi-Agent Task Delegation

Split complex tasks across specialized AI agents that work independently and report back to a coordinator.

## The Problem

A single AI call struggles with large tasks: "Refactor the authentication system." It tries to do everything at once, loses context, and produces incomplete results. You want specialized agents -- one gathers context, one reviews code, one plans architecture -- working in parallel.

## Solution

Configure multiple agents, each with its own role and tools. The main session delegates to them and synthesizes results.

Octomind offers three ways to delegate, in increasing flexibility:

1. **Static `[[agents]]`** — pre-defined local sub-agents exposed as `agent_<name>` tools. Best when you have stable, project-specific specialists. (Steps 1–3 below.)
2. **The `tap` tool** — run a community-maintained specialist role from a tap registry with no config edits. Best when someone already built the role you need. (See [Tap Roles](#tap-roles-no-config-needed).)
3. **The dynamic `agent` tool** — let the orchestrator create sub-agents on the fly during a session. Best for ad-hoc, one-off tasks. (See [Dynamic Agents](#dynamic-agents).)

### Step 1: Define Agent Roles

A role's `system`, `welcome`, `temperature`, `top_p`, and `top_k` are all **required** — there are no implicit defaults, so omitting any of them makes the config fail to load. Set `welcome = ""` for sub-agent roles you never start interactively.

```toml
# Roles for each agent (in config.toml)

[[roles]]
name = "context_gatherer"
temperature = 0.2
top_p = 0.7
top_k = 20
welcome = ""
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
top_p = 0.7
top_k = 20
welcome = ""
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

> **Note:** `filesystem` is **not** a built-in server. The default config declares only `core`, `runtime`, and `agent` as MCP servers — the `filesystem` tools (`view`, `text_editor`, `ast_grep`, `shell`, …) are supplied by the default tap (`muvon/tap`). With MCP disabled or that tap not installed, `server_refs = ["filesystem"]` resolves to nothing. See [Tap System](../integration/04-tap-system.md) and [MCP Tools](../usage/07-mcp-tools.md) for how filesystem tools become available.

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

Each `[[agents]]` entry is exposed to the main session as a tool named `agent_<name>`. The positional argument in `command` (`octomind acp context_gatherer`) **is the role name** — the agent inherits its model, system prompt, temperature, and tools from the matching `[[roles]]` entry. Keep the agent name and the role name identical so the wiring is obvious.

Config agents run as ACP subprocesses with their stderr suppressed, so a child-side crash or misconfiguration surfaces only as an error or empty result returned from the `agent_<name>` call — not as console output.

### Step 3: Use in Session

Start the orchestrating session with your default role and delegate:

```bash
octomind run            # uses the configured default tag (assistant:concierge)
# or name an orchestrator role explicitly:
octomind run assistant
```

> `developer` is not a built-in role — it only resolves through the default tap's `developer` category. Use `assistant` (the shipped default) or any orchestrator role you defined, as long as it has access to the `agent` tools.

The main AI can now use these agents as tools (illustrative transcript):

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

For large tasks, run agents in parallel (illustrative transcript):

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

Tap roles share their own system prompt + model + tool kit. `background: true` returns the run id immediately and, when the run finishes, the reply lands as a user message in the next turn labeled `[Tap-run '<id>' (<role>) completed]` (or `… failed]` on error) — distinct from the `[Async agent '...' completed]` label used by `agent_*` jobs. Resume with `{"action": "run", "session": "<id>", "prompt": "follow-up question"}`.

A few runtime constraints worth knowing:

- `discover` (and `capability`) require the local embedding model to be initialized — if it failed to load or is not ready yet, the action returns an error instead of results.
- `discover` returns at most the **top 5** matching roles, and only those with a cosine score above `0.2`. A vague intent may return nothing.
- Resuming a tap-run that is still executing returns a **busy** error. Wait for it to finish, or stop it first with `{"action": "stop", "session": "<id>"}`.

See [Tap System](../integration/04-tap-system.md) and [MCP Tools — `tap`](../usage/07-mcp-tools.md#tap----run-specialist-roles-from-taps).

Use `[[agents]]` (this page) when the role doesn't exist in any tap or you need a custom local-only agent. Use `tap` when a community-maintained role already covers the task.

### Dynamic Agents

Create agents on the fly during a session using the `agent` tool from the `runtime` server (the `core` server hosts only `plan` and `tap`):

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

If you omit `server_refs` but list `allowed_tools`, the servers are inferred automatically from the tool prefixes (e.g. `filesystem:view` implies `server_refs = ["filesystem"]`).

## Example: Full Development Pipeline

The following is an illustrative walkthrough of how the orchestrator chains the agents — not literal commands to type:

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

The snippets below show only the field being changed — a real `[[roles]]` entry must still include the required `system`, `welcome`, `temperature`, `top_p`, and `top_k` fields shown in [Step 1](#step-1-define-agent-roles).

**Cheap models for simple agents:**
```toml
[[roles]]
name = "context_gatherer"
model = "openrouter:google/gemini-2.5-flash-preview"  # Fast, cheap, large context
# ... plus the required system / welcome / temperature / top_p / top_k fields
```

**Powerful models for complex analysis:**
```toml
[[roles]]
name = "code_reviewer"
model = "anthropic:claude-sonnet-4"  # Best reasoning
# ... plus the required system / welcome / temperature / top_p / top_k fields
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
- Max concurrent async jobs = number of CPU cores (defaults to 4 only if the core count cannot be detected)
- The main session orchestrates; agents do focused work
- Use cheap models for simple agents, powerful models where reasoning matters
