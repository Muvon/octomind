# Use Case: Long-Running Development with Session Resume

Use named sessions and resume to work on complex tasks across multiple sittings without losing context.

## The Problem

Large refactoring, feature development, or investigation tasks don't fit in a single sitting. When you start a new session, the AI has no memory of yesterday's work -- you re-explain context, re-read files, and repeat decisions. Wasted time and tokens.

## Solution

Named sessions persist full conversation history to disk. Resume exactly where you left off.

### Day 1: Start the Task

```bash
octomind run --name auth-refactor
```

```
> Refactor the authentication module to support OAuth2.
> Start by analyzing the current auth system.

AI: [reads files, analyzes architecture, proposes plan]
  - Current auth: session-based in src/auth/session.rs
  - Token validation in src/auth/tokens.rs
  - Middleware chain in src/middleware/auth.rs
  - Proposed approach: add OAuth2 flow alongside existing session auth
  - 4-phase plan: design → implement → test → migrate

> Good plan. Let's start with the design phase.

AI: [designs interfaces, creates types, documents API]
```

End of day -- just close the terminal or `/exit`. Session is auto-saved.

### Day 2: Resume with Full Context

```bash
octomind run --resume auth-refactor
```

```
> Continue with the implementation phase from yesterday's design.

AI: "Resuming from the design phase. I see we agreed on:
  - OAuth2Flow struct in src/auth/oauth2.rs
  - TokenValidator trait extension
  - New middleware for OAuth2 token validation
  Let me start implementing..."
```

The AI has the complete conversation history -- all decisions, file reads, code changes, and reasoning from day 1.

### Day 3: Quick Resume

Don't remember the exact session name? Use `--resume-recent`:

```bash
octomind run --resume-recent
```

Finds the most recent session for the current project directory automatically.

Or list all sessions:

```bash
octomind run
```
```
/list
# Shows all saved sessions with dates and names
```

### Managing Context Over Long Sessions

As sessions grow, context management becomes important:

```
# Check token usage
/info

# If context is getting large, force compression with /done
# (or rely on automatic compression at threshold)
/done

# Or use the reduce command (if configured)
/run reduce

# View what's in context
/context
/context large    # Show only large messages
```

The automatic compression system also helps -- it kicks in at configured token thresholds and preserves critical knowledge (decisions, constraints, preferences) across compressions.

### Multi-Branch Development

Work on related tasks in parallel with separate sessions:

```bash
# Main feature work
octomind run --name auth-refactor

# Bug found during refactoring
octomind run --name auth-bugfix-csrf

# Tests for the new feature
octomind run --name auth-tests
```

Switch between them:
```
/list
/session auth-bugfix-csrf
```

Each session maintains its own independent context and history.

### Combining with Agents

For large tasks, delegate subtasks to agents while maintaining the main session:

```
> I need to understand the test coverage before continuing the refactor.
> Use the context_gatherer agent to analyze test coverage for src/auth/

AI calls: agent_context_gatherer(task="Analyze test coverage for src/auth/. List all test files, what they cover, and gaps.")

# Agent runs independently, returns results
# Main session continues with full context + new coverage data

> Good. Now implement the OAuth2 token validator based on yesterday's design
> and today's coverage analysis.
```

### Session Persistence Details

What's saved and restored:

| Preserved | Details |
|-----------|---------|
| Full message history | All user messages, AI responses, tool calls and results |
| Token counts | Input, output, cached, reasoning tokens |
| Cost tracking | Per-request and cumulative costs |
| Compression knowledge | Critical decisions and constraints survive compression |
| Model info | Which model was used |
| Media attachments | Images and videos attached during session |

What's NOT persisted:
- Active schedules (in-memory only)
- Running background jobs
- Dynamic MCP servers added at runtime (use `persist` to save them)
- Workflow execution state (but compressed summaries are preserved)

## Practical Tips

**Name sessions descriptively:**
```bash
octomind run --name "feature-oauth2-phase2"
octomind run --name "bugfix-login-timeout"
octomind run --name "investigate-memory-leak"
```

**Use `/done` at natural checkpoints:**
```
/done
```
This triggers task completion, summary, and cleanup -- creating a clean checkpoint before the next phase.

**Set spending limits for safety:**
```toml
max_session_spending_threshold = 10.0   # USD per session
```

Long sessions can accumulate significant costs. Monitor with `/info`.

**Enable compression for multi-day sessions:**
```toml
[compression]
hints_enabled = true
knowledge_retention = 10

[[compression.pressure_levels]]
threshold = 60000
target_ratio = 2.0
```

This keeps context manageable while preserving critical decisions.

## Key Points

- `--name` creates or resumes a named session
- `--resume NAME` explicitly resumes an existing session
- `--resume-recent` finds the most recent session for the current project
- Full conversation history is persisted in `~/.local/share/octomind/sessions/`
- The AI picks up exactly where you left off -- all context, decisions, and findings intact
- Use `/done` or automatic compression to manage growing context
- Combine with agents for parallel subtask delegation
- Session persistence works across CLI, daemon, and WebSocket modes
