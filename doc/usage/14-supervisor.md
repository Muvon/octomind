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

When `[supervisor.detectors] self_report = true`, the agent ends every turn with a compact structured handoff:

```
<sup>{"state":"STATE","focus":"current subgoal and why","next":"next action","carry":["minimum resume-critical fact or opaque reference"]}</sup>
```

`STATE` is one of `exploring`, `progressing`, `blocked`, `need_input`, `done`. The token is **parsed by the supervisor and stripped before display** — you never see it. `focus`, `next`, and `carry` form a low-cost handoff to conversation compression. The compressor treats it as an attention hint, grounds it against the transcript, and may promote supported durable protocol into critical knowledge. It is never evidence by itself, and credential values are forbidden; only opaque credential pointers may be carried. Legacy one-word and `STATE · reason` reports remain accepted when resuming older sessions.

| State | Effect |
|-------|--------|
| `done` | Arms the verify-gate |
| `need_input` | Treated as a question — passed to you, **never** gated (no false-positive verification) |
| `blocked` | Triggers a steer note |
| `exploring` / `progressing` | Fused with the counters below |

## Detectors

Deterministic, free, every turn — they cost nothing and decide *when* (rarely) to spend a model call.

Both derive from one primitive — **information novelty**: did the action add new information? A mutation (edit/write) always advances state; a read/search advances only when its result is one not seen recently.

- **Loop** — the same *result* repeats `loop_threshold` times in a row (default `3`). Keyed on the result, so reworded calls that return the same thing are caught too. Unambiguous; no model needed.
- **No-progress** — `no_progress_window` actions (default `5`) with **zero novelty** — churn, not genuine work.
- **Truncation** — `truncation_threshold` truncated tool results in a row: the model is re-querying without narrowing instead of reading the spill file.
- **Dedup** — `dedup_threshold` deduplicated results in a row: the model is re-issuing calls whose output it already received.
- **Distraction** (opt-in, `distraction_threshold = 0` is off) — a result is drift when its embedding cosine to the centroid of recent results falls below `drift_floor`. Self-referential (no task anchor needed); costs one embedding per sizable tool result when enabled. The centroid follows every result, so a coherent move to another subsystem re-anchors after a couple of results — only wandering that never anchors keeps the streak alive. Condensed results are not scored (they carry the condenser's wording, not the tool's output).
- **Sequential** (opt-in, `sequential_threshold = 0` is off) — single-tool-call rounds in a row where independent calls could have been batched into one parallel round. `sequential_max_steers_per_turn` caps emitted advisories within one genuine user turn; `0` is unlimited, and successful compression starts a fresh budget.

The power is in **fusing** the counter with the self-report: if the counter says "no progress" but the agent reports `progressing`, *that conflict* is the real stuck signal. The full fusion table: any `done` defers to the gate; no-progress while `exploring` waits; loop, or no-progress otherwise, steers. Agreement needs no model at all.

## Verify-gate

When the agent self-reports `done` and `[supervisor.gate] enabled = true`, the claim is checked before completion is accepted — free deterministic pre-gates first, an independent model pass only if those pass:

**Free pre-gates (no model call):**

- **Mutation → check** (`require_check_after_mutation`) — state was changed but no successful command execution ran since the change. Tool-agnostic: any non-mutation command that succeeds on an unchanged tree counts as a check, so it works for any domain (build/test/lint, booking confirmations, health checks).
- **Plan complete** (`require_plan_complete`) — the live plan checklist still has open items.
- **Plan coverage** — the plan advanced (tasks completed) but the evidence ledger shows no real tool actions since; finishing paperwork is not doing the work.
- **Plan conditions** — an open task's declared `valid_if` condition (e.g. `file_exists: src/foo.rs`) is deterministically broken right now.

**Model pass (rare):** an independent verifier checks the result against your request:

- **Pass** → the run is labelled verified; distill is allowed to learn from it.
- **Gaps** → an advisory listing the gaps is injected and the turn re-runs, bounded by `max_iterations` (default `2`, to avoid over-verifying). If gaps remain after the bound, the run is marked unverified and **distill is suppressed** — we never learn from an unverified trajectory.

Set `verifier_model` to a **different model family** than your agent model — a same-family verifier inherits the same blind spots and rubber-stamps them.

### Evidence-bound claims

With `claim_check = true`, the agent backs load-bearing facts with a verbatim quote inside an `<evidence locator="source:location">…</evidence>` tag (hidden from the user's display, kept in the stored message). Each quoted line is verified deterministically — it must occur in an actual tool result — each file:line reference must hold on disk, and each cited URL must appear in something the agent actually received (tool output or the user's own message). Fabricated citations are re-grounded through the verify-gate. Verification here is abstract: a quote from a web page, a file read, or a command output all count — it is not tied to coding.

### Compaction fidelity

Compression is lossy by design. After a compression is applied, one cheap verifier pass (a *different* model than the summarizer) checks that the surviving view still entails the authoritative pre-compression requirements — the goal plus every explicit constraint. Anything lost is re-injected, so a binding requirement ("never X", a scope boundary) can never be silently dropped by compaction. Fail-open: a verifier outage accepts the compression.

## Steer

When a detector fires (loop, or no-progress that the self-report doesn't excuse), the supervisor queues an advisory **re-anchor** note — *"you've repeated this without new results; try a different approach, or report `blocked`"* — injected at the next request's safe point. It nudges; it never forces.

**Circuit-breaker.** A steer is advisory, so a loop can otherwise ignore it forever and burn tokens. `max_consecutive_steers` (default `0` = off) hard-stops the turn after that many consecutive steered rounds without breakout. Before the hard stop, one cheap **on-track checkpoint** classifies whether the current line of work still serves your request: on-track (retrying a failing check, iterating on a fix) resets the steer counter and gets more room; off-track (drifted to unrequested work, cycling on an irrelevant file) stops immediately. Any uncertainty keeps the hard stop.

## Condense

When a tool round returns oversized results (over `[supervisor.condense] tokens_threshold`), one cheap-model call decides per result what the agent actually needs to see for the current task:

- **All relevant** → kept in full, byte-for-byte.
- **Partly relevant** → only the needed lines. The condenser sees a line-numbered copy and answers with **line ranges**; the kept lines are reconstructed verbatim from the original — the model never retypes content, so nothing can be mis-copied.
- **Irrelevant** → replaced with a deterministic system notice. The condenser cannot write a factual summary that could hallucinate tool output.

It is recoverable: condensation runs only for plain-text results when the active role has a local file-reading tool, the full original is spilled to a session file first, and every condensed result carries the path so the agent can read any cut span on demand. The hard `mcp_response_tokens_threshold` prefix-cut still applies afterwards as the plain-text ceiling. Structured/non-text MCP payloads fail open instead of being flattened and corrupted. Any condenser or response-contract error leaves results untouched. Main session only (layers/agents are not condensed).

Relevance is conditioned on three separate signals: trusted standing context (system prompt, project instructions, and currently active skills), the live goal/request/plan, and the assistant text explaining why the current tool batch was issued. Tool data is serialized as JSON, treated as untrusted reference data, and cannot create instructions for the condenser.

The numbered-view budget is fixed **per round**, not per result. A large result is represented by task/argument matches, diagnostics with context, head and tail lines, and stratified middle samples, all carrying their original line numbers. A partial view can be extracted but never discarded wholesale; selected ranges must fall entirely inside visible spans. The response is atomic: missing, duplicate, unknown, malformed, or unsafe entries keep every original. Error/diagnostic lines are also retained deterministically even if the model overlooks them.

## Memory: lessons + orientation

The supervisor keeps two kinds of cross-session memory in one backend:

- **Lessons** — procedural *do / avoid* rules, extracted from your corrections. The deep dive lives in **[Cross-Session Learning](13-learning.md)**.
- **Orientation** — durable, descriptive understanding of the subject (architecture, decisions, constraints) that was expensive to discover and would otherwise be re-explored. Stored under `memory_type = "orientation"` and recalled as **working assumptions to verify**, never as truth.

The rule for what to store: *cache what is expensive to re-derive, never what one search recovers.* A symbol's location is cheap (grep finds it); an architectural decision is not.

**Self-correcting (the closed loop).** Recall is wired back to the verify-gate's verdict: entries that were in context when a run **passes** get reinforced (importance up); entries present when a run **fails after retries** are decayed (importance down) and dropped once they fall below a floor. A distill-time pass additionally prunes entries that have gone both stale (older than `decay_days`) and weak. So memory is validated by *outcome*, not by assertion — useful knowledge strengthens, misleading or unused knowledge fades out.

**Verified lessons.** Extraction is quote-first: every lesson must carry a verbatim user quote as evidence. At distill time, one batched verifier pass re-checks each candidate lesson's evidence against the transcript and drops any lesson whose quote is unsupported — a lesson the model invented or stretched never reaches storage.

## Recite

On long (already-compacted) sessions, the live goal — anchor intent plus next steps — is re-injected at the context tail each turn, so it stays in the high-attention recency window instead of buried in the mid-transcript summary. Short sessions (no compaction yet, empty anchor) pay nothing. Config: `[supervisor.recite] enabled`.

## Delegate gate

`tap run` / `agent_*` spawn a **context-isolated** child that sees only the prompt string — no transcript, no prior tool output. Before the spawn, one cheap-model call judges each proposed handoff against the parent's goal, live request and plan: is it faithful to what you asked, self-contained (concrete paths, symbols, commands, constraints), does it state the deliverable and the scope edge? A failing handoff is **not spawned** — the gaps come back as a tool error so the agent rewrites the prompt, bounded by `max_revisions` per turn. Fail-open on any gate outage.

## Configuration

The supervisor is configured under `[supervisor]`. It is **strict**: a missing `[supervisor]` section — or any required key within it — is a hard parse error. We own the schema, so we fail loudly instead of degrading to silent defaults.

```toml
[supervisor]
enabled = true
model   = "anthropic:claude-haiku-4-5"   # shared cheap model for gate/reflection
claim_check = true                       # evidence-bound claims, verified deterministically
max_consecutive_steers = 0               # steer circuit-breaker; 0 = off

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
truncation_threshold = 2
dedup_threshold = 2
distraction_threshold = 0  # opt-in embedding drift detector
drift_floor = 0.7
self_report = true
sequential_threshold = 0   # opt-in over-sequencing advisory
sequential_max_steers_per_turn = 0 # 0 = unlimited; successful compression resets it

[supervisor.gate]          # verify on self-reported `done`
enabled = true
max_iterations = 2
verifier_model = "openai:gpt-5-mini"     # recommended: a DIFFERENT family than the agent model
require_check_after_mutation = true
require_plan_complete = true

[supervisor.recite]        # goal recitation on compacted sessions
enabled = true

[supervisor.condense]      # task-aware narrowing of oversized tool outputs
enabled = true
tokens_threshold = 5000
model = "anthropic:claude-haiku-4-5"

[supervisor.delegate]      # handoff quality check before spawning subagents
enabled = true
model = "anthropic:claude-haiku-4-5"
max_revisions = 2
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
| Detectors (loop / no-progress / truncation / dedup / distraction / sequential) | Every turn | Free (distraction: 1 embedding) | `[supervisor.detectors]` |
| Evidence-bound claims | Every answer with repo facts | Free | `[supervisor] claim_check` |
| Free pre-gates (mutation→check, plan complete / coverage / conditions) | On self-reported `done` | Free | `[supervisor.gate]` |
| Verify-gate | On self-reported `done`, pre-gates passed | Model (rare) | `[supervisor.gate]` |
| Compaction fidelity | After each compression | Model (cheap) | — (uses `gate.verifier_model`) |
| Condense | On oversized tool results | Model (cheap) | `[supervisor.condense]` |
| Steer | On loop / no-progress | Free | `[supervisor.detectors]` |
| Steer circuit-breaker + on-track checkpoint | After `max_consecutive_steers` steers | Model (cheap, per breaker trip) | `[supervisor] max_consecutive_steers` |
| Recite | Every turn on compacted sessions | Free | `[supervisor.recite]` |
| Delegate gate | Before each subagent spawn | Model (cheap) | `[supervisor.delegate]` |
| Distill (learn) + lesson verification | End of a verified run | Model (cheap) | `[supervisor.learning]`, `[supervisor.orientation]` |
| Recall | Session start + per turn | Embedding | `[supervisor.learning]`, `[supervisor.orientation]` |
