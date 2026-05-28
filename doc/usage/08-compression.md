# Context Compression

Octomind automatically manages conversation context size through intelligent compression. This is the single reference for the compression system.

## Overview

As sessions grow, token costs increase and context windows fill up. The compression system:
1. Monitors token usage against configurable thresholds
2. Decides whether compression would save money (cache-aware economics)
3. Compresses older exchanges while preserving recent context
4. Retains critical knowledge across compressions

## Configuration

```toml
[compression]
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
target_ratio = 8.0    # Aggressive: 87.5% reduction

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

Compression triggers when the full context (messages + system prompt + tool definitions + safety margin) exceeds a pressure level threshold. The highest matched threshold determines the **base** compression ratio, but the actual level used is **escalated** based on consecutive compressions (round-robin through levels). This prevents infinite loops when compress-all drops context hard and it grows back to the same threshold repeatedly.

| Token Count | Compression | Effect |
|-------------|-------------|--------|
| 60,000+ | 2.0x | 50% reduction |
| 120,000+ | 4.0x | 75% reduction |
| 160,000+ | 8.0x | 87.5% reduction |

### Exponential Cooldown

To prevent compression loops during tool-heavy operations, consecutive compressions (without a user message between them) require increasing token growth:

| Consecutive Compressions | Required Growth Before Re-compression |
|--------------------------|--------------------------------------|
| 1st | 10% |
| 2nd | 20% |
| 3rd | 40% |
| 4th+ | 80-100% (capped) |

The cooldown resets when the user sends a new message. It also activates when the compression range is invalid (e.g., `start_idx >= end_idx`) or when compression won't bring context below the threshold — preventing futile re-analysis loops.

### Cache-Aware Economics

Before compressing, the system calculates net benefit:

```
net_benefit =
    (cost of remaining turns with full context)
  - (compression cost + cache invalidation cost + cost of remaining turns with compressed context)
```

**If net_benefit > 0**: Compress (saves money).
**If net_benefit <= 0**: Skip (would cost money).

**Cost factors:**
- Cache write cost: 1.25x base token cost
- Cache read cost: 0.1x base token cost (90% savings on cached content)
- Compression cost: AI decision + summarization (typically 2-3k tokens)
- Cache invalidation: compression forces cache rewrite at 1.25x cost

**Future turn estimation** uses velocity-based analysis:
- Tracks actual calls/minute during the session
- Accounts for session lifecycle (early sessions have more remaining time)
- Applies velocity decay (sessions slow down over time)
- Bounded: minimum 5 calls, maximum 2x current calls or 100

### Forced vs Automatic Compression

The `/done` command triggers **forced compression**, which behaves differently from automatic compression:

| Behavior | Forced (`/done`) | Automatic |
|----------|-------------------|-----------|
| Minimum context threshold | Bypassed (20% floor ignored) | Respected |
| Exponential cooldown | Resets counters | Applied normally |
| Aggressiveness | More aggressive cleanup | Preserves more context |
| Purpose | Session boundary — clean slate | Mid-session cost optimization |

Forced compression marks a task boundary. It resets cooldown counters so the next task starts fresh without accumulated compression debt.

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

- Last 4 turns (2 exchanges) always remain uncompressed
- Semantic grouping preserves related messages together
- Importance weighting prioritizes recent and tool-related messages
- Discourse flow maintains reasoning chains

### Knowledge Retention

Each compression may extract critical knowledge (decisions, constraints, preferences). The last N entries (configurable via `knowledge_retention`, default: 10) are injected into every subsequent compression, ensuring the AI never loses essential context.

## Decision Model

Use a fast, cheap model for compression decisions to minimize overhead:

| Model | Cost per Decision | Recommendation |
|-------|-------------------|----------------|
| `openai:gpt-5-mini` | ~$0.0001 | Default (fast, cheap) |
| `anthropic:claude-haiku-4-5` | ~$0.0003 | Alternative |
| `anthropic:claude-sonnet-4` | ~$0.003 | 10x more expensive |
Set `ignore_cost = true` in `[compression.decision]` to exclude compression decision costs from session cost tracking.

## Monitoring

Use `/info` to see compression statistics:

```
Compression Statistics:
  Total compressions: 3
  Average reduction: 72.5%
  Total tokens saved: 45,000
  Cost saved: $0.045

  Last compression:
    Before: 98,500 tokens
    After: 24,625 tokens (4.0x compression)
    Cost saved: $0.0225
```

## Examples

### Profitable Compression

```
Session: 95,000 tokens | Threshold: 100,000 (4.0x)
Estimated remaining turns: 5

Without compression: 5 * 95k * $0.003 = $1.425
With compression:    cache invalidation + 5 * 24k * $0.003 = $0.594
Net benefit: $0.831 --> COMPRESS
```

### Skipped Compression

```
Session: 55,000 tokens | Threshold: 50,000 (2.0x)
Estimated remaining turns: 1

Without compression: 1 * 55k * $0.003 = $0.165
With compression:    cache invalidation + 1 * 28k * $0.003 = $0.220
Net benefit: -$0.055 --> SKIP (would cost money)
```

## Best Practices

1. **Monitor effectiveness** with `/info` to verify compression saves money
2. **Use a cheap decision model** -- `openai:gpt-5-mini` is the default; `anthropic:claude-haiku-4-5` is a good alternative
3. **Start conservative** with default thresholds, adjust based on workflow
4. **Disable for short sessions** if sessions rarely exceed 50k tokens
5. **Increase thresholds** if compression triggers too frequently

## Troubleshooting

**Compression not triggering:**
- Verify `hints_enabled = true`
- Check `[[compression.pressure_levels]]` is not empty
- Use `/info` to see current token count vs. thresholds

**Compression too aggressive:**
- Lower `target_ratio` values (e.g., 2.0 instead of 4.0)
- Increase `threshold` values (e.g., 75,000 instead of 50,000)

**Compression not saving money:**
- Use a cheaper `[compression.decision]` model
- Increase thresholds to compress less frequently
- Set `ignore_cost = true` if tracking is misleading
