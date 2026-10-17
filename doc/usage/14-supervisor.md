# Supervisor: Agent Completion Verification and Planning

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
[decision]                # optional calibrated decisions, see below
model = "cloudflare:typesafe/jev"
recall = false
skills = false
capabilities = false
compression = false
supervisor = false

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

[supervisor.gate]          # verify every turn stop: `done` or a hand-back
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

Supervisor writers use the shared profile. Eligible decision calls use `[decision]` when enabled; see below. Omitting
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

Enable `[supervisor.authorizer] enabled = true` to check proposed tool operations against **role + user intent**.
The rule is allow-first: if uncertain, allow. Reasonable investigation and intermediate steps are permitted. Only a
concrete prohibition or scope conflict can block a call. The shared supervisor receives separately labelled role and
user sources, exact tool arguments, and runtime receipts of completed actions. Role text cannot masquerade as a user
quote; injected memory and tool output cannot manufacture permission. Original user text survives pre-model pipes.

Native pre-call guards run first. The authorizer checks the remaining batch before execution, including inline capability
activation and delegated tool loops. An ordinary allow needs one judge call. For a proposed block, the model selects a
source ID and argument path; the runtime copies the exact source text and argument value. The proposed conflict must
then pass a second independent check using the shared supervisor model. A native guard override is also verified.
An unconfirmed or malformed opinion contributes no veto. A confirmed block returns a normal tool error with its original call ID:

```text
[authorizer] Tool not executed: Tests conflict with the user's restriction. user source user:0: "Do not run tests".
```

Blocked calls never enter the guard history or run post-result hooks. Guard history is committed in call order after
authorization; an earlier denied call cannot satisfy a later call's prerequisite. History records admission, not proof
that an admitted parallel call succeeded.

Only independently confirmed denials are reused for identical calls under unchanged user, role, memory, tool-definition,
and completed-action context. Uncertainty and missing prerequisites are never cached as prohibitions. Allow decisions are
not cached. Completed actions have explicit success flags and retain argument/workdir identity separately from untrusted
output text; a successful read in an earlier round satisfies reading first. Missing history is unknown, not proof of
nonexecution. User instructions, the last 16 completed actions, and authorizer counters survive compaction and session resume;
the bounded denial cache is process-local. `/info` shows this session's checked, blocked, cached, and unavailable counts.
These counters and instruction text are not added to anonymous telemetry. Shared supervisor model-call totals retain
their existing process-global accounting behavior.

With learning and evolution enabled, denial observations become candidate leads for the existing verifier and native
guard lifecycle. A denial is never evidence that its own rule is correct. Permanent rules need independently grounded
user evidence, expressible scope, replay cases, shadow matching, and bounded trials. Task-local or conditional restrictions
must not become unconditional permanent guards. A matching generated guard can be superseded for a call by an explicit
current-user correction; user-authored project guards remain authoritative.

Octomind ACP children receive an immutable snapshot of ancestor constraints and completed actions. Child prompts cannot
revoke ancestor constraints. Peers without this extension still run after parent admission, but cannot promise child-level
authorization. In-process dynamic agents receive separate scoped contexts with the same parent boundary.

Unavailable, malformed, over-budget, or timed-out judgments allow calls; failure of the judge is not evidence against
the tool. The full judgment has a 30-second deadline and a 32,000-token request budget. Unsupported blocks are allowed
without becoming learned rules. User cancellation still stops execution at the tool loop, independently of authorization.
Explicit native guards and sandbox protections retain their existing behavior. This is a model-based intent check, not
a guarantee of zero false positives or protection against arbitrary code behavior. Configured pipes and hook scripts
execute at their existing lifecycle points outside this tool-call check.

## Evaluation gates

`[decision]` routes calibrated questions through TypeSafe Jev via octolib's `evaluation` module.
It takes a state and typed questions and returns probabilities without generating text. The four independent flags
and the shared `supervisor` flag default to false. Routing requires `supervisor.enabled`, a nonempty `decision.model`,
and the corresponding flag. Absent or empty model disables routing; malformed nonempty models fail validation naming
`decision.model`. Unknown keys and missing or mistyped booleans fail config parsing.

| Key | Meaning |
|-----|---------|
| `model` | Optional `provider:model`: `cloudflare:typesafe/jev`, `typesafe:jev-latest`, or `octohub:<alias>` / `octohub:auto`. No sampling profile. |
| `recall` | One Noul per ranked scoped candidate; keep at 0.5. Global rules are unchanged. Unavailable keeps candidates. |
| `skills` | When rules abstain, one Choice over inactive domain skills plus `none`; activate at 0.6. Unavailable activates none. |
| `capabilities` | One Choice over the top five inactive capabilities plus `none`; confirm a cosine winner at 0.05 or promote a choice at 0.6. Unavailable keeps cosine selection. |
| `compression` | Before a fold, score summarize-lane tool packets and demote below 0.5 to `archive_reference`, respecting dependencies. Any unavailable window keeps all lanes. |
| `supervisor` | Enables every eligible supervisor decision below, plus existing evolution grading. Writers retain the supervisor model. |

| Supervisor call | Decision behavior | Unavailable outcome |
|-----------------|-------------------|---------------------|
| Gate refutation | One Noul per charged finding over the verifier's evidence; refute at 0.8. | Every original gap stands. |
| Short-lesson verifier | One Noul per candidate over the same transcript; keep at 0.5. | Reject every candidate. |
| Condense selector | Line-aligned chunks (96-token budget), at most 96 questions per window and four windows in flight; keep at 0.5 with neighbors and diagnostics. Existing reconstruction and spill behavior remain. | Entire round untouched. |
| Authorizer prescreen | Three risk Nouls per call; all below 0.5 admits ordinary allows without judges. | Primary writer runs, as on a flagged batch. |
| Authorizer confirmation | One Noul per proposed veto or native-guard override, using the existing verification payload; confirm at 0.5. | Discard proposed vetoes and overrides; native guards remain. |
| Plan prescreen | Below 0.2 declines a request or holds a phase with deterministic feedback. `reassess` is unscored. | Planner writer runs. |
| Retention promotion verifier | One Noul for a rule's universality across projects and roles; support at 0.5. | Keep scoped. |
| Retention merge verifier | One Noul for entailment and preserved boundaries; support at 0.5. Merge text remains a writer call. | No consolidation; independent archival still applies. |
| Repaired experience and orientation verifiers | One grounding Noul over the existing candidate, outcome, and cited evidence; support at 0.5. | Reject candidate, preserving existing records. |
| Evolution grading | Existing outcome and applicability Nouls; applicability floor 0.5. An artifact's first sample fixes its graded or plain-verdict measure for both arms. | Graded artifacts skip the turn; existing verdict sampling remains unchanged. |

Decision mode never retries a replaced decision on the chat model. With decision routing disabled, the original chat
prompts, payloads, and consumers remain unchanged. Gate primary judgment, readback and format repair, resolve
classification and expansion, initial experience verification with repair diagnostics, evolution-proposal verification,
extraction, repair, merge text, and evolution synthesis stay on `[supervisor.model]`. Authorizer and plan prescreens
can skip their writers; a flagged or unavailable prescreen still requires the writer's authored output.

Keys come from the environment, as for every other provider: `CLOUDFLARE_API_KEY` and `CLOUDFLARE_ACCOUNT_ID` for the
`cloudflare` provider, `TYPESAFE_API_KEY` for `typesafe`. Nothing is stored in config.

`octomind evaluate [FILE]` calls the same model with a hand-written `{"state", "questions"}` request and prints the
answers as JSON, so a question can be tried on a real state before it becomes a seam constant, and a script or
guardrail validator can get a calibrated answer without a session. `-m provider:model` overrides the model for one call.
See the [CLI reference](../reference/01-cli-reference.md#octomind-evaluate-file) and
[Calibrated Checks](../use-cases/23-calibrated-checks-with-evaluate.md).

Every decision call is one attempt with a 5-second timeout, no retries, and a 24,000-token state cap. Missing keys,
HTTP failures, transport errors, timeouts, over-cap states, and malformed or incomplete answers produce the seam's
unavailable outcome above. One debug line (`evaluate <seam> unavailable: <reason>`) and the existing `unavailable`
counter record the failure. Module and stats names remain `evaluate`; thresholds and questions live in
`src/supervisor/evaluate.rs`.

Recall, skills, capabilities, and the authorizer prescreen exclude tool outputs and assistant prose from their state.
Confirmation uses the existing full verification payload. Condense and compression judge tool outputs by design;
learning verifiers receive the same cited evidence as their chat counterparts. No production shadow decisions are
added. Offline comparison with logged chat verdicts remains a release prerequisite for the new P2 questions.

`/info` shows, per seam, `calls` (attempts), `unavailable` (unavailable decisions), and `applied` (recall: candidates excluded;
skills: activations; capabilities: cosine decisions overridden, vetoes and promotions alike; authorizer: batches admitted without the supervisor and confirmation rounds answered; condense: results whose inline content
changed; compression: packets demoted; distill: verification rounds answered; plan: planner calls skipped; gate:
refutation passes answered), plus `replaced` where the seam stood in for a chat call (authorizer, condense, distill,
plan, gate). The `evaluate` row totals the seam calls with their input tokens and cost against the chat calls
replaced, with the estimated input tokens those calls would have carried, priced at the supervisor model's reference
input rate when the table knows it. Output tokens and wall time are not counted, so the saving shown is a floor.
Usage lands under one `evaluate` call kind in the supervisor totals and in the session's external spend.

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
uncertain work. `verified` means execution evidence: the gate passed a turn that changed state and then ran a
recognized check on the result. A pass resting on the verifier's reading alone is `unknown`. Supervisor context is
explicit and mid-trajectory steering remains advisory.

## Self-report

When supervision is enabled, the agent is instructed to end each response with a compact structured handoff:

```text
<sup>{"state":"progressing","focus":"inspect authentication routing","next":"read the route handlers","carry":[],"plan":null,"memories":[]}</sup>
```

`state` is one of `exploring`, `progressing`, `blocked`, `need_input`, `done`. The token is **parsed by the supervisor
and stripped before display** — you never see it. `focus`, `next`, and `carry` form a low-cost handoff to conversation
compression. The compressor treats it as an attention hint, grounds it against the transcript, and may promote supported
durable protocol into critical knowledge. It is never evidence by itself, and credential values are forbidden; only
opaque credential pointers may be carried. Legacy one-word and `STATE · reason` reports remain accepted when resuming
older sessions.

| State | Effect |
|-------|--------|
| `done` | Completion claim, checked by the verify-gate |
| `need_input` | Hand-back with a question; the verify-gate checks it is genuine before it reaches you |
| `blocked` | Hand-back; detector hints do not interrupt it, and the verify-gate checks the blocker is genuine |
| `exploring` / `progressing` | Fused with the counters below |

## Detectors

Detectors run in-process after tool rounds without another model call. Their thresholds are fixed constants. The status
report and injected notes still use tokens in normal agent requests.

The first two track **receipt novelty**. A loop compares whole calls: tool name, arguments, result, and error status.
Novelty compares the target a call addresses (its path-like arguments) with the result and error status: different
targets returning the same output are distinct observations, but the same output for the same target under reworded
arguments (a widened window, a bumped limit) is not new information. A fresh error can provide diagnostic information,
and a successful mutation counts as progress even when its receipt repeats.

- **Loop** — identical calls and results repeat for 3 tool rounds without current-round novelty.
- **No-progress** — 5 tool rounds without new receipts or successful mutations.
- **Recovery** — several command executions returned errors without a later success for those exact calls. The
  detector does not establish whether they were tests, expected probes, or obsolete attempts.

All three signals are advisory. Repetition can be legitimate polling, and an error need not require recovery. Hints
never escalate into mandatory strategy changes or blocked handbacks; repeated hints back off. `done`, `blocked`, and
`need_input` are left alone, and no-progress hints are also suppressed while `exploring`. The main model judges relevance
from the actual outputs and user request; a counter does not establish non-compliance.

## Verify-gate

With supervision and the gate enabled, every stop of a user-owned turn — a `done` claim, a hand-back, or a turn that ends
with no status — is checked by an independent model verification pass before it is accepted.

**Free pre-check (no model call):**

- **Unfinished handback** — non-interactive/background execution can nudge an `exploring` or `progressing` response that
  ends without action, within a fixed retry budget.

Mutation and verification detectors supply heuristic context to the model pass; they cannot reject completion by
themselves. A saved artifact can fulfill an artifact request without a separate check. A command can both change state
and exercise behavior, so its output remains available as evidence. The verifier judges the requested outcome against
the resulting artifacts and recorded outputs, and must identify a concrete gap before requesting more work.

A `blocked` or `need_input` stop is a hand-back, and agents use hand-backs to escape unfinished work: the verifier
passes one only when the remaining work truly needs you (a decision, access the environment lacks, or an action the
request forbids), and otherwise sends the agent back to clear the blocker itself.
Pending session-owned background work defers completion. User and standing-instruction prohibitions on verification
remain binding: a forbidden check's absence is compliance, not a gap.

For example, a standing verification prohibition persists until you explicitly change it:

```text
Make the requested documentation edits. Do not run tests; I will test myself.
Now you may run the relevant tests for those edits.
```

Plan outcomes are judged against evidence, not whether every status box is already checked. A `PASS` can close all
remaining bookkeeping items for the applicable plan. With the completion gate disabled, an eligible completion
self-report can finalize it instead.

Machine-checkable plan assumptions (for example `file_exists: src/foo.rs`) resolve relative paths from the session
anchor. A confirmed broken assumption emits `reassess`; filesystem observation errors remain unknown. A planner outage
preserves plan bookkeeping and allows the authorized task to continue.

Model-selected operational excerpts are matched against the current user's exact text before recitation. Selected
excerpts are contextual reminders, not independently binding rules or additional requirements.

**Model pass (rare):** an independent verifier checks the result against your request:

- **Pass** → completion is accepted. When the turn changed state and a recognized check ran on the result, the
  trajectory is labelled verified and only materially used memories receive positive outcome credit; a pass without
  that execution evidence is labelled unknown and earns no correctness credit.
- **Gaps** → an advisory listing the gaps is injected and the turn re-runs, bounded by a fixed re-entry budget.
  Exhaustion ends the turn unverified. Unchanged gaps after new evidence can also stop retries early.
- **Indeterminate** → transport failure or invalid verifier protocol fails closed for the turn. A structurally malformed
  successful response gets one bounded format-only retry; substantive gaps do not. Learning records the trajectory as
  unknown, not failed, and applies no correctness credit.

The verifier can request one bounded read-back of recorded tool evidence. Blocking findings receive a separate
refutation pass before causing rework: with `[decision] supervisor = true` the evaluation model answers it per
finding, otherwise a second supervisor-model call does. Unavailable Jev refutation leaves gaps standing. The primary verifier and chat refutation use the shared supervisor profile; a separate call
does not guarantee a different model family. Change `[supervisor.model]` in the configuration example to choose the profile.

## Adaptive external planning

Planning is exceptional and supervisor-owned. Focused answers and routine work stay plan-free. For work with meaningful
dependent phases, context-loss risk, or a real branch to track, the specialist emits a sparse hidden `request` signal
alongside normal work. A separate supervisor call makes one structured create/no-plan decision from the current request,
specialist instructions and capabilities, bounded current-phase assistant/tool trajectory, and runtime evidence. With
`[decision] supervisor = true` the evaluation model pre-screens `request` and `phase_complete` first and skips the
planner call when it is confident no plan is needed or the phase outcome is not yet evidenced.

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

A rule you set for the rest of the session ("from now on, never push to main") is kept as a **standing session
rule**: the turn classifier records your exact words, the rule is recited on every later turn and pinned through every
compaction, and it survives `--resume`. It stays until you revoke or replace it ("you can push to main now"); asking for work
that would break it is not a revocation, so the agent flags the conflict instead. An
addition or revocation the classifier cannot back with your exact words is discarded, and at most 8 are held at once (oldest first
out). Per-turn prohibitions still come from the latest request only.

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
| Evaluation gates (recall / skills / capabilities / authorizer) | Ranked recall, rule abstention, cosine decision, pending tool batch | One bounded evaluation call per seam per turn | `[decision]` |

## Source reference

| Surface | Source |
|---------|--------|
| Defaults and configuration | [config-templates/default.toml](../../config-templates/default.toml), [src/supervisor/mod.rs](../../src/supervisor/mod.rs) |
| Status and detectors | [src/supervisor/detect.rs](../../src/supervisor/detect.rs), [src/session/chat/response.rs](../../src/session/chat/response.rs) |
| Completion and plan state | [src/supervisor/gate.rs](../../src/supervisor/gate.rs), [src/session/chat/session/api_executor.rs](../../src/session/chat/session/api_executor.rs), [src/supervisor/plan.rs](../../src/supervisor/plan.rs) |
| Condensation and recitation | [src/supervisor/condense.rs](../../src/supervisor/condense.rs), [src/supervisor/recite.rs](../../src/supervisor/recite.rs) |
| Delegation and accounting | [src/supervisor/delegate.rs](../../src/supervisor/delegate.rs), [src/supervisor/stats.rs](../../src/supervisor/stats.rs), [src/session/external_spend.rs](../../src/session/external_spend.rs) |
| Evaluation gates | [src/supervisor/evaluate.rs](../../src/supervisor/evaluate.rs), [src/supervisor/learning/inject.rs](../../src/supervisor/learning/inject.rs), [src/mcp/runtime/skill_auto.rs](../../src/mcp/runtime/skill_auto.rs), [src/mcp/runtime/capability.rs](../../src/mcp/runtime/capability.rs), [src/supervisor/authorizer.rs](../../src/supervisor/authorizer.rs) |

## See also

- [Cross-session learning](13-learning.md)
- [Token efficiency](16-token-efficiency.md)
- [Skills](15-skills.md)
- [Configuration reference](../reference/03-config-reference.md)
