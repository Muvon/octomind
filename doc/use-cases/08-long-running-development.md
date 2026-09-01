# Long-Running Development

Use named sessions and resume to work on complex tasks across multiple sittings without losing context.

## The Problem

Large refactoring, feature development, or investigation tasks don't fit in a single sitting. When you start a new session, the AI has no memory of yesterday's work — you re-explain context, re-read files, and repeat decisions. Wasted time and tokens.

## Solution

Named sessions persist their reconstructable conversation state to disk so a later process can resume the same task boundary.

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

End of day — just close the terminal or `/exit`. Session is auto-saved.

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

The active conversation state is reconstructed from the session log, including any compression checkpoints and retained knowledge.

### Day 3: Quick Resume

Don't remember the exact session name? Use `--resume-recent`:

```bash
octomind run --resume-recent
```

This matches saved sessions whose name contains the current working directory's
basename and resumes the most recently modified one. Run it from the same project
directory you started in — a session begun in a different directory (different
basename) will not be matched, even if it is newer.

Or list all sessions:

```bash
octomind run
```
```
/list
# Lists saved sessions with their metadata (date, name, message/token counts).
# Paginated 15 per page — use "/list 2" for the next page.
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

**How automatic compression decides to fire.** `compression.threshold` is a single
*absolute* token count. When the full-context token count exceeds it, compression
becomes eligible; how deep each compression goes is computed per cycle from the
measured session growth rate and the context ceiling — hot sessions compress deeply,
winding-down sessions gently. Critical knowledge (decisions, constraints, preferences)
is extracted and re-injected so it survives every compression.

Above all sits the context ceiling: the lower of the root-level
`max_session_tokens_threshold` (default `200000`) and the session model's usable
window. Once the context reaches it, compression is forced unconditionally --
bypassing the cooldown and cost guards that govern ordinary compressions. Set
`max_session_tokens_threshold = 0` to rely on the model window alone. See
[Context Compression](../usage/08-compression.md) for the full mechanics.

Model terminology is limited to three purposes: **main**, **supervisor**, and
**compression**. The shipped default uses OctoHub via `octomind login`, with
`octohub:auto` for each purpose; `[compression.model]` is the optional override
for compression calls.

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
```

Start a fresh session for a new task:
```
/new auth-bugfix-csrf
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
`~/.local/share/octomind/sessions/`. Resuming replays the log — including `SUMMARY`,
`COMPRESSION_POINT`, `RESTORATION_POINT`, `KNOWLEDGE_ENTRY`, `COMMAND`, plan snapshots,
and schedule snapshots — to rebuild the current session state.

What's saved and restored:

| Preserved | Details |
|-----------|---------|
| Full message history | All user messages, AI responses, tool calls and results |
| Token counts | Input, output, cached, reasoning tokens |
| Cost tracking | Per-request and cumulative costs |
| Compression knowledge | Critical decisions and constraints survive compression |
| Schedules | The latest `SCHEDULE_SNAPSHOT` is restored for the resumed session |
| Model info | Which model was used |
| Media attachments | Images and videos attached during session |

Critical knowledge survives **both** compression and resume: it is replayed from the
`KNOWLEDGE_ENTRY` log entries when the session is reloaded, so decisions and constraints
are intact across sittings.

What's NOT persisted:
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

# Or compress, then immediately process a follow-up request
/done focus on the API layer and the migration plan
```
`/done` force-compresses the current context (bypassing automatic thresholds and the
cooldown) and, when learning is enabled, extracts lessons in the background — producing
a compact checkpoint before the next phase. A bare `/done` stops there;
`/done <text>` runs the same compression first and then treats `<text>` as the
next user message. It does not steer the compression summary.

**Enable compression for multi-day sessions:**
```toml
[compression]
knowledge_retention = 10
threshold = 60000
```

This keeps context manageable while preserving critical decisions.

## Key Points

- `--name` creates or resumes a named session
- `--resume NAME` explicitly resumes an existing session
- `--resume-recent` finds the most recent session for the current project
- Full conversation history is persisted in `~/.local/share/octomind/sessions/`
- The AI picks up exactly where you left off — all context, decisions, and findings intact
- Use `/done` or automatic compression to manage growing context (same engine, different triggers)
- Combine with agents for parallel subtask delegation
- Session persistence works across CLI, daemon, and WebSocket/ACP modes — a session started in one mode is resumable by the same name in another

## See Also

- [Sessions](../usage/05-sessions.md) — full session lifecycle, naming, and resume
- [Context Compression](../usage/08-compression.md) — thresholds, adaptive depth, and the compression model
- [Adaptive Learning](../usage/13-learning.md) — how knowledge is carried across separate sessions
- [Providers](../usage/04-providers.md) — OctoHub, provider credentials, and model selection
