# Context Compression

Use this guide to configure and inspect context compression in long-running sessions. It explains automatic folds,
manual task boundaries, retained context, and archive recovery.

## Get Started

Compression is enabled by default at `70000` tokens. Start a session and inspect its context with `/info`; use `/done`
when you finish a task and want to compress its history before the next task:

```bash
octomind run
```

```text
/info
/done
/info
```

## Overview

As sessions grow, token costs increase and context windows fill up. The compression system:

1. Monitors token usage against configurable thresholds
2. Decides whether compression would save money (cache-aware economics)
3. Drains older exchanges into an AI-generated summary while re-injecting the most recent intent
4. Retains critical knowledge across compressions

Two related safety nets sit on top of the adaptive engine:

- **The context ceiling** — the lower of `max_session_tokens_threshold` (root config, default `200000`) and the session
  model's usable window; reaching its safety margin requests a forced fold (see [The Hard Ceiling](#the-hard-ceiling)).
- **Cache keepalive** — an opt-in subsystem that keeps the prompt cache warm during idle time (see [Cache
  Keepalive](#configure-cache-keepalive)).

## Configuration

```toml
# Root config field — the user half of the hard compression ceiling (0 = model window only)
max_session_tokens_threshold = 200000

[compression]
knowledge_retention = 25
analysis_findings_max_tokens = 6000

# The single compression trigger, in absolute tokens (0 = compression disabled).
# Depth is NOT configured — it is computed per cycle from the measured session
# growth rate and the context ceiling.
threshold = 70000

[compression.model]
name = "openai:gpt-5.6-luna"
reasoning_effort = "medium"
max_tokens = 16000
temperature = 0.3
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

The example uses an explicit compression model override; the shipped name is `octohub:auto`.

| Field | Shipped default | Purpose |
|-------|-----------------|---------|
| `compression.threshold` | `70000` | Soft automatic trigger; `0` disables automatic folding |
| `compression.knowledge_retention` | `25` | Maximum rolling critical-knowledge entries |
| `compression.analysis_findings_max_tokens` | `6000` | Retained findings budget; `0` disables this channel |
| `compression.attention.enabled` | `false` | Enable attributed PACT summaries |
| `compression.attention.validator` | `true` | Validate summary attribution |
| `compression.attention.telemetry` | `true` | Write local compression diagnostics |
| `compression.attention.governance.enabled` | `true` | Preserve and verify runtime-owned governance context |
| `compression.attention.governance.verify_hash` | `true` | Verify governance before committing a fold |

To enable attributed PACT summaries, set these tables after scalar `[compression]` fields; replace existing tables
rather than defining the same table twice:

```toml
[compression.attention]
enabled = true
validator = true
telemetry = true

[compression.attention.governance]
enabled = true
verify_hash = true
```

See [Configuration Reference](../reference/03-config-reference.md#compression) for all fields.

## Compression Model

Octomind has exactly three persistent model purposes: main `[model]`, shared `[supervisor.model]`, and
`[compression.model]`. The compression profile performs both the compression decision and summary generation; it is not
a fourth model purpose. The shipped profile uses `octohub:auto`, and omitted override fields inherit from `[model]`.

## Configure Cache Keepalive

When you walk away after the AI replies, the prompt cache TTL counts down and the next turn may miss cache. Cache
keepalive sends minimal `max_tokens = 1` idle pings against a frozen snapshot of the conversation. Put these root
fields before any table headers:

```toml
cache_keepalive_enabled = true           # opt in; shipped default is false
cache_keepalive_max_idle_seconds = 1800  # stop pinging 30 min after last activity (0 = until session ends)
```

- Only models whose provider returns a keepalive policy are pinged; the runtime skips other models.
- The ping **interval comes from the provider**, not from config.
- Pings only fire when the snapshot actually has a cached message (otherwise there is nothing to keep warm).
- Each ping costs cache-read tokens; those costs are folded back into the session cost.

## Monitoring

Use `/info` to see compression statistics. Illustrative values after three folds:

```text
compression
  runs               3 · 3 conversation
  messages removed   128
  tokens saved       45,000
  avg ratio          81.8%
```

- `runs` — total count, broken down by compression kind (shown only when > 0).
- `messages removed` — cumulative messages drained across all compressions.
- `tokens saved` — cumulative tokens reclaimed.
- `avg ratio` — a saturating heuristic, `tokens_saved / (tokens_saved + 10000)` rendered as a percentage, not a literal
  compression ratio.

When available, the block also reports the compression model's input/output tokens, throughput, and cost. It does not
report monetary savings; token savings alone do not prove a fold saved money.

## Tune Compression

Start with the shipped threshold. If summaries interrupt useful context too often, increase it in your config:

```toml
[compression]
threshold = 100000
```

The model-window ceiling still limits available headroom. To disable automatic folds for a short-session workflow:

```toml
[compression]
threshold = 0
```

Manual `/done` remains available. The final context-ceiling check can still reject an oversized request.

## Troubleshooting

**Compression not triggering:**

- Check `compression.threshold` is non-zero and actually exceeded.
- A zero `max_session_tokens_threshold` removes the configured ceiling; the usable model window still applies.
- Use `/info` to see the current token count vs. your thresholds.

**Compression too aggressive:**

- Increase `compression.threshold`; compression depth is computed and has no `target_ratio` config key.

**Compression not reducing repeated context:**

- Revisit the `[compression.model]` profile if it produces poor summaries
- Increase thresholds to compress less frequently

**Context still exceeds the ceiling:** The runtime caps oversized stored tool results before paying for a fold and
checks the materialized context again before sending it. A protected current request can still be too large. Shorten
that request, reduce tool output, or select a model with more usable context; raising the soft threshold cannot fix it.

```text
/info
/loglevel debug
```

Debug logs explain fire-line eligibility, feasibility, and fold economics; `/info` shows the accumulated results.

## Compression Reference

### Token-Based Triggers

Compression becomes eligible when the full context (messages + system prompt + tool definitions + safety margin) exceeds
the **fire line**: `compression.threshold`, pulled down automatically when the model's window is small so at least 5
API calls of measured growth still fit below the ceiling.

**Computed depth.** How deep each compression goes is not configured. The controller picks the post-compression token
target directly from measured session dynamics:

```text
target_after = fire_line − runway × growth
```

- `growth` — measured full-context growth per API call since the last compression checkpoint, including tool results and
  runtime injections; before the first fold it uses lifetime full-context growth with output growth as a floor
- `runway` — the autonomous per-turn ladder, `5 × 2^consecutive_compressions`

The target is clamped between the deepest and gentlest achievable sizes (derived ratio always lands in **[2.0, 16.0]**)
and must fall at least 5 API calls of growth below the fire line — a compression that would re-fire immediately is refused
before a paid call. The effect: a hot session (high growth, long predicted runway) compresses deep and buys a long quiet
stretch; a winding-down session can receive a gentler fold.

(Forced `/done` compression skips the controller and uses the gentlest fixed 2.0x — it is a task boundary, so there are
no session dynamics to project onto the next task; see [Forced vs Automatic
Compression](#forced-vs-automatic-compression).)

### The Hard Ceiling

The context ceiling is the lower of `max_session_tokens_threshold` (root config, default `200000`) and the session
model's physical window minus the reserved completion budget (`max_tokens`). With automatic compression enabled, a fold
is **forced** one runway margin early — when the full-context token count plus 5 calls of measured growth reaches the
ceiling (the margin applies once at least 5 calls have been measured since the last fold; before that only the bare
ceiling counts) — so the next few rounds cannot overshoot the window. A forced fold bypasses the amortization gate, the
failure cooldown, and the compression model's veto, runs inline, and uses the deepest allowed ratio (**16.0x**). If the
fold call itself fails inside the margin, the error surfaces on the request instead of being retried round after round.

### Adaptive Fire Line and Runway

Within one autonomous turn, every successful compression increments `consecutive_compressions`. The soft fire line
follows `threshold × 2^k` (capped below the hard ceiling), while the desired quiet runway follows `5 × 2^k` API calls:
5, 10, 20, 40, and so on. A genuine new user turn resets `k`; forced `/done` also resets it after the fold. Paid
declines set a cooldown without raising the fire line.

Before the ceiling cap is applied, the line is raised to at least the last post-compression watermark plus five calls of
measured growth. The ceiling cap can pull it below the configured threshold. If the range is empty or even a `16.0x`
fold cannot land usefully below the line, the source returns without a paid compression call. A declined, failed,
cancelled, or discarded background fold separately sets `fold_cooldown_until_call` for one runway; the hard ceiling
bypasses that cooldown.

### Amortization Gate

Behind the fire line, a fold has to earn its place. Two regimes:

- **Genuine turn boundary** (between a user message and its first API call): after eligibility and feasibility checks,
  the economic gate accepts the fold without requiring amortization.
- **Mid-turn**: the fold must be amortized over the work the session's own pace predicts.

```text
expected_calls = (median_calls_per_turn − calls_this_turn)⁺ + median_calls_per_turn × turns_seen
                 (never below calls_this_turn; first turn: calls_this_turn)
fold iff expected_calls ≥ runway
     and (current − target_after) × cache_read × expected_calls
         ≥ sent × folder_input + summary × folder_output + target_after × cache_write
```

- `median_calls_per_turn` comes from the last 16 completed genuine turns (`turn_call_counts`); `turns_seen` is the
  number of recorded turns in that bounded history, used as a future-work estimate.
- `runway` is the autonomous ladder (5, 10, 20 … per consecutive in-turn fold), so each further fold in one turn needs a
  longer predicted horizon.
- The fire line itself is a geometric per-turn ladder: the k-th consecutive successful in-turn fold doubles it —
  `threshold × 2^k`, capped one safety margin under the ceiling — so a single long turn gets 70k → 140k → cap of room
  instead of re-folding at the same mark. A genuine user turn resets the level.
- The amortization calculation uses provider accounting when available and conservative internal fallback weights
  otherwise.
- `sent` is estimated as 45% of the compressible range; `summary` is estimated as that range divided by 16, capped by
  the compression model's output budget when non-zero.

These estimates guide mid-turn folds; they do not know how many calls the task will actually need.

### Background Folds

An automatic fold outside the ceiling margin does not block the agent. The prompt is built from the drained range, the
decision+summary call runs in a spawned task, and the agent keeps working; the summary is applied at a later round
boundary, and only to the exact range it was computed from (a content fingerprint of the drained messages — a changed
range discards the summary). One fold is in flight at a time.

- **Turn end**: a finished fold is applied before the session is saved — replace only, never auto-continue. A fold still
  running stays parked and is collected at the next round; turn end never waits on it.
- **Ceiling margin**: a pending fold is awaited, and its result applied without the veto; with no fold pending the
  trigger runs inline and forced (see [The Hard Ceiling](#the-hard-ceiling)).
- **Failure cooldown**: a fold that fails, is cancelled, or is discarded holds unforced attempts for one runway of calls
  (5, 10, 20… on the ladder) instead of retrying on the next round. A slow or broken compression model therefore gets
  one attempt per runway, never one per call.

### Forced vs Automatic Compression

The `/done` command triggers **forced compression**, which behaves differently from automatic compression:

| Behavior | Forced (`/done`) | Automatic |
|----------|------------------|-----------|
| Failure cooldown | Bypassed | Applied outside the ceiling margin |
| Amortization gate | Bypassed | Enforced outside the ceiling margin |
| Feasibility check ("won't drop below threshold") | Bypassed | Enforced outside the ceiling margin |
| AI veto | Forced — AI cannot decline | AI may decline outside the ceiling margin |
| Min. conversation messages | 3 | 5 normally; 3 when forced by the ceiling |
| Compression ratio | Fixed gentlest ratio, `2.0x` | Computed from session growth and runway, clamped to `2.0x`–`16.0x`; hard-ceiling folds use `16.0x` |
| Consecutive-fold counter after success | Reset to 0 | Incremented |
| Purpose | Session boundary — clean slate | Mid-session cost optimization |

Note that `/done` is **less** aggressive on ratio than a high-pressure automatic compression: it uses the fixed gentlest
ratio (2.0x) with no adaptive adjustment. Its "clean slate" character comes from bypassing the gates and resetting
`consecutive_compressions` to 0 while recording the actual post-compression context size, so the next task starts
without accumulated compression debt — not from a higher ratio.

### Skill Preservation

Skills injected into context are handled differently depending on the compression trigger:

| Trigger | Skill Preservation Behavior |
|---------|----------------------------|
| Automatic (threshold-based) | All active skills preserved — their content stays in context |
| `/done` (forced) | No injected skills are preserved, including env-loaded skills |
| `skill(forget)` | No immediate compression — the skill is removed from the active list, and its stale content is naturally excluded at the next automatic compression |

**Why `/done` is different:** It marks a task boundary. The next task starts from a clean compressed state and activates
or injects only the skills it actually needs.

**Why `skill(forget)` doesn't force compression:** Immediate compression is unnecessary. The forgotten skill's content
naturally disappears at the next automatic compression since it's no longer in the active list.

### Context Preservation

Range selection is structural. The kept anchor is the last immutable-preamble message immediately before the first
task-stating message; system, welcome, and `<instructions>` scaffolding stay outside the drain, while old user tasks,
earlier summaries, and earlier continuation wrappers can fold into the new summary.

Automatic compression preserves the live exchange byte-for-byte. When a fresh user request is newest, it keeps the
preceding assistant response plus that request and any intervening control messages. Mid-task, it keeps the latest
assistant step and its following tool traffic. `/done` is the deliberate exception and may fold the whole task boundary.

The drained range is replaced by preserved active-skill messages, the generated summary, and—when needed—a continuation
envelope carrying the exact previous assistant response and user request. The summary retains older user tasks and the
active plan. Normal automatic compression requires at least five conversational messages in the candidate range; forced
folds (including `/done` and ceiling folds) require three.

### Lossless Archive and Recall

Compression is not one-way. Drained messages are written to per-session JSONL archives, and (when the PACT
attention/governance machinery is on — governance is on by default) each drain also writes a sidecar index of
content-addressed **block IDs** (`b:<hex>`). The compressed summary's `<folded_state>` units cite those IDs, and an
`<archive>` pointer in the summary names the file.

The **`recall` tool** closes the loop: the model passes up to 2 cited block IDs per call and gets their original text
with role labels (surrounding whitespace is trimmed). Recalled content arrives as a normal tool result—appended at the
tail, never rewriting history—and is subject to the global `mcp_response_tokens_threshold`. If the session has no block
registry yet, or an ID is unknown, the tool returns an error instead of guessing or scanning an uncited archive.

For example, the model calls `recall` with a real block ID copied from the current summary:

```json
{"ids": ["b:1a2b3c4d"]}
```

The sample ID shows the format and must be replaced. Archives live under `sessions/archive/<session-name>/`, with
`<compression-id>.jsonl` messages and `.blocks.jsonl` sidecars. With attention/governance active, optional folds abort
on archive verification failure; forced folds can proceed after a storage failure with recall unavailable for that
cycle. Governance verification failure is never bypassed.

### Knowledge Retention

Each compression may extract critical knowledge (decisions, constraints, preferences). New entries are appended and the
list is FIFO-trimmed to the most recent N (configurable via `knowledge_retention`, default: 25) — the oldest are dropped
when the limit is exceeded. Separately, `analysis_findings_max_tokens` (default `6000`) bounds retained findings by
relevance, recency, and diversity; `0` disables that findings channel.

**Intermediate learning.** When `supervisor.learning.enabled = true` and the conversation has at least 3 genuine user
task messages, a successful automatic compaction can start a detached lesson-extraction pass from a snapshot taken
before the fold. This is asynchronous and never blocks compression. See [Learning](13-learning.md).

## Source Reference

- [Defaults](../../config-templates/default.toml)
- [Trigger, background folds, and learning snapshots](../../src/session/chat/conversation_compression/mod.rs)
- [Controller math](../../src/session/chat/conversation_compression/decision.rs)
- [Archive and context replacement](../../src/session/chat/conversation_compression/apply.rs)
- [Keepalive policy](../../src/session/cache_keepalive.rs)

## See also

- [Configuration reference](../reference/03-config-reference.md)
- [MCP tools and recall](07-mcp-tools.md)
- [Learning](13-learning.md)
- [Token efficiency](16-token-efficiency.md)
