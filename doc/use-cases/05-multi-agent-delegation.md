# Multi-Agent Task Delegation

Split complex tasks across specialized AI agents that work independently and report back to a coordinator.

## The Problem

A single AI call struggles with large tasks: "Refactor the authentication system." It tries to do everything at once, loses context, and produces incomplete results. You want specialized agents — one gathers context, one reviews code, one plans architecture — working in parallel.

## Solution

Configure multiple agents, each with its own role and tools. The main session delegates to them and synthesizes results.

Octomind offers three ways to delegate, in increasing flexibility:

1. **Static `[[agents]]`** — pre-defined local sub-agents exposed as `agent_<name>` tools. Best when you have stable, project-specific specialists. (Steps 1–3 below.)
2. **The `tap` tool** — run a community-maintained specialist role from a tap registry with no config edits. Best when someone already built the role you need. (See [Tap Roles](#tap-roles-no-config-needed).)
3. **The dynamic `agent` tool** — let the orchestrator create sub-agents on the fly during a session. Best for ad-hoc, one-off tasks. (See [Dynamic Agents](#dynamic-agents).)

### Step 1: Define Agent Roles

A role's `system` and `welcome` remain explicit role behavior. Its `[roles.model]` block is optional: omitted model fields inherit from the required main `[model]` profile. Set `welcome = ""` for sub-agent roles you never start interactively.

```toml
# Roles for each agent (in config.toml)

[[roles]]
name = "context_gatherer"
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

[roles.model]
temperature = 0.2
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:workdir"]

[[roles]]
name = "code_reviewer"
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

[roles.model]
temperature = 0.1
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"]
```

> **Note:** `filesystem` is **not** a built-in server. The default config declares `core`, `orchestration`, `runtime`, and `agent`; the `filesystem` tools (`view`, `text_editor`, `shell`, …) come from resolved tap configuration. If no resolved server has that name, `server_refs = ["filesystem"]` contributes no tools. See [Tap System](../integration/04-tap-system.md) and [MCP Tools](../usage/07-mcp-tools.md).

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
octomind run assistant:concierge
```

> `assistant:concierge` is the shipped default orchestrator. Use it or an orchestrator role explicitly defined under local `[[roles]]`, as long as it has access to the `agent` tools.

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

Use `/status` for a concise view of every active agent alongside MCP jobs and
command monitors. `/status agents` expands the agent view with recent tap-run
history, live actions, model usage, and cost where the runtime provides it.

### Tap Roles (no config needed)

If a tap registry already provides a specialist role for the sub-task, use the `tap` tool from the `orchestration` builtin server instead of defining your own `[[agents]]`:

```jsonc
// Discover, then delegate — no config edits, no subprocess setup.
{"action": "discover", "intent": "review code for OWASP Top 10 issues"}
{"action": "run", "role": "security:owasp", "prompt": "Audit src/auth/ for OWASP issues"}
```

Tap roles share their own system prompt + model + tool kit. `run` returns the run id immediately and, when it finishes, the reply lands as a user message in the next turn labeled `[Tap-run '<id>' (<role>) completed]` (or `… failed]` on error) — distinct from the `[Async agent '...' completed]` label used by `agent_*` jobs. Resume with `{"action": "run", "session": "<id>", "prompt": "follow-up question"}`.

A few runtime constraints worth knowing:

- `discover` (and `capability`) require the local embedding model to be initialized — if it failed to load or is not ready yet, the action returns an error instead of results.
- `discover` returns at most the **top 5** matching roles, and only those with a cosine score above `0.2`. A vague intent may return nothing.
- Resuming a tap-run that is still executing returns a **busy** error. Wait for it to finish, or stop it first with `{"action": "stop", "session": "<id>"}`.

See [Tap System](../integration/04-tap-system.md) and [MCP Tools — `tap`](../usage/07-mcp-tools.md#tap----run-specialist-roles-from-taps).

Use `[[agents]]` (this page) when the role doesn't exist in any tap or you need a custom local-only agent. Use `tap` when a community-maintained role already covers the task.

### Dynamic Agents

Create agents on the fly during a session using the `agent` tool from the `runtime` server (`tap` delegation lives on the separate `orchestration` server):

```jsonc
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
User: "Add request tracing to the API endpoints"

Main AI:
  1. Calls agent_context_gatherer:
     "Find all API endpoint handlers, middleware patterns, and existing tracing code"
     -> Returns: file list, handler signatures, middleware chain pattern

  2. Calls agent_code_reviewer:
     "Review the current API middleware for potential issues with adding request tracing"
     -> Returns: thread-safety concerns, shared state patterns, test coverage gaps

  3. Synthesizes findings:
     "Based on the context and review, here's the implementation plan:
      - Add RequestTrace middleware in src/middleware/request_trace.rs
      - Use existing SharedState pattern from src/middleware/mod.rs
      - Add per-endpoint config in src/config/api.rs
      - Fix thread-safety issue in connection pool (flagged by reviewer)"

  4. Implements the changes with full context
```

## Agent Configuration Tips

The complete role examples in [Step 1](#step-1-define-agent-roles) show the required identity and prompt fields. A role's model block is optional and inherits every omitted field from `[model]`.

When a specialist genuinely needs a different main-purpose model, set a
concrete override rather than restating the default. For example:

```toml
# Inside the context_gatherer role
[roles.model]
name = "openai:gpt-5.6-luna"
temperature = 0.2
```

For a separate review specialist:

```toml
# Inside the code_reviewer role
[roles.model]
name = "anthropic:claude-sonnet-4-6"
temperature = 0.1
```

Octomind has exactly three model purposes: main, supervisor, and compression.
Agent role overrides belong to the main purpose; they do not introduce another
purpose. The shipped default for all three is `octohub:auto` after
`octomind login`. Omit `name` from `[roles.model]` to inherit the main profile's
model name, as the runnable roles in Step 1 do. To make an agent read-only, grant
only read tools in its `[roles.mcp].allowed_tools`; to permit edits or commands,
add those exact tool names or a deliberate server wildcard.

## Key Points

- Config-defined `[[agents]]` run as ACP subprocesses; runtime-created dynamic agents run in-process
- Each agent has its own resolved role, tool filter, and main-purpose model profile
- `async: true` runs agents in parallel (results arrive via inbox)
- Dynamic agents can be created at runtime for ad-hoc tasks
- Max concurrent async jobs = number of CPU cores (defaults to 4 only if the core count cannot be detected)
- The main session orchestrates; agents do focused work
- The default model is `octohub:auto`; role model overrides affect the main purpose only
