# Context Compression

Octomind automatically manages conversation context size through intelligent compression. This is the single reference for the compression system.

## Overview

As sessions grow, token costs increase and context windows fill up. The compression system:
1. Monitors token usage against configurable thresholds
2. Decides whether compression would save money (cache-aware economics)
3. Drains older exchanges into an AI-generated summary while re-injecting the most recent intent
4. Retains critical knowledge across compressions

Two related safety nets sit on top of the pressure-level engine:
- **`max_session_tokens_threshold`** (root config, default `200000`) — a hard ceiling that force-compresses unconditionally once exceeded (see [The Hard Ceiling](#the-hard-ceiling)).
- **Cache keepalive** — an opt-in subsystem that keeps the prompt cache warm during idle time (see [Cache Keepalive](#cache-keepalive)).

> **Hints are not the compression engine.** The `hints_*` fields below only control a cosmetic `/plan next` suggestion shown when an active plan exists. They do **not** gate automatic compression — the engine is driven solely by `pressure_levels` and `max_session_tokens_threshold`.

## Configuration

```toml
# Root config field — the hard compression ceiling (0 = disabled)
max_session_tokens_threshold = 200000

[compression]
# hints_* are cosmetic: they only drive the "/plan next" suggestion, NOT compression
hints_enabled = true
hints_pressure_threshold = 0.7
hints_min_interval = 5
knowledge_retention = 10

[[compression.pressure_levels]]
threshold = 60000
target_ratio = 2.0    # Light: 50% reduction

[[compression.pressure_levels]]
threshold = 120000
target_ratio = 4.0    # Medium: 75% reduction

[[compression.pressure_levels]]
threshold = 160000
target_ratio = 8.0    # Aggressive: 87.5% reduction (clamped to 4.0x for automatic compression — see below)

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

Compression triggers when the full context (messages + system prompt + tool definitions + safety margin) exceeds a pressure level threshold. The highest matched threshold determines the **base** compression ratio, but the level actually applied is **escalated** by the number of consecutive compressions, clamped at the highest level — it never wraps back to a lighter ratio under sustained pressure. This prevents infinite loops when compress-all drops context hard and it grows back to the same threshold repeatedly.

| Token Count | Configured Ratio | Effect |
|-------------|------------------|--------|
| 60,000+ | 2.0x | 50% reduction |
| 120,000+ | 4.0x | 75% reduction |
| 160,000+ | 8.0x | 87.5% reduction |

**Adaptive ratio.** The matched ratio is not used verbatim for automatic compression. It is scaled by recent session activity and then clamped to **[1.5, 4.0]**:

- ×1.2 when an active plan exists (longer session expected → compress harder)
- ×1.15 when tool density (`tool_calls / api_calls`) > 2.5 (heavy exploration)
- ×1.0 when tool density > 1.0 (normal)
- ×0.9 when tool density > 0.3 (winding down)
- ×0.8 otherwise (session ending soon)

Adaptive scaling only kicks in after the first 5 API calls; before that the base ratio is trusted as-is. Because the final value is clamped at 4.0, the configured **8.0 level effectively caps at 4.0x for automatic compression** — the full 8.0 ratio is never applied. (Forced `/done` compression skips adaptive scaling and uses the first level's ratio directly; see [Forced vs Automatic Compression](#forced-vs-automatic-compression).)

### The Hard Ceiling

`max_session_tokens_threshold` (root config, default `200000`) is the single most important compression control. When the full-context token count reaches this value, compression is **forced unconditionally** — it bypasses the exponential cooldown, the cache-aware cost analysis, the feasibility check, and the AI's veto (the decision model cannot decline). The ratio used is the **maximum `target_ratio` across all pressure levels**.

This same value is the denominator for the `/plan next` hint pressure calculation. Setting it to `0` disables **both** the hard ceiling and the hints.

### Exponential Cooldown

To prevent compression loops during tool-heavy operations, each consecutive compression (without a user message between them) doubles the token growth required before the next compression is allowed. The required growth is `min(0.10 × 2ⁿ, 1.0)`, where `n` is the number of compressions already performed:

| After this many compressions | Required Growth Before Re-compression |
|------------------------------|---------------------------------------|
| 1st | 20% |
| 2nd | 40% |
| 3rd | 80% |
| 4th+ | 100% (capped — context must double) |

The watermark check is inactive until the first compression sets `context_tokens_after_last_compression > 0`. The cooldown resets when escalation stops (a check that finds nothing to compress) and on forced `/done` compression.

Two escape hatches set the **same** watermark (`context_tokens_after_last_compression = current_tokens`) to suppress re-analysis until context grows again — they are not a separate cooldown mechanism: (1) the chosen compression range is empty (`start_idx >= end_idx`), and (2) compression would not bring context below the threshold that fired. Both occur only inside the `net_benefit > 0` branch.

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
| `/done` (forced) | Only env-loaded skills (`OCTOMIND_SKILLS`) preserved — manually activated skills are dropped |
| `skill(forget)` | No immediate compression — the skill is removed from the active list, and its stale content is naturally excluded at the next automatic compression |

**Why `/done` is different:** It marks a task boundary. You want a clean slate for the next task, so only permanently configured skills (loaded via `OCTOMIND_SKILLS`) survive the compression.

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

### Knowledge Retention

Each compression may extract critical knowledge (decisions, constraints, preferences). New entries are appended and the list is FIFO-trimmed to the most recent N (configurable via `knowledge_retention`, default: 10) — the oldest are dropped when the limit is exceeded. The retained entries are injected into every subsequent compression so the AI never loses essential context.

**Intermediate learning.** When `learning.enabled = true` and the conversation has at least `learning.min_messages_for_intermediate` user messages (default 3), each automatic compaction also fires a fire-and-forget lesson-extraction pass. This is asynchronous and never blocks compression. See [Learning](13-learning.md).

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
4. **Disable for short sessions** (empty `pressure_levels`) if sessions rarely reach the lowest threshold (60k by default)
5. **Increase thresholds** if compression triggers too frequently

## Troubleshooting

**Compression not triggering:**
- Check `[[compression.pressure_levels]]` is non-empty and a threshold is actually exceeded.
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
