# Context Compression

Octomind automatically manages conversation context size through intelligent compression. This is the single reference for the compression system.

## Overview

As sessions grow, token costs increase and context windows fill up. The compression system:
1. Monitors token usage against configurable thresholds
2. Decides whether compression would save money (cache-aware economics)
3. Drains older exchanges into an AI-generated summary while re-injecting the most recent intent
4. Retains critical knowledge across compressions

Two related safety nets sit on top of the adaptive engine:
- **The context ceiling** — the lower of `max_session_tokens_threshold` (root config, default `200000`) and the session model's usable window; crossing it force-compresses unconditionally (see [The Hard Ceiling](#the-hard-ceiling)).
- **Cache keepalive** — an opt-in subsystem that keeps the prompt cache warm during idle time (see [Cache Keepalive](#cache-keepalive)).

> **Hints are not the compression engine.** The `hints_*` fields below only control a cosmetic `/plan next` suggestion shown when an active plan exists. They do **not** gate automatic compression — the engine is driven solely by `compression.threshold` and the context ceiling.

## Configuration

```toml
# Root config field — the user half of the hard compression ceiling (0 = model window only)
max_session_tokens_threshold = 200000

[compression]
# hints_* are cosmetic: they only drive the "/plan next" suggestion, NOT compression
hints_enabled = true
hints_pressure_threshold = 0.7
hints_min_interval = 5
knowledge_retention = 10

# The single compression trigger, in absolute tokens (0 = compression disabled).
# Depth is NOT configured — it is computed per cycle from the measured session
# growth rate and the context ceiling.
threshold = 90000

[compression.decision]
model = "openai:gpt-5-mini"
max_tokens = 16000
temperature = 0.3
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
ignore_cost = false
```

See [Configuration Reference](../reference/03-config-reference.md#compression) for all fields.

## How It Works

### Token-Based Triggers

Compression becomes eligible when the full context (messages + system prompt + tool definitions + safety margin) exceeds the **fire line**: `compression.threshold`, pulled down automatically when the model's window is small so at least 5 turns of measured growth still fit below the ceiling.

**Computed depth.** How deep each compression goes is not configured. The controller picks the post-compression token target directly from measured session dynamics:

```
target_after = ceiling − runway × growth
```

- `growth` — measured output tokens per API call since the last compression checkpoint (lifetime average before the first compression)
- `runway` — predicted remaining calls: the symmetry estimate (work remaining ≈ work done) corrected by the self-tuning accuracy of the previous prediction

The target is clamped between the deepest and gentlest achievable sizes (derived ratio always lands in **[2.0, 16.0]**) and must fall at least 5 turns of growth below the fire line — a compression that would re-fire immediately is refused (cooldown instead). The effect: a hot session (high growth, long predicted runway) compresses deep and buys a long quiet stretch; a winding-down session compresses gently and preserves fidelity. Compressing to a stable watermark also keeps the post-compression prefix size consistent, which is what keeps the prompt cache effective across cycles.

(Forced `/done` compression skips the controller and uses the gentlest fixed 2.0x — it is a task boundary, so there are no session dynamics to project onto the next task; see [Forced vs Automatic Compression](#forced-vs-automatic-compression).)

### The Hard Ceiling

The context ceiling is the lower of `max_session_tokens_threshold` (root config, default `200000`) and the session model's physical window minus the reserved completion budget (`max_tokens`). When the full-context token count reaches it, compression is **forced unconditionally** — it bypasses the exponential cooldown, the cache-aware cost analysis, the feasibility check, and the AI's veto (the decision model cannot decline). The ratio used is the deepest allowed (**16.0x**).

`max_session_tokens_threshold` is also the denominator for the `/plan next` hint pressure calculation. Setting it to `0` leaves the model-window half of the ceiling in force but disables the hints.

### Exponential Cooldown

To prevent compression loops during tool-heavy operations, each consecutive compression (without a user message between them) doubles the token growth required before the next compression is allowed. The required growth is `min(0.10 × 2ⁿ, 1.0)`, where `n` is the number of compressions already performed:

| After this many compressions | Required Growth Before Re-compression |
|------------------------------|---------------------------------------|
| 1st | 20% |
| 2nd | 40% |
| 3rd | 80% |
| 4th+ | 100% (capped — context must double) |

The watermark check is inactive until the first compression sets `context_tokens_after_last_compression > 0`. The cooldown resets when escalation stops (a check that finds nothing to compress) and on forced `/done` compression.

Two escape hatches set the **same** watermark (`context_tokens_after_last_compression = current_tokens`) to suppress re-analysis until context grows again — they are not a separate cooldown mechanism: (1) the chosen compression range is empty (`start_idx >= end_idx`), and (2) the depth controller finds no feasible target — even the deepest fold could not land usefully below the fire line. Both run before the cost analysis.

### Cache-Aware Economics

Before compressing, the system calculates net benefit:

```
net_benefit =
    (cost of remaining turns with full context)
  - (compression cost + cache invalidation cost + cost of remaining turns with compressed context)
```

**If net_benefit > 0**: Compress (saves money).
**If net_benefit <= 0**: Skip (would cost money).

**Cost factors** use **per-model pricing fetched from the provider** (`ModelPricing`: input / output / cache-write / cache-read per 1M tokens). The cache-write and cache-read multipliers vary by provider; the figures below are illustrative Anthropic-typical values, not octomind constants:

- Cache write: ~1.25x base token cost (illustrative)
- Cache read: ~0.1x base token cost (~90% savings on cached content; illustrative)
- Compression cost: a single combined decision+summary LLM call (see below)
- Cache invalidation: compression forces a cache rewrite of the surviving prefix

Two short-circuits replace the cost math when pricing is unusable:
- **Free/zero-priced session model** (e.g. local `ollama`): always compress, for context management (cost is irrelevant).
- **Pricing unavailable**: skip compression — unless `ignore_cost = true`, which treats missing pricing as zero cost and compresses anyway.

**One combined LLM call.** Compression is decided and summarized in a **single** request (`ask_ai_decision_and_summary`) that returns a typed `CompressionSummary` carrying both `should_compress` and the full narrative sections — there is no separate decision call followed by a summarization call. The call uses JSON-schema mode for providers that support structured output, and an XML-tagged prompt otherwise. If the model returns `should_compress = true` but every narrative field is empty, compression is **refused** (the substantive-summary gate) to avoid wiping context with a header-only summary.

**Future turn estimation** uses no time or velocity signal. It is:

```
estimate    = min(headroom / growth_rate, api_calls_so_far)
future_turns = max(estimate × accuracy, 5)
```

- `headroom` = tokens freed by this compression; `growth_rate` = output tokens per call (incremental since the last compression, else lifetime average).
- `api_calls_so_far` is the symmetry estimate (work remaining ≈ work done). When there are no calls yet (cold start), the physical ceiling is capped at **100** instead.
- `accuracy` is a self-tuning factor (actual ÷ predicted from the last cycle, clamped **[0.25, 4.0]**) that corrects systematic over/under-estimation.
- The result is floored at **5**. There is no "calls per minute", velocity decay, or "2x current calls" cap.

### Forced vs Automatic Compression

The `/done` command triggers **forced compression**, which behaves differently from automatic compression:

| Behavior | Forced (`/done`) | Automatic |
|----------|------------------|-----------|
| Exponential cooldown | Bypassed | Applied |
| Cost gate (`net_benefit`) | Bypassed | Enforced |
| Feasibility check ("won't drop below threshold") | Bypassed | Enforced |
| AI veto | Forced — AI cannot decline | AI may decline |
| Min. conversation messages | 3 | 5 |
| Compression ratio | First level's `target_ratio` (default 2.0), no adaptive scaling | Adaptive, clamped [1.5, 4.0] |
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

**Why `skill(forget)` doesn't force compression:** Immediate compression would be expensive and unnecessary. The forgotten skill's content naturally disappears at the next automatic compression since it's no longer in the active list.

### Context Preservation

Range selection is purely structural — there is no semantic grouping, importance weighting, discourse-flow analysis, or "last N turns kept verbatim" carve-out. The engine:

1. Picks an **anchor**: the latest `<instructions>` user message, or (if none) the first user message. The anchor is kept.
2. **Drains everything** between the anchor and the end of the conversation (`anchor_idx + 1` through the last message).
3. Re-inserts, in order: preserved active-skill messages, the AI-generated summary, then a synthetic `<continuation>` wrapper.

The only recent context that survives is therefore carried by the **summary** and the **continuation wrapper** — not by uncompressed turns:

- **Summary** — an AI-generated entry that begins with a `## USER TASKS` list of up to the **last 4 older user requests** (raw, not AI-rephrased, so intent is never lost), followed by the narrative sections. The current active plan (if any) is appended so the model needn't spend a turn recovering it.
- **`<continuation>` wrapper** — a synthetic user message carrying the most recent real user intent inside a `<task>` tag. It signals an in-progress task (preventing "fresh start" hallucinations) and is tagged so the next compression cycle's USER TASKS list skips it.

(For minimum-message gating, automatic compression needs at least 5 conversational messages after the anchor; forced `/done` lowers this to 3.)

### Lossless Archive and Recall

Compression is not one-way. Every drained message is archived verbatim to a per-session JSONL file, and (when the PACT attention/governance machinery is on — governance is on by default) each drain also writes a sidecar index of content-addressed **block IDs** (`b:<hex>`). The compressed summary's `<folded_state>` units cite those IDs, and an `<archive>` pointer in the summary names the file.

The **`recall` tool** closes the loop: the model passes up to 2 cited block IDs per call and gets the exact original messages back (digest-verified). Recalled content arrives as a normal tool result — appended at the tail, never rewriting history — so the prompt cache stays intact, and it folds back into the next compression cycle automatically once it stops being referenced. The response is capped by the global `mcp_response_tokens_threshold` truncation like any other tool output. Sessions without a block index fall back to reading the archive file directly.

### Knowledge Retention

Each compression may extract critical knowledge (decisions, constraints, preferences). New entries are appended and the list is FIFO-trimmed to the most recent N (configurable via `knowledge_retention`, default: 10) — the oldest are dropped when the limit is exceeded. The retained entries are injected into every subsequent compression so the AI never loses essential context.

**Intermediate learning.** When `supervisor.learning.enabled = true` and the conversation has at least `supervisor.learning.min_messages_for_intermediate` user messages (default 3), each automatic compaction also fires a fire-and-forget lesson-extraction pass. This is asynchronous and never blocks compression. See [Learning](13-learning.md).

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

## Decision Model

Use a fast, cheap model for compression decisions to minimize overhead. Relative cost ranking (the dollar figures are rough illustrative estimates, not measured guarantees):

| Model | Relative Cost | Recommendation |
|-------|---------------|----------------|
| `openai:gpt-5-mini` | cheapest | Default (fast, cheap) |
| `anthropic:claude-haiku-4-5` | ~$0.0003 per decision | Alternative |
| `anthropic:claude-sonnet-4` | ~$0.003 per decision (~10x Haiku) | More capable, more expensive |

Set `ignore_cost = true` in `[compression.decision]` to exclude compression decision costs from session cost tracking.

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

These illustrate the net-benefit logic with the default pressure levels. Numbers are rounded for clarity; the real engine uses provider-reported pricing and the estimation model described above.

### Profitable Compression

```
Session: 125,000 tokens | Threshold 120,000 fired (adaptive ~4.0x)
Estimated remaining turns: ~8 (many calls still ahead)

Without compression: each future call re-reads ~125k cached tokens
With compression:    one-time cache rewrite + future calls re-read ~31k
Net benefit: positive --> COMPRESS
```

### Skipped Compression

```
Session: 62,000 tokens | Threshold 60,000 fired (adaptive ~2.0x)
Estimated remaining turns: 5 (floor — session winding down)

The cache-rewrite cost now plus a few cheap remaining calls
outweighs the savings on those calls.
Net benefit: negative --> SKIP (would cost money)
```

## Best Practices

1. **Monitor effectiveness** with `/info` to verify compression saves money
2. **Use a cheap decision model** -- `openai:gpt-5-mini` is the default; `anthropic:claude-haiku-4-5` is a good alternative
3. **Start conservative** with default thresholds, adjust based on workflow
4. **Disable for short sessions** (`threshold = 0`) if sessions rarely reach the trigger (90k by default)
5. **Raise `threshold`** if compression triggers too frequently

## Troubleshooting

**Compression not triggering:**
- Check `compression.threshold` is non-zero and actually exceeded.
- If you rely on the hard ceiling, confirm `max_session_tokens_threshold > 0`.
- Use `/info` to see the current token count vs. your thresholds.
- Note: `hints_enabled` does **not** control compression. It only gates the cosmetic `/plan next` hint (which additionally requires an active plan and a non-zero `max_session_tokens_threshold`). Changing it will not make compression trigger.

**Compression too aggressive:**
- Lower `target_ratio` values (e.g., 2.0 instead of 4.0)
- Increase `threshold` values (e.g., 75,000 instead of 50,000)

**Compression not saving money:**
- Use a cheaper `[compression.decision]` model
- Increase thresholds to compress less frequently
- Set `ignore_cost = true` if tracking is misleading
