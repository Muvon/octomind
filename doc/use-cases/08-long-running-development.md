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

This matches saved sessions whose name contains the current working directory's
basename and resumes the most recently modified one. Run it from the same project
directory you started in -- a session begun in a different directory (different
basename) will not be matched, even if it is newer.

Or list all sessions:

```bash
octomind run
```
```
/list
# Lists saved sessions with their metadata (date, name, message/token counts).
# Paginated 15 per page -- use "/list 2" for the next page.
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
/context large    # Show only messages larger than 1000 characters
```

`/done` and automatic compression are the **same engine** with different triggers --
`/done` forces it now, automatic compression waits for a threshold. `/run reduce` is a
separate, configurable ACP layer command and is independent of that engine.

**How automatic compression decides to fire.** Each `[[compression.pressure_levels]]`
entry has an *absolute* `threshold` (a token count) and a `target_ratio`. When the
full-context token count exceeds any configured threshold, compression triggers using
the highest matched ratio. Critical knowledge (decisions, constraints, preferences) is
extracted and re-injected so it survives every compression.

Above all sits the root-level `max_session_tokens_threshold` (default `200000`). This
is a hard ceiling: once the context reaches it, compression is forced unconditionally --
bypassing the cooldown and cost guards that govern ordinary pressure-level compressions.
It is the single most important knob to tune for genuinely long sessions; set it `0` to
disable the session-token limit entirely. See
[Context Compression](../usage/08-compression.md) for the full mechanics.

**Why a second `/done` may report nothing to compress.** `/done` *forces* compression,
which bypasses the automatic cooldown and resets its counters — so the cooldown is not
the cause. The forced path still needs something to compress: it always keeps at least
the 3 most recent conversation messages (vs 5 for automatic compaction), so once the
first `/done` has folded everything down to the session anchor, a near-unchanged context
has no compressible range left and reports "nothing to compress." This is expected, not
a bug. (The `10% × 2^n` exponential cooldown governs only *automatic* compaction — it
raises the bar for the next automatic pass and resets on each new user message.)

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

### Carrying Knowledge Across Separate Sessions

Compression keeps a *single* session compact. The **learning system** is what carries
knowledge between *separate* named sessions. When learning is enabled (`[supervisor.learning]
enabled = true`, on by default), `/done` and session exit fire a background lesson
extraction: generalizable, project- and role-scoped lessons are saved and later injected
into future sessions for the same project. So `auth-refactor` on Day 5 can benefit from a
lesson learned during `auth-bugfix-csrf` on Day 2, even though they are different
sessions. This is distinct from per-session compression, which only summarizes the
current conversation. See [Adaptive Learning](../usage/13-learning.md) for details.

### Session Persistence Details

Sessions are stored as append-only `.jsonl.zst` files (zstd-compressed JSON lines) in
`~/.local/share/octomind/sessions/`. Resuming replays the log -- including `SUMMARY`,
`KNOWLEDGE_ENTRY`, and `COMMAND` entries -- to rebuild the exact context.

What's saved and restored:

| Preserved | Details |
|-----------|---------|
| Full message history | All user messages, AI responses, tool calls and results |
| Token counts | Input, output, cached, reasoning tokens |
| Cost tracking | Per-request and cumulative costs |
| Compression knowledge | Critical decisions and constraints survive compression |
| Model info | Which model was used |
| Media attachments | Images and videos attached during session |

Critical knowledge survives **both** compression and resume: it is replayed from the
`KNOWLEDGE_ENTRY` log entries when the session is reloaded, so decisions and constraints
are intact across sittings.

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

# Or steer the summary toward what matters next
/done focus on the API layer and the migration plan
```
`/done` force-compresses the current context (bypassing automatic thresholds and the
cooldown) and, when learning is enabled, extracts lessons in the background -- producing
a compact checkpoint before the next phase. A bare `/done` compresses with default
behavior; `/done <instructions>` passes guidance for the compression summary so you can
emphasize what the next phase will need.

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

**Keep the prompt cache warm across idle gaps (Anthropic only):**
```toml
cache_keepalive_enabled = true          # default: false
cache_keepalive_max_idle_seconds = 1800 # stop pinging after 30 min idle (cap: 86400)
```
When you step away mid-session, the prompt cache normally expires before you return, so
the next turn pays full price. With keepalive enabled, Octomind sends minimal idle pings
to keep the cache warm, then stops after `cache_keepalive_max_idle_seconds` so an
abandoned session does not bill forever. The ping interval is provider-driven and this
applies only to providers whose API supports refresh-on-read -- today that is Anthropic.
Ping costs are folded into the session cost. See
[Providers & Caching](../usage/04-providers.md) for the provider-facing details.

## Key Points

- `--name` creates or resumes a named session
- `--resume NAME` explicitly resumes an existing session
- `--resume-recent` finds the most recent session for the current project
- Full conversation history is persisted in `~/.local/share/octomind/sessions/`
- The AI picks up exactly where you left off -- all context, decisions, and findings intact
- Use `/done` or automatic compression to manage growing context (same engine, different triggers)
- Combine with agents for parallel subtask delegation
- Session persistence works across CLI, daemon, and WebSocket/ACP modes -- a session started in one mode is resumable by the same name in another

## See Also

- [Sessions](../usage/05-sessions.md) -- full session lifecycle, naming, and resume
- [Context Compression](../usage/08-compression.md) -- pressure levels, ratios, and the decision model
- [Adaptive Learning](../usage/13-learning.md) -- how knowledge is carried across separate sessions
- [Providers & Caching](../usage/04-providers.md) -- prompt caching and cache keepalive
