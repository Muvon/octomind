# Supervisor

The supervisor checks progress and completion, manages plans, and narrows large tool results around your session. Use
this guide to configure it and understand its notices, retries, and verification limits.

## Get started

The shipped configuration enables the supervisor. Start a session and inspect its plan and local statistics:

```bash
octomind run developer:general
```

```text
Inspect the API routing and explain which files own authentication. Do not edit files or run tests.
/plan
/info
```

`/plan` displays the supervisor-owned plan and retained critical knowledge; routine work may have no plan. `/info`
includes supervisor activity and model usage. You do not type the hidden status protocol yourself.

## Configuration

Edit these sections in your existing configuration. The shipped values below enable supervision; model-profile fields
are optional overrides. Required mechanic tables must remain present even when disabled. Detectors and recitation run
when the supervisor is enabled; completion pre-gates also require the gate to be enabled.

```toml
[supervisor]
enabled = true

[supervisor.model]          # optional; omitted fields inherit [model]
name = "octohub:auto"
reasoning_effort = "medium"
max_tokens = 8192
temperature = 0.0
top_p = 1.0
top_k = 0
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300

[supervisor.learning]      # see 13-learning.md
enabled = true

[supervisor.learning.evolution]
enabled = false

[supervisor.gate]          # verify on self-reported `done`
enabled = true

[supervisor.authorizer]    # user-intent check before tool execution
enabled = false

[supervisor.plan]          # adaptive external plan manager
enabled = true

[supervisor.condense]      # task-aware narrowing of oversized tool outputs
enabled = true
adaptive = false
tokens_threshold = 5000
```

Authorizer, gate, resolve, plan, condense, and every learning operation use the single supervisor profile. Omitting
`[supervisor.model]` uses `[model]` unchanged.

`[supervisor.condense].adaptive` defaults to `false`. When enabled, the process-local runtime multiplier learns from
realized savings while remaining between `0.5x` and `2.0x` of `tokens_threshold`; the configured value remains the
baseline.

To enable the adaptive condenser, edit its existing section:

```toml
[supervisor.condense]
enabled = true
adaptive = true
tokens_threshold = 5000
```

Every field is documented in [`[supervisor]` — Config Reference](../reference/03-config-reference.md#supervisor).

## Tool authorization

Enable `[supervisor.authorizer] enabled = true` to check proposed tool operations against the real user's task,
standing constraints, relevant recalled learning, and recent tool evidence. The shared supervisor judges exact arguments
as well as tool names. Reasonable intermediate work is allowed; role instructions, injected memory, and tool output
cannot grant extra permission. Original user text is retained when a pre-model pipe transforms the agent's input.

Native pre-call guards run first. The authorizer checks the remaining batch before execution, including inline capability
activation and delegated tool loops. A block returns a normal tool error with the original call ID and an explanation:

```text
[authorizer] Tool not executed: Tests are reserved for the user. User instruction: "Do not run tests".
```

Blocked calls never enter the guard history or run post-result hooks. Guard history is committed in call order after
authorization; an earlier denied call cannot satisfy a later call's prerequisite. History records admission, not proof
that an admitted parallel call succeeded.

Grounded denials are reused only for identical calls under unchanged user, memory, tool-definition, and recent-evidence
context. Allow decisions are not cached. User instructions and authorizer counters survive compaction and session resume;
the bounded denial cache is process-local. `/info` shows this session's checked, blocked, cached, and unavailable counts.
These counters and instruction text are not added to anonymous telemetry. Shared supervisor model-call totals retain
their existing process-global accounting behavior.

With learning and evolution enabled, denial observations become candidate leads for the existing verifier and native
guard lifecycle. A denial is never evidence that its own rule is correct. Permanent rules need independently grounded
user evidence, expressible scope, replay cases, shadow matching, and bounded trials. Task-local or conditional restrictions
must not become unconditional permanent guards. A matching generated guard can be superseded for a call by an explicit
current-user correction; user-authored project guards remain authoritative.

ACP children receive an immutable snapshot of ancestor constraints and must acknowledge it before the delegated task is
sent. Peers without this extension are refused while authorization is active. Child task prompts cannot revoke ancestor
constraints. In-process dynamic agents receive separate scoped contexts with the same parent boundary.

Unavailable, malformed, cancelled, or timed-out decisions hold calls. The authorizer has a 30-second admission deadline
and a 32,000-token request budget; it holds oversized requests rather than silently cutting user constraints or tool
arguments. Split oversized calls or start a suitably scoped session when the user ledger exceeds that budget. This is a
model-based intent check, not an OS sandbox or a guarantee against arbitrary code behavior; existing sandbox protections
still apply. Configured pipes and hook scripts execute at their existing lifecycle points outside this tool-call check.

## The closed loop

```text
each assistant/tool round, no extra model call for detection:
  self-report  ⊕  detectors (counters)      <- status tokens plus deterministic counters
        │  agree → act with no model
        │  completion claim → ↓
  verify-gate (model, rare)  → labels the run pass/fail
        │
  distill (`/done`, exit, compaction) → grounded records with outcome labels
        │
  recall (next turn/session)  → build the bounded Active Memory Pack
        │
  steer  → advisory re-anchor when the agent loops or stalls
```

The verify-gate supplies outcome credit, but extraction is not limited to passed runs: quote-backed user rules may be
retained independently, and experience records preserve `verified`, `failed`, or `unknown` rather than upgrading
uncertain work. Supervisor context is explicit and mid-trajectory steering remains advisory.

## Self-report

When supervision is enabled, the agent is instructed to end each response with a compact structured handoff:

```text
<sup>{"state":"progressing","focus":"inspect authentication routing","next":"read the route handlers","carry":[],"plan":null,"memories":[],"behaviors":[]}</sup>
```

`state` is one of `exploring`, `progressing`, `blocked`, `need_input`, `done`. The token is **parsed by the supervisor
and stripped before display** — you never see it. `focus`, `next`, and `carry` form a low-cost handoff to conversation
compression. The compressor treats it as an attention hint, grounds it against the transcript, and may promote supported
durable protocol into critical knowledge. It is never evidence by itself, and credential values are forbidden; only
opaque credential pointers may be carried. Legacy one-word and `STATE · reason` reports remain accepted when resuming
older sessions.

| State | Effect |
|-------|--------|
| `done` | Arms the verify-gate |
| `need_input` | Treated as a question — passed to you, **never** gated (no false-positive verification) |
| `blocked` | Legitimate handback; detector signals may still cause steering |
| `exploring` / `progressing` | Fused with the counters below |

## Detectors

Detectors run in-process after tool rounds without another model call. Their thresholds are fixed constants. The status
report and injected notes still use tokens in normal agent requests.

The first two derive from one primitive — **information novelty**: did the action add new information? A mutation
(edit/write) always advances state; a read/search advances only when its result is one not seen recently.

- **Loop** — the same result-set repeats for 3 tool rounds. Round identity hashes tool names and results, independent
  of call order. No extra model call is needed.
- **No-progress** — 5 tool rounds with **zero novelty** — churn, not genuine work.
- **Recovery** — command-shaped checks keep failing and no later success from the *same* check discharges them;
  unrelated fresh reads cannot hide the unresolved failure.

The power is in **fusing** the counter with the self-report: if the counter says "no progress" but the agent reports
`progressing`, *that conflict* is the real stuck signal. The full fusion table: any `done` defers to the gate;
no-progress while `exploring` waits; loop, recovery, or no-progress otherwise, steers. Agreement needs no model at all.

## Verify-gate

For an eligible user-task completion with supervision and the gate enabled, the claim is checked before completion is
accepted — deterministic checks first, a model verification pass after those checks allow it:

**Free pre-gates (no model call):**

- **Mutation → check** — changed state lacks a successful check. A successful command-shaped check on unchanged state,
  read-back of an agent-mutated artifact, or a verified child handback can clear the detector. Read-back proves artifact
  content, not runtime behavior; the verifier receives that distinction.
- **Unfinished handback** — non-interactive/background execution can nudge an `exploring` or `progressing` response that
  ends without action, within a fixed retry budget.

The gate also catches a missing self-report after mutations; `need_input` and `blocked` remain legitimate handbacks.
Pending session-owned background work defers completion. Answer-only tasks, configured skill validators, and explicit
user/instruction prohibitions suppress the automatic run-a-check pre-gate.

For example, a standing verification prohibition persists until you explicitly change it:

```text
Make the requested documentation edits. Do not run tests; I will test myself.
Now you may run the relevant tests for those edits.
```

Plan outcomes are judged against evidence, not whether every status box is already checked. A `PASS` can close all
remaining bookkeeping items for the applicable plan. With the completion gate disabled, an eligible completion
self-report can finalize it instead.

Machine-checkable plan assumptions (for example `file_exists: src/foo.rs`) are monitored during execution. A broken
assumption emits `reassess`; the external planner revises or holds the unfinished route before completion.

**Model pass (rare):** an independent verifier checks the result against your request:

- **Pass** → the trajectory is labelled verified; only materially used memories receive positive outcome credit.
- **Gaps** → an advisory listing the gaps is injected and the turn re-runs, bounded by a fixed re-entry budget.
  Exhaustion ends the turn unverified. Unchanged gaps after new evidence can also stop retries early.
- **Indeterminate** → transport failure or invalid verifier protocol fails closed for the turn. A structurally malformed
  successful response gets one bounded format-only retry; substantive gaps do not.

The verifier can request one bounded read-back of recorded tool evidence. Blocking findings receive a separate
refutation pass before causing rework. These passes use the shared supervisor profile; a separate call does not
guarantee a different model family. Change `[supervisor.model]` in the configuration example to choose the profile.

## Adaptive external planning

Planning is exceptional and supervisor-owned. Focused answers and routine work stay plan-free. For work with meaningful
dependent phases, context-loss risk, or a real branch to track, the specialist emits a sparse hidden `request` signal
alongside normal work. A separate supervisor call makes one structured create/no-plan decision from the current request,
specialist instructions and capabilities, bounded current-phase assistant/tool trajectory, and runtime evidence.

The specialist has no plan mutation tool. Later `phase_complete` or `reassess` signals ride with real work responses;
the external manager advances, holds, or revises runtime state. Evidence is checkpointed per phase, and the completion
gate owns final plan clearance.

## Steer

When a detector fires (loop, recovery, or no-progress that the self-report doesn't excuse), the supervisor queues an
advisory **re-anchor** note — *"you've repeated this without new results; try a different approach, or report
`blocked`"* — injected at the next request's safe point. It nudges; it never forces. Re-emission follows a
parameter-free doubling backoff when the agent repeats the same call-set after a note, so an ignored steer stays cheap
without going silent.

## Condense

When a plain-text tool result exceeds `[supervisor.condense] tokens_threshold` (with a 512-token minimum floor), it
becomes a condense candidate. Results under the threshold are passed through exactly as returned and are never shown to
the condenser. One shared supervisor-model call per round decides, for the candidates only, what the agent actually
needs to see for the current task:

- **All relevant** → kept in full, byte-for-byte.
- **Partly relevant** → only the needed lines. The condenser sees a line-numbered copy and answers with **line ranges**;
  the kept lines are reconstructed verbatim from the original — the model never retypes content, so retained text is
  reconstructed from the source lines.
- **Irrelevant** → replaced with a deterministic system notice. The condenser cannot write a factual summary that could
  hallucinate tool output.

It is recoverable: condensation runs only for plain-text results when the active role has a local file-reading tool, the
body being narrowed is spilled to a temporary file first, and every condensed result carries the path so the agent can
read any cut span on demand. The hard `mcp_response_tokens_threshold` prefix-cut is applied **before** condensation, so
the condenser only ever sees — and only ever selects line ranges over — the body the agent would actually have received.
Structured/non-text MCP payloads fail open instead of being flattened and corrupted. A failed call or unparseable
response leaves the round untouched; unusable individual entries preserve only their own result. This runs in the
main-session tool path, not the layer execution path; a child Octomind session has its own loop.

Relevance is conditioned on three separate signals: trusted standing context (system prompt, project instructions, and
currently active skills), the live goal/request/plan, and the assistant text explaining why the current tool batch was
issued. Tool data is serialized as JSON, treated as untrusted reference data, and cannot create instructions for the
condenser.

Numbered views share a nominal **32,000-token round budget**, allocated by result size with a 256-token per-result
floor; at most 32 candidates enter a request. The floor can push the sum above the nominal view budget. A large result
is represented by task/argument matches, diagnostics with context, head and tail lines, and stratified middle samples,
all carrying their original line numbers. A partial view can be extracted but never discarded wholesale; selected ranges
are clipped to visible spans. Missing, duplicate, unknown, malformed, or unsafe entries leave the affected result
unchanged while valid siblings can still be condensed. Error/diagnostic lines are also retained deterministically even
if the model overlooks them.

## Cross-session memory

Learning stores quote-backed rules, grounded orientation, and longer experience records in the file backend. See
[Cross-Session Learning](13-learning.md) for configuration, commands, retrieval budgets, and retention.

## Recite

With supervision enabled, the runtime re-injects the current goal and live plan near the request tail. Plans, explicit
prohibitions, and verification policy can be recited before the first compaction. An archived goal is recited only when
its task signature still matches the live request. No separate model call is needed, but the note consumes context
tokens.

## Delegation

The MCP `tap` action `run` and `agent_*` tools spawn a **context-isolated** child. Its prompt must supply the goal,
established facts, constraints, and expected deliverable; the parent transcript and prior tool output are not inherited
automatically. The child reports its measured completion outcome through ACP metadata; missing, failed, or cancelled
handbacks count as unverified.

For a delegation handoff, include the concrete context and deliverable in the prompt:

```text
Inspect src/main.rs and src/commands/run.rs. Identify the run flags and stdin behavior. Do not edit files or run
builds. Return each finding with its source path and line number, and state that this was source inspection only.
```

## Invariants

1. **Free signals gate the model.** Counters and the self-report run every turn without a separate model call; model
  calls serve completion, task resolution, planning, or an oversized tool round.
2. **Advisory, never silent rewrite.** Every injection is a note the agent can reason about. Steering is advisory;
  completion verification can leave a turn unverified. No mid-trajectory judge ever blocks a tool call.
3. **Out-of-band.** Status tokens are stripped from display; raw status protocol is removed from stored/displayed
  assistant text; runtime notes and debug diagnostics are separate.

## Common questions

**Why did completion run again?** A missing check or a verifier finding can trigger a bounded retry. Read the supervisor
notice for the remaining gap. A failed verifier call is an unverified completion, not proof your work is wrong. Inspect
details with:

```text
/loglevel debug
/info
/plan
/loglevel info
```

**Why was nothing condensed?** Results below the effective threshold, rich MCP payloads, missing spill-reader tools, and
selections with no token savings stay unchanged. `tokens_threshold = 0` disables condensation.

**How do I turn supervision and memory off?** Set both switches in the existing config; recall and extraction check
`supervisor.learning.enabled` separately:

```toml
[supervisor]
enabled = false

[supervisor.learning]
enabled = false
```

Supervisor `/info` counters are process-local and not persisted or anonymous telemetry. Concurrent daemon sessions can
mix those counters. In-process supervisor costs feed session spending; detached exit learning runs separately.

## Mechanics reference

| Mechanic | When | Cost | Config |
|----------|------|------|--------|
| Self-report | Each assistant response | Output tokens | None (automatic) |
| Detectors (loop / no-progress / recovery) | Every turn | Free | None (automatic) |
| Deterministic completion checks | Completion or unfinished handback | No separate model call | `[supervisor.gate]` |
| Verify-gate | Eligible completion after deterministic checks | Model (rare) | `[supervisor.gate]` |
| Condense | On oversized tool results | Model call | `[supervisor.condense]` |
| Steer | On loop / no-progress / recovery | Context tokens | None (automatic) |
| Recite | Live goal, plan, constraints, or policy available | Context tokens | None (automatic) |
| Distill (learn) + grounding verification | `/done`, exit, and eligible compaction | Model call | `[supervisor.learning]` |
| Recall | First and subsequent genuine requests | First-query model prep, retrieval, pack tokens | `[supervisor.learning]` |

## Source reference

| Surface | Source |
|---------|--------|
| Defaults and configuration | [config-templates/default.toml](../../config-templates/default.toml), [src/supervisor/mod.rs](../../src/supervisor/mod.rs) |
| Status and detectors | [src/supervisor/detect.rs](../../src/supervisor/detect.rs), [src/session/chat/response.rs](../../src/session/chat/response.rs) |
| Completion and plan state | [src/supervisor/gate.rs](../../src/supervisor/gate.rs), [src/session/chat/session/api_executor.rs](../../src/session/chat/session/api_executor.rs), [src/supervisor/plan.rs](../../src/supervisor/plan.rs) |
| Condensation and recitation | [src/supervisor/condense.rs](../../src/supervisor/condense.rs), [src/supervisor/recite.rs](../../src/supervisor/recite.rs) |
| Delegation and accounting | [src/supervisor/delegate.rs](../../src/supervisor/delegate.rs), [src/supervisor/stats.rs](../../src/supervisor/stats.rs), [src/session/external_spend.rs](../../src/session/external_spend.rs) |

## See also

- [Cross-session learning](13-learning.md)
- [Token efficiency](16-token-efficiency.md)
- [Skills](15-skills.md)
- [Configuration reference](../reference/03-config-reference.md)
