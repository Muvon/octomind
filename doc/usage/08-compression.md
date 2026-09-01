# Context Compression

Context compression keeps long-running sessions within model limits while preserving active work, critical knowledge, and lossless archives.

## Overview

As sessions grow, token costs increase and context windows fill up. The compression system:
1. Monitors token usage against configurable thresholds
2. Decides whether compression would save money (cache-aware economics)
3. Drains older exchanges into an AI-generated summary while re-injecting the most recent intent
4. Retains critical knowledge across compressions

Two related safety nets sit on top of the adaptive engine:
- **The context ceiling** — the lower of `max_session_tokens_threshold` (root config, default `200000`) and the session model's usable window; crossing it force-compresses unconditionally (see [The Hard Ceiling](#the-hard-ceiling)).
- **Cache keepalive** — an opt-in subsystem that keeps the prompt cache warm during idle time (see [Cache Keepalive](#cache-keepalive)).

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

See [Configuration Reference](../reference/03-config-reference.md#compression) for all fields.

## How It Works

### Token-Based Triggers

Compression becomes eligible when the full context (messages + system prompt + tool definitions + safety margin) exceeds the **fire line**: `compression.threshold`, pulled down automatically when the model's window is small so at least 5 turns of measured growth still fit below the ceiling.

**Computed depth.** How deep each compression goes is not configured. The controller picks the post-compression token target directly from measured session dynamics:

```
target_after = fire_line − runway × growth
```

- `growth` — measured full-context growth per API call since the last compression checkpoint, including tool results and runtime injections; before the first fold it uses lifetime full-context growth with output growth as a floor
- `runway` — the autonomous per-turn ladder, `5 × 2^consecutive_compressions`

The target is clamped between the deepest and gentlest achievable sizes (derived ratio always lands in **[2.0, 16.0]**) and must fall at least 5 turns of growth below the fire line — a compression that would re-fire immediately is refused (cooldown instead). The effect: a hot session (high growth, long predicted runway) compresses deep and buys a long quiet stretch; a winding-down session compresses gently and preserves fidelity. Compressing to a stable watermark also keeps the post-compression prefix size consistent, which is what keeps the prompt cache effective across cycles.

(Forced `/done` compression skips the controller and uses the gentlest fixed 2.0x — it is a task boundary, so there are no session dynamics to project onto the next task; see [Forced vs Automatic Compression](#forced-vs-automatic-compression).)

### The Hard Ceiling

The context ceiling is the lower of `max_session_tokens_threshold` (root config, default `200000`) and the session model's physical window minus the reserved completion budget (`max_tokens`). Compression is **forced unconditionally** one runway margin early — when the full-context token count plus 5 calls of measured growth reaches the ceiling (the margin applies once at least 5 calls have been measured since the last fold; before that only the bare ceiling counts) — so the next few rounds cannot overshoot the window. A forced fold bypasses the amortization gate, the failure cooldown, and the compression model's veto, runs inline, and uses the deepest allowed ratio (**16.0x**). If the fold call itself fails inside the margin, the error surfaces on the request instead of being retried round after round.

### Adaptive Fire Line and Runway

Within one autonomous turn, every successful compression increments `consecutive_compressions`. The soft fire line follows `threshold × 2^k` (capped below the hard ceiling), while the desired quiet runway follows `5 × 2^k` API calls: 5, 10, 20, 40, and so on. A genuine new user turn resets `k`; forced `/done` also resets it after the fold.

The line never falls below the configured threshold or below the last post-compression watermark plus five calls of measured growth. If the range is empty or even a `16.0x` fold cannot land usefully below the line, the source returns without a paid compression call. A failed, cancelled, or discarded background fold separately sets `fold_cooldown_until_call` for one runway; the hard ceiling bypasses that cooldown.

### Amortization Gate

Behind the fire line, a fold has to earn its place. Two regimes:

- **Genuine turn boundary** (between a user message and its first API call): crossing the line is enough. Nothing is mid-flight, so no execution state is lost, and the new turn rewrites the cache tail anyway.
- **Mid-turn**: the fold must be amortized over the work the session's own pace predicts.

```
expected_calls = (median_calls_per_turn − calls_this_turn)⁺ + median_calls_per_turn × turns_seen
                 (never below calls_this_turn; first turn: calls_this_turn)
fold iff expected_calls ≥ runway
     and (current − target_after) × cache_read × expected_calls
         ≥ sent × folder_input + summary × folder_output + target_after × cache_write
```

- `median_calls_per_turn` comes from the last 16 completed genuine turns (`turn_call_counts`); `turns_seen` is the Lindy horizon — a session that has run N turns is expected to run about N more.
- `runway` is the autonomous ladder (5, 10, 20 … per consecutive in-turn fold), so each further fold in one turn needs a longer predicted horizon.
- The fire line itself is a geometric per-turn ladder: the k-th consecutive in-turn fold (or paid decline) doubles it — `threshold × 2^k`, capped one safety margin under the ceiling — so a single long turn gets 70k → 140k → cap of room instead of re-folding at the same mark. A genuine user turn resets the level.
- The amortization calculation uses provider accounting when available and conservative internal fallback weights otherwise.
- `sent` is the part of the drained range the fold prompt actually sends (recent bodies whole, older ones trimmed), and `summary` is the compression model's output budget.

Net effect: a session that crosses the line on its last call does not fold, while a long tool loop folds only after its measured horizon can amortize the rewrite.

### Background Folds

An automatic fold outside the ceiling margin does not block the agent. The prompt is built from the drained range, the decision+summary call runs in a spawned task, and the agent keeps working; the summary is applied at a later round boundary, and only to the exact range it was computed from (a content fingerprint of the drained messages — a changed range discards the summary). One fold is in flight at a time.

- **Turn end**: a finished fold is applied before the session is saved — replace only, never auto-continue. A fold still running stays parked and is collected at the next round; turn end never waits on it.
- **Ceiling margin**: a pending fold is awaited, and its result applied without the veto; with no fold pending the trigger runs inline and forced (see [The Hard Ceiling](#the-hard-ceiling)).
- **Failure cooldown**: a fold that fails, is cancelled, or is discarded holds unforced attempts for one runway of calls (5, 10, 20… on the ladder) instead of retrying on the next round. A slow or broken compression model therefore gets one attempt per runway, never one per call.

### Forced vs Automatic Compression

The `/done` command triggers **forced compression**, which behaves differently from automatic compression:

| Behavior | Forced (`/done`) | Automatic |
|----------|------------------|-----------|
| Exponential cooldown | Bypassed | Applied |
| Amortization gate | Bypassed | Enforced |
| Feasibility check ("won't drop below threshold") | Bypassed | Enforced |
| AI veto | Forced — AI cannot decline | AI may decline |
| Min. conversation messages | 3 | 5 |
| Compression ratio | Fixed gentlest ratio, `2.0x` | Computed from session growth and runway, clamped to `2.0x`–`16.0x`; hard-ceiling folds use `16.0x` |
| Cooldown counters after | Reset to 0 | `consecutive_compressions` incremented |
| Purpose | Session boundary — clean slate | Mid-session cost optimization |

Note that `/done` is **less** aggressive on ratio than a high-pressure automatic compression: it uses the lightest configured level (default 2.0x) with no adaptive adjustment. Its "clean slate" character comes from bypassing the gates and resetting both `consecutive_compressions` and `context_tokens_after_last_compression` to 0, so the next task starts without accumulated compression debt — not from a higher ratio.

### Skill Preservation

Skills injected into context are handled differently depending on the compression trigger:

| Trigger | Skill Preservation Behavior |
|---------|----------------------------|
| Automatic (threshold-based) | All active skills preserved — their content stays in context |
| `/done` (forced) | No injected skills are preserved, including env-loaded skills |
| `skill(forget)` | No immediate compression — the skill is removed from the active list, and its stale content is naturally excluded at the next automatic compression |

**Why `/done` is different:** It marks a task boundary. The next task starts from a clean compressed state and activates or injects only the skills it actually needs.

**Why `skill(forget)` doesn't force compression:** Immediate compression is unnecessary. The forgotten skill's content naturally disappears at the next automatic compression since it's no longer in the active list.

### Context Preservation

Range selection is structural. The kept anchor is the last immutable-preamble message immediately before the first task-stating message; system, welcome, and `<instructions>` scaffolding stay outside the drain, while old user tasks, earlier summaries, and earlier continuation wrappers can fold into the new summary.

Automatic compression preserves the live exchange byte-for-byte. When a fresh user request is newest, it keeps the preceding assistant response plus that request and any intervening control messages. Mid-task, it keeps the latest assistant step and its following tool traffic. `/done` is the deliberate exception and may fold the whole task boundary.

The drained range is replaced by preserved active-skill messages, the generated summary, and—when needed—a continuation envelope carrying the exact previous assistant response and user request. The summary retains older user tasks and the active plan. Automatic compression requires at least five conversational messages in the candidate range; forced `/done` requires three.

### Lossless Archive and Recall

Compression is not one-way. Every drained message is archived verbatim to a per-session JSONL file, and (when the PACT attention/governance machinery is on — governance is on by default) each drain also writes a sidecar index of content-addressed **block IDs** (`b:<hex>`). The compressed summary's `<folded_state>` units cite those IDs, and an `<archive>` pointer in the summary names the file.

The **`recall` tool** closes the loop: the model passes up to 2 cited block IDs per call and gets the exact original messages back. Recalled content arrives as a normal tool result—appended at the tail, never rewriting history—and is subject to the global `mcp_response_tokens_threshold`. If the session has no block registry yet, or an ID is unknown, the tool returns an error instead of guessing or scanning an uncited archive.

### Knowledge Retention

Each compression may extract critical knowledge (decisions, constraints, preferences). New entries are appended and the list is FIFO-trimmed to the most recent N (configurable via `knowledge_retention`, default: 25) — the oldest are dropped when the limit is exceeded. Separately, `analysis_findings_max_tokens` (default `6000`) bounds retained findings by relevance, recency, and diversity; `0` disables that findings channel.

**Intermediate learning.** When `supervisor.learning.enabled = true` and the conversation has at least 3 user messages, each automatic compaction also fires a fire-and-forget lesson-extraction pass. This is asynchronous and never blocks compression. See [Learning](13-learning.md).

### Cache Keepalive

When you walk away after the AI replies, the prompt cache TTL counts down and the next turn may miss cache. Cache keepalive keeps it warm with minimal `max_tokens = 1` idle pings against a frozen snapshot of the conversation:

```toml
cache_keepalive_enabled = false          # opt-in (default false)
cache_keepalive_max_idle_seconds = 1800  # stop pinging 30 min after last activity (0 = until session ends)
```

- **Anthropic-only** today. Only providers whose API supports refresh-on-read are pinged; others (OpenAI implicit cache, Gemini, DeepSeek) are skipped to avoid wasted requests.
- The ping **interval comes from the provider**, not from config.
- Pings only fire when the snapshot actually has a cached message (otherwise there is nothing to keep warm).
- Each ping costs cache-read tokens; those costs are folded back into the session cost.

## Compression Model

Octomind has exactly three persistent model purposes: main `[model]`, shared `[supervisor.model]`, and `[compression.model]`. The compression profile performs both the compression decision and summary generation; it is not a fourth model purpose. The shipped profile uses `octohub:auto`, and omitted override fields inherit from `[model]`.

## Monitoring

Use `/info` to see compression statistics. The `compression` block shows:

```
compression
  conversation       3
  messages removed   128
  tokens saved       45,000
  avg ratio          81.8%
```

- `conversation` — count of conversation compressions (shown only when > 0).
- `messages removed` — cumulative messages drained across all compressions.
- `tokens saved` — cumulative tokens reclaimed.
- `avg ratio` — a saturating heuristic, `tokens_saved / (tokens_saved + 10000)` rendered as a percentage, not a literal compression ratio.

There is no per-compression before/after breakdown and no cost-saved figure in this block.

## Examples

These illustrate the net-benefit logic with rounded token counts. The real engine uses the measured session state and provider accounting available at runtime.

### Profitable Compression

```
Session: 125,000 tokens | Threshold 120,000 fired (adaptive ~4.0x)
Estimated remaining turns: ~8 (many calls still ahead)

Without compression: each future call re-reads ~125k cached tokens
With compression:    one-time cache rewrite + future calls re-read ~31k
Net benefit: positive → COMPRESS
```

### Skipped Compression

```
Session: 62,000 tokens | Threshold 60,000 fired (adaptive ~2.0x)
Estimated remaining turns: 5 (floor — session winding down)

The cache rewrite plus only a few remaining calls
outweighs the savings on those calls.
Net benefit: negative → SKIP (would cost money)
```

## Best Practices

1. **Monitor effectiveness** with `/info` to verify compression saves money
2. **Keep the compression profile explicit** when it should differ from the main profile; the shipped default is `octohub:auto`
3. **Start conservative** with default thresholds, adjust based on workflow
4. **Disable for short sessions** (`threshold = 0`) if sessions rarely reach the trigger (`70000` by default)
5. **Raise `threshold`** if compression triggers too frequently

## Troubleshooting

**Compression not triggering:**
- Check `compression.threshold` is non-zero and actually exceeded.
- If you rely on the hard ceiling, confirm `max_session_tokens_threshold > 0`.
- Use `/info` to see the current token count vs. your thresholds.

**Compression too aggressive:**
- Increase `compression.threshold`; compression depth is computed and has no `target_ratio` config key.

**Compression not reducing repeated context:**
- Revisit the `[compression.model]` profile if it produces poor summaries
- Increase thresholds to compress less frequently
