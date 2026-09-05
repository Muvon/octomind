# Long-Running Development

Use named sessions and resume to continue development across multiple sittings. This guide covers saving work,
controlling context growth, and understanding what a resumed session can reconstruct.

## Start and Resume a Task

Named sessions persist reconstructable conversation state to disk. Resume the same session to continue its task; start a
different session when you need an independent conversation.

### Day 1: Start the Task

```bash
octomind run --name auth-refactor
```

In your project, give the task and its constraints explicitly:

```text
Refactor the authentication module to support OAuth2. First inspect the current implementation,
identify the affected files, and propose a design. Wait for my approval before editing.
```

End the sitting with the session command:

```text
/exit
```

Messages and state updates are logged as you work; a normal exit also saves session metadata. Do not rely on closing the
terminal abruptly to finish background work or lesson extraction.

### Day 2: Resume the Saved Context

```bash
octomind run --resume auth-refactor
```

```text
Summarize the retained design decisions, inspect the current files for changes since our last sitting,
and continue with the implementation phase.
```

The active conversation is reconstructed from the log, including compression checkpoints and retained knowledge.
Compressed history returns as summaries, not as every original message in the model's context. Resuming without an
explicit tag restores the saved role; supplying a tag deliberately selects that role instead.

### Day 3: Quick Resume

Don't remember the exact session name? Use `--resume-recent`:

```bash
octomind run --resume-recent
```

This selects the most recently modified session whose name contains the current directory's basename as a
**dash-delimited segment**, such as `-myproject-`. A custom name like `auth-refactor` may not match at all; use
`--resume auth-refactor` for it. Directories with the same basename can match the same saved sessions.

For an interactive picker of saved sessions, use bare `--resume` (a terminal is required):

```bash
octomind run --resume
```

Or list all sessions:

```bash
octomind run
```

```text
/list
# Lists saved sessions with metadata including name, date, model, tokens, and cost.
# Paginated 15 per page — use "/list 2" for the next page.
```

## Configure Context Management

Automatic compression is enabled in the default template. These are the shipped limits, copied from the template; put
root-level fields before any table header in your config file:

```toml
max_session_tokens_threshold = 200000

[compression]
knowledge_retention = 25
analysis_findings_max_tokens = 6000
threshold = 70000
```

| Field | Default | Meaning |
|---|---|---|
| `max_session_tokens_threshold` | `200000` | Context cap, further bounded by the model window minus output reservation; `0` uses the model bound alone |
| `compression.threshold` | `70000` | Base automatic trigger in absolute tokens; `0` disables automatic eligibility |
| `compression.knowledge_retention` | `25` | Retained critical-knowledge entries; `0` disables trimming of these entries |
| `compression.analysis_findings_max_tokens` | `6000` | Token budget for retained analysis findings; `0` disables their retention |

The base trigger grows geometrically during a long autonomous turn and resets on a genuine new user turn. Compression
depth and timing also account for measured growth and expected savings. Ordinary compression can run in the background
while the agent works; the summary is applied at a later round boundary. Near the context ceiling, compression is forced
and awaited. If the remaining context still exceeds the usable ceiling, the request fails rather than sending an
oversized prompt.

Automatic compression preserves the live exchange and active skill guidance. `/done` uses the same compression machinery
as an explicit task boundary: it can fold the whole task and does not preserve injected skills. Neither path guarantees
every detail will survive summarization. The compression model is configured separately at `[compression.model]`; see
[Context Compression](../usage/08-compression.md) for the full mechanics.

## Operate a Long Session

Inspect usage and the messages currently in context:

```text
/info
/context
/context large
```

`/context large` filters messages whose text exceeds 1000 bytes. It inspects the active context, not the entire
historical log.

### Finish a Task or Start the Next Phase

```text
/done
```

`/done` forces compression without waiting for automatic thresholds or cost guards. In CLI sessions, append the next
request to compress first and then process that text as a new user message:

```text
/done focus on the API layer and the migration plan
```

The trailing text does not steer the compression summary. Bare `/done` returns after compression; it does not exit the
session. The ACP prompt path also supports trailing instructions; WebSocket command messages and ACP command extension
calls use the generic handler and should send the next prompt separately.

The configured `reduce` command is a separate ACP layer transformation, independent of automatic compression:

```text
/run reduce
```

Use it only if the `reduce` entry is present in your `[[commands]]` configuration, as it is in the default template.

### Keep Related Tasks in Separate Sessions

Work on related tasks in parallel with separate sessions:

```bash
# Main feature work
octomind run --name auth-refactor

# Bug found during refactoring
octomind run --name auth-bugfix-csrf

# Tests for the new feature
octomind run --name auth-tests
```

`/list` inspects saved sessions; it does not switch conversations. Exit the current session and resume another:

```text
/exit
```

```bash
octomind run --resume auth-bugfix-csrf
```

To start fresh inside an interactive session, `/new` accepts an optional **display title** and generates a new session
ID. The title is not a resumable session name:

```text
/new Investigate the CSRF bug
```

Each session has independent conversation state. It does not create a Git branch or isolate working-tree files.

### Combining with Agents

For large tasks, delegate focused research while keeping the main session as the task record. With the default
template's `context_gatherer` agent configured, ask:

```text
Use context_gatherer to inspect the authentication tests. Report what their assertions cover and identify
missing cases before we change the implementation.
```

See [Multi-Agent Delegation](05-multi-agent-delegation.md) for agent setup, tool arguments, and execution behavior.

### Carry Knowledge Across Separate Sessions

Compression summarizes one session. Learning stores selected grounded memories for later retrieval across sessions,
including project/role-scoped records and global user rules. The default is:

```toml
[supervisor.learning]
enabled = true
```

`/done` captures the pre-compression transcript and starts extraction in the background. Eligible automatic compressions
can also extract memories. Normal interactive CLI exits can launch a separate background distillation process; this does
not guarantee that an abrupt terminal close will finish extraction.

Recall selects a bounded memory pack for the current user turn. It does not copy another session's full history into the
new one or guarantee that every lesson is recalled. Inspect stored records after extraction:

```text
/learning
/learning show 1
```

Use an index returned by `/learning`. See [Adaptive Learning](../usage/13-learning.md) for memory formation, recall, and
retention.

## Troubleshoot Resume and Compression

**Why does `/done` say there is nothing to compress?** Its candidate range needs at least three user/assistant messages
(ordinary compression needs five). These are minimum range sizes, not messages retained at the tail. A short
conversation or a second `/done` after the task has already been folded can have no eligible range.

**Why was my session not found?** `--resume NAME` requires an existing readable session. Use the exact ID from `/list`
or the picker, the same `OCTOMIND_DATA_DIR`, and explicit `--resume` for custom names. For isolated data:

```bash
OCTOMIND_DATA_DIR="$PWD/.octomind-data" octomind run --name auth-refactor
OCTOMIND_DATA_DIR="$PWD/.octomind-data" octomind run --resume auth-refactor
```

**Why are yesterday's tools or jobs missing?** Resume reconstructs conversation state, not live subprocesses.
Re-establish needed connections and re-check any work that was running when you stopped. Persist dynamic MCP
configuration before exit if you want it selected next time; see [Dynamic MCP Servers](06-dynamic-mcp-servers.md).

**Why does the AI need to inspect a file again?** Saved conversation state and compression summaries can describe an
older checkout. Ask it to inspect the current files before continuing, especially after changes outside the session.

## Persistence Reference

Sessions are append-only `.jsonl.zst` files (zstd-compressed JSON lines) under Octomind's `sessions` directory:

| Platform or override | Directory |
|---|---|
| macOS/Linux default | `~/.local/share/octomind/sessions/` |
| Windows default | `%LOCALAPPDATA%/octomind/sessions/` |
| `OCTOMIND_DATA_DIR` set | `$OCTOMIND_DATA_DIR/sessions/` |

Resuming replays message records and markers such as `SUMMARY`, `COMPRESSION_POINT`, `RESTORATION_POINT`,
`KNOWLEDGE_ENTRY`, and `COMMAND`. Plan and schedule snapshots restore their respective state.

| Saved state | Resume behavior |
|---|---|
| Messages, tool calls, and results | Reconstructs the active view after compression/restoration markers |
| Token and cost accounting | Restores saved cumulative metadata and newer usage records |
| Compression knowledge | Replays retained knowledge entries; summaries replace compressed ranges |
| Schedules | Restores the latest readable `SCHEDULE_SNAPSHOT` |
| Model and role | Restores saved state, subject to explicit startup overrides |
| Image/video attachments | Serialized with their message records; availability in active context follows message retention |

Running jobs and dynamic server registrations are runtime state. Saving a conversation does not restart a background
process or resume an interrupted workflow execution.

Implementation: [session replay](../../src/session/persistence.rs),
[compression](../../src/session/chat/conversation_compression/mod.rs), [compression
ranges](../../src/session/chat/conversation_compression/range.rs), and [default
configuration](../../config-templates/default.toml).

## See also

- [Sessions](../usage/05-sessions.md)
- [Context Compression](../usage/08-compression.md)
- [Adaptive Learning](../usage/13-learning.md)
- [Dynamic MCP Servers](06-dynamic-mcp-servers.md)
- [Multi-Agent Delegation](05-multi-agent-delegation.md)
