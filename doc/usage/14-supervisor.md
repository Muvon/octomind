# Supervisor

The **supervisor** is an out-of-band control plane that runs *beside* the agent loop — never in your transcript. It watches each turn, keeps the agent on task, verifies completion, and carries knowledge across sessions. Learning is just one of its mechanics.

It exists to make the loop **more precise**: fewer side-tracks, fewer "looks done but isn't" finishes, and no re-discovering what a past session already figured out.

## The closed loop

```
every turn, FREE:
  self-report  ⊕  detectors (counters)      <- two free signals, fused
        │  agree → act with no model
        │  conflict / `done` → ↓
  verify-gate (model, rare)  → labels the run pass/fail
        │
  distill (on pass)  → lessons + orientation written to memory
        │
  recall (next turn/session)  → inject lessons + orientation
        │
  steer  → advisory re-anchor when the agent loops or stalls
```

The **verify-gate is the reward signal**: it labels a run pass/fail, so the supervisor only learns from work it has evidence was correct. Everything injected is **advisory** — a note the agent reads, never a silent rewrite of its context.

## Self-report

When `[supervisor.detectors] self_report = true`, the agent is instructed to end every turn with a one-word status token:

```
<sup>STATE</sup>          # optionally: <sup>STATE · short reason</sup>
```

`STATE` is one of `exploring`, `progressing`, `blocked`, `need_input`, `done`. The token is **parsed by the supervisor and stripped before display** — you never see it. It is the cheapest, most reliable signal of intent, because the agent already knows whether it is stuck, asking you a question, or finished.

| State | Effect |
|-------|--------|
| `done` | Arms the verify-gate |
| `need_input` | Treated as a question — passed to you, **never** gated (no false-positive verification) |
| `blocked` | Triggers a steer note |
| `exploring` / `progressing` | Fused with the counters below |

## Detectors

Deterministic, free, every turn — they cost nothing and decide *when* (rarely) to spend a model call.

- **Loop** — the same tool with the same arguments `loop_threshold` times in a row (default `3`). Unambiguous; no model needed.
- **No-progress** — `no_progress_window` turns (default `5`) with no new information (no new file read, error signature unchanged, no edit applied).

The power is in **fusing** the counter with the self-report: if the counter says "no progress" but the agent reports `progressing`, *that conflict* is the real stuck signal. Agreement needs no model at all.

## Verify-gate

When the agent self-reports `done` and `[supervisor.gate] enabled = true`, an independent pass checks the result against your request before completion is accepted:

- **Pass** → the run is labelled verified; distill is allowed to learn from it.
- **Gaps** → an advisory listing the gaps is injected and the turn re-runs, bounded by `max_iterations` (default `2`, to avoid over-verifying). If gaps remain after the bound, the run is marked unverified and **distill is suppressed** — we never learn from an unverified trajectory.

## Steer

When a detector fires (loop, or no-progress that the self-report doesn't excuse), the supervisor queues an advisory **re-anchor** note — *"you've repeated this without new results; try a different approach, or report `blocked`"* — injected at the next request's safe point. It nudges; it never forces.

## Memory: lessons + orientation

The supervisor keeps two kinds of cross-session memory in one backend:

- **Lessons** — procedural *do / avoid* rules, extracted from your corrections. The deep dive lives in **[Cross-Session Learning](13-learning.md)**.
- **Orientation** — durable, descriptive understanding of the subject (architecture, decisions, constraints) that was expensive to discover and would otherwise be re-explored. Stored under `memory_type = "orientation"` and recalled as **working assumptions to verify**, never as truth.

The rule for what to store: *cache what is expensive to re-derive, never what one search recovers.* A symbol's location is cheap (grep finds it); an architectural decision is not.

## Configuration

The supervisor is configured under `[supervisor]`. It is **strict**: a missing `[supervisor]` section — or any required key within it — is a hard parse error. We own the schema, so we fail loudly instead of degrading to silent defaults.

```toml
[supervisor]
enabled = true
model   = "anthropic:claude-haiku-4-5"   # shared cheap model for gate/reflection

[supervisor.learning]      # procedural lessons — see 13-learning.md
enabled = true
backend = "file"
max_inject = 5

[supervisor.orientation]   # durable subject understanding
enabled = true
max_inject = 5
decay_days = 90

[supervisor.detectors]     # deterministic, free, every turn
loop_threshold = 3
no_progress_window = 5
self_report = true

[supervisor.gate]          # verify on self-reported `done`
enabled = true
max_iterations = 2
```

Every field is documented in [`[supervisor]` — Config Reference](../reference/03-config-reference.md#supervisor).

## Invariants

1. **Free signals gate the model.** Counters and the self-report run every turn at zero cost; the model (verify-gate / drift confirm) is woken only on a `done` or a conflict.
2. **Advisory, never silent rewrite.** Every injection is a note the agent can reason about. A wrong supervisor degrades gracefully instead of corrupting the run.
3. **Out-of-band.** Status tokens are stripped from display; supervisor deliberation never reaches your transcript.

## Mechanics at a glance

| Mechanic | When | Cost | Config |
|----------|------|------|--------|
| Self-report | Every turn | Free | `[supervisor.detectors] self_report` |
| Detectors (loop / no-progress) | Every turn | Free | `[supervisor.detectors]` |
| Verify-gate | On self-reported `done` | Model (rare) | `[supervisor.gate]` |
| Steer | On loop / no-progress | Free | `[supervisor.detectors]` |
| Distill (learn) | End of a verified run | Model (cheap) | `[supervisor.learning]`, `[supervisor.orientation]` |
| Recall | Session start + per turn | Embedding | `[supervisor.learning]`, `[supervisor.orientation]` |
