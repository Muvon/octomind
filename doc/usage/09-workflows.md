# Workflows

`octomind workflow <file.toml>` is an external orchestrator that chains multiple `octomind run` invocations into a multi-step process. Each step is an independent subprocess; outputs flow between steps by name; per-step responses, progress, costs, and totals are written to **stderr** for a human to watch (stdout stays empty unless you pass `--format jsonl`).

> **By default a real run writes nothing to stdout** — the human view is on stderr, and stdout carries only the `--dry-run` plan. For a machine-readable result, pass **`--format jsonl`**: each step emits an `assistant` JSON line to stdout as it completes (the last is the final result), followed by an aggregated `cost` line (see [Machine-readable output](#machine-readable-output---format-jsonl)). Without that flag, a shell pipeline reading the workflow's stdout gets nothing — use `--format jsonl`, read stderr, or have the final step write a file itself.

> **In-session input preprocessing** via `[[pipe]]` in `.agents/guardrails.toml` runs before the model — see [Guardrails](18-guardrails.md#pipe--pre-model-input-transform). Workflows sit *above* sessions; pipes sit *inside* one.

## Concept

```
stdin ─► octomind workflow file.toml
                    │
                    ├── step "spec"      → octomind run (subprocess)
                    ├── step "developer" → octomind run (subprocess)  ─┐
                    └── step "tester"    → octomind run (subprocess)  ─┘  loop
                    │
                    ▼
        stderr: per-step responses + progress, cost, tokens, totals (human)
        stdout: empty by default · --format jsonl → per-step + cost events · --dry-run → plan
```

A workflow file is a portable TOML document — no edits to `default.toml` or any role config are needed. Each step invokes `octomind run --format jsonl`, streams the JSONL event log, accumulates assistant text and cost/token totals, then hands the captured output to the next step.

## CLI

```bash
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml

# Validate + print execution plan without spawning anything
octomind workflow myflow.toml --dry-run
```

- The file is read, TOML-parsed, and fully validated **before** anything else — including before stdin is touched. `--dry-run` therefore never reads stdin.
- stdin is required for a real run (not for `--dry-run`). Both a terminal stdin (nothing piped) and an empty piped stdin (empty after trimming) fail with the same error: `workflow requires input via stdin`.
- stderr receives each step's assistant message (rendered as markdown when `enable_markdown_rendering` is on), progress lines, per-step stats, warnings, and the final total — the human view. **stdout is empty by default**; pass `--format jsonl` for a machine-readable result on stdout (per-step `assistant` + final `cost` events — see [Machine-readable output](#machine-readable-output---format-jsonl)), or `--dry-run` to print the plan.

## File format

```toml
name        = "my-workflow"
description = "Optional human description"
max_cost    = 5.00              # optional USD cap for the whole run (abort if exceeded)

# ── Sequential step (the default) ─────────────────────────────────────
[[steps]]
name    = "spec"
role    = "developer:general"   # any installed role or tap-agent tag
prompt  = """
User request:
{{input}}

Write a tight implementation spec.
"""
session = "fresh"               # "fresh" (default) | "continue"
timeout = 0                     # seconds; 0 = no timeout (default)
retries = 0                     # extra attempts on failure (default 0)
# model = "anthropic:claude-sonnet-4-6"  # optional: override the role's model for this step
# skills = ["skill-a", "skill-b"]        # optional: force-load these skills (OCTOMIND_SKILLS)
# capabilities = ["cron", "docker"]      # optional: force-load these capabilities (OCTOMIND_CAPABILITIES)

# ── Parallel block — sub-steps run concurrently ───────────────────────
[[steps]]
name     = "review"
parallel = true

  [[steps.run]]
  name   = "security"
  role   = "security:owasp"
  prompt = "Security review of:\n{{spec}}"

  [[steps.run]]
  name   = "performance"
  role   = "developer:general"
  prompt = "Performance review of:\n{{spec}}"

# ── Loop block — generator/evaluator refine pattern ───────────────────
[[steps]]
name           = "refine"
loop           = true
max_iterations = 3                                       # default 10
exit_when      = { output = "tester", contains = "NO ISSUES" }

  [[steps.run]]
  name    = "developer"
  role    = "developer:general"
  session = "continue"            # see "Session modes" below
  prompt  = "Implement:\n{{spec}}"

  [[steps.run]]
  name    = "tester"
  role    = "developer:brief"
  session = "continue"
  prompt  = "Verify against spec:\n{{spec}}\n\nCode:\n{{developer}}"

# ── Conditional block — branch on a pattern match ─────────────────────
[[steps]]
name        = "route"
conditional = true
condition   = { output = "spec", contains = "security" }
on_match    = ["deep-dive"]
on_no_match = ["quick-summary"]

  [[steps.run]]
  name   = "deep-dive"
  role   = "security:owasp"
  prompt = "Deep analysis:\n{{spec}}"

  [[steps.run]]
  name   = "quick-summary"
  role   = "developer:general"
  prompt = "One-line summary:\n{{spec}}"

# ── Final step ────────────────────────────────────────────────────────
[[steps]]
name   = "evaluator"
role   = "developer:general"
prompt = """
Score 1-10:
{{developer}}

SCORE: <n>/10
"""
```

## Variable substitution

Every step prompt is resolved in **three passes**, in order, exactly like the interactive chat resolves user input:

**Pass 1 — workflow variables.** Anywhere in a prompt, `{{name}}` is substituted with:

| Variable           | Value                                                                  |
|--------------------|------------------------------------------------------------------------|
| `{{input}}`        | The raw stdin content (trimmed)                                        |
| `{{step_name}}`    | The full text output of a previously completed step (by name)          |
| `{{parallel_step}}`| A parallel **block's** name → every sub-step's output joined; an expanded sub-step's name → all its replica outputs joined (see [Parallel](#parallel-parallel--true)). In a **dynamic parallel block** (with `match`), the block's own name is the *loop variable* — inside the template it resolves to this branch's matched item; the accumulated output is read downstream via the **sub-step's** name (see [Dynamic fan-out](#dynamic-fan-out-match)). |

An unknown `{{var}}` is left **untouched** in this pass so the next pass can claim it as a built-in.

**Pass 2 — built-in placeholders.** The same canonical chat helper then expands these built-ins (no quotes, used bare in the prompt):

| Placeholder      | Expands to                                              |
|------------------|---------------------------------------------------------|
| `{{DATE}}`       | Current date/time                                       |
| `{{CWD}}`        | Project working directory                               |
| `{{SHELL}}`      | Detected shell                                          |
| `{{OS}}`         | Operating system                                        |
| `{{BINARIES}}`   | Available developer binaries on PATH                    |
| `{{ROLE}}`       | The resolved role name                                  |
| `{{SYSTEM}}`     | System info summary                                     |
| `{{CONTEXT}}`    | Project context bundle                                  |
| `{{GIT_STATUS}}` | `git status` of the working directory                   |
| `{{GIT_TREE}}`   | Git-tracked file tree                                   |
| `{{README}}`     | Project README contents                                 |

> Built-in placeholders are recognized by pre-flight validation (`src/workflow/validate.rs`) and pass through to this expansion pass. Only genuinely unknown `{{var}}` references — not `{{input}}`, a declared step name, or a built-in above — are rejected as *unknown variable* before the step runs.

**Pass 3 — context file inlining.** Any `<context>path</context>` or `<context>path:start:end</context>` block is replaced with the named file's contents rendered as XML (the same file-context path chat uses). Use `path:start:end` to inline only a line range. Because this runs on every step prompt, a step can also emit a `<context>...</context>` block in *its own* response and the next step that interpolates `{{that_step}}` will receive the file inlined.

Forward references (`{{later}}` from an earlier step) are rejected at pre-flight validation — which rejects **any** `{{var}}` that is not `{{input}}`, a built-in placeholder, or an already-defined step name. Step names must be unique across the entire file, including all sub-steps. `<context>` blocks use angle brackets rather than `{{ }}`, so they are not treated as variable references.

## Step types

### Sequential (default)
Runs `octomind run` once with the resolved prompt. No flag needed — any `[[steps]]` table without `parallel`/`loop`/`conditional = true` is sequential.

Optional fields on any sequential step (including sub-steps inside parallel/loop/conditional blocks):

| Field | Default | Description |
|-------|---------|-------------|
| `session` | `"fresh"` | Session reuse policy (see [Session modes](#session-modes)) |
| `timeout` | `0` | Seconds before the subprocess is killed; 0 = no timeout |
| `retries` | `0` | Extra attempts on non-zero exit or empty output |
| `model` | _(role default)_ | Override the model for this step; use `provider:model` format (e.g. `anthropic:claude-sonnet-4-6`). Forwarded as `--model` to the subprocess. Must not be empty when specified. |
| `skills` | _(none)_ | List of skill names to force-load in the subprocess before its first turn. Forwarded as `OCTOMIND_SKILLS` (comma-joined) — same env-loading mechanism an interactive session uses. |
| `capabilities` | _(none)_ | List of capability names to force-load in the subprocess before its first turn. Forwarded as `OCTOMIND_CAPABILITIES` (comma-joined) — same env-loading mechanism an interactive session uses. |

### Parallel (`parallel = true`)
Sub-steps run concurrently via `tokio::join_all`. The next top-level step starts only after every sub-step completes. Sub-steps cannot reference each other; only outer scope.

A `session = "continue"` field on a parallel sub-step is **silently ignored** — parallel sub-steps always run with a fresh session. Continue-session state only makes sense across the sequential iterations of a loop.

**Block fields** (on the `[[steps]]` table with `parallel = true`):

| Field | Default | Description |
|-------|---------|-------------|
| `min_success` | _(all)_ | Minimum replicas (counted across the whole block, after `count` expansion) that must succeed for the block to pass. Lets a fan-out tolerate a flaky branch. Out of range → pre-flight error. |
| `max_parallel` | _(unbounded)_ | Cap on how many replicas run concurrently (semaphore-throttled). Omit to launch all at once. Must be ≥ 1. |

**Different models / different prompts** are just plain named sub-steps — each carries its own `model` and `prompt`. There is no special "model sweep" field; copy a `[[steps.run]]` block per branch (names are unique, so each branch is referenceable). The only fan-out field is `count`, for repeating one identical sub-step:

| Field | Default | Description |
|-------|---------|-------------|
| `count` | _(1)_ | Run this sub-step N times **unchanged** — same `role`, `model`, and `prompt`. The model is non-deterministic, so the N runs differ; an aggregator then picks/merges the best (best-of-N sampling). Just shorthand for copy-pasting the same block N times. Must be ≥ 2. Valid **only** on a parallel sub-step; rejected elsewhere. |

```toml
[[steps]]
name        = "candidates"
parallel    = true
min_success = 2          # tolerate one failed branch
# max_parallel = 4       # optional concurrency cap

  # Same task on two different models → two named sub-steps.
  [[steps.run]]
  name   = "opus"
  role   = "developer:general"
  model  = "anthropic:claude-opus-4-8"
  prompt = "Solve:\n{{input}}"

  [[steps.run]]
  name   = "gpt"
  role   = "developer:general"
  model  = "openai:gpt-5"
  prompt = "Solve:\n{{input}}"

  # Best-of-3 with one model + prompt → use count instead of copy-pasting.
  [[steps.run]]
  name   = "sampler"
  role   = "developer:general"
  prompt = "Solve:\n{{input}}"
  count  = 3
```

**Aggregation variables.** After a parallel block completes, two kinds of `{{var}}` become available to later steps:

- `{{<sub-step-name>}}` — a sub-step with `count` resolves to **all its replica outputs joined** under `── <name> #N ──` headers. A plain sub-step resolves to its single raw output, exactly as before.
- `{{<parallel-step-name>}}` — resolves to **every sub-step's (aggregated) output joined**, so an aggregator can reference the whole block at once instead of listing each branch. (Previously this name validated but resolved to empty; it now carries the joined content.)

Failed replicas (under `min_success`) are skipped in both joins.

#### Dynamic fan-out (`match`)

Everything above is **static** — branches are fixed in the file. To fan out a
**runtime-determined** number of branches (e.g. a planner step emits a list, and
you want one branch per item), add a `match` regex to the parallel block. Its
presence flips the block to **dynamic** mode:

- `match` is a regex applied to the **previous step's output**. Each match is one branch.
- The block has **exactly one** sub-step — the per-item template.
- The block's own name is the **loop variable**. Inside the template, `{{<block-name>}}` resolves to *this branch's matched item* (one task). Each branch's output accumulates under the **sub-step's name**, so a later step reads `{{<sub-step-name>}}` to get *all branches joined*.
- Item text = **capture group 1** of the regex (the regex must define one — `{{...}}`-style content). Trimmed; empty matches dropped.
- Branch count is unknown until runtime, so concurrency / spend are bounded by the existing `max_parallel` and top-level `max_cost`; `min_success` is an absolute count.

```toml
[[steps]]
name   = "plan"
role   = "researcher:general"
prompt = "Break this into independent research tasks, each wrapped in <task>…</task>:\n{{input}}"

[[steps]]
name         = "research"
parallel     = true
match        = "(?s)<task>(.*?)</task>"   # one branch per <task> block
max_parallel = 4
min_success  = 1
  [[steps.run]]
  name   = "researcher"
  role   = "researcher:general"
  prompt = "Research this task thoroughly:\n{{research}}"     # {{research}} = THIS branch's one task

[[steps]]
name   = "summary"
role   = "developer:general"
prompt = "Synthesize all findings:\n\n{{researcher}}"         # {{researcher}} = every branch's output joined
```

`(?s)` lets a task body span lines; `(.*?)` is non-greedy so each `<task>…</task>` is its own item. The two names play distinct roles: `{{research}}` (the block) is the loop variable — one matched task per branch — while `{{researcher}}` (the sub-step) is every branch's output accumulated. Downstream steps read the sub-step name to get the joined result; the block name is scoped to the template only. A ready-to-run copy is at [`config-templates/workflow-research.toml`](../../config-templates/workflow-research.toml).

### Loop (`loop = true`)
Sub-steps run sequentially within each iteration. Between iterations, `exit_when` is checked against the named step's output:

- `exit_when = { output = "tester", contains = "NO ISSUES" }` — substring match
- `exit_when = { output = "tester", matches = "^PASS" }` — Rust regex match
- omit `output` to test the most recent step's output

If `max_iterations` is reached without exit, the loop exits with the last iteration's outputs and a warning to stderr (the workflow does **not** fail).

### Conditional (`conditional = true`)
`condition` tests a prior step output (same shape as `exit_when`). On match, the names in `on_match` run; otherwise `on_no_match` runs. Skipped sub-step names resolve to empty strings in later substitutions.

Omitting `output` in the `condition` tests the most recently completed step. If **no** step has completed yet (the conditional is the first step), the workflow fails with `conditional step '<name>': no prior step output to test`.

## Session modes

| Mode                          | Behaviour                                                              |
|-------------------------------|------------------------------------------------------------------------|
| `session = "fresh"` (default) | Brand-new session every invocation. No state persists.                 |
| `session = "continue"`        | First run: new session, ID is remembered. Subsequent runs (loop iter 2+, or retry): the same session is resumed; `/done` is sent first to compress prior context. The session is ephemeral to a single `octomind workflow` invocation. |

**Continue-session prompt rule:** on the *first* run of a continue-session, the templated prompt is sent. On *subsequent* runs, the templated prompt is **replaced** with the most recent prior step's raw output — the session already holds the full context, so it just needs the latest signal to react to. This is what makes the generator↔tester GAN pattern work without re-feeding the whole spec each iteration.

Each step owns its own session ID. In a loop, `developer` and `tester` accumulate independent histories. The generated session name has the form `wf-<sanitized-workflow-name>-<step-name>-<short-uuid>` (workflow name sanitized to ASCII alphanumerics and `-`; short-uuid is the first segment of a UUIDv4). These sessions are ephemeral to one `octomind workflow` invocation and are not reused across runs.

## Cost budget (`max_cost`)

Each step is a separate `octomind run` subprocess with its **own** session, so any per-request / per-session spending cap from your config resets on every step. A loop that runs 2 steps for 10 iterations can therefore spend up to ~20× a per-session cap. `max_cost` adds a single hard ceiling for the **entire** workflow:

```toml
name     = "gan"
max_cost = 2.50    # USD; abort the workflow once total spend exceeds this
```

- Optional top-level field. Omit it for no cap (default).
- Must be a positive number — validated pre-flight (`--dry-run` shows it in the plan).
- The check runs **after each step's cost is added to the running total**, so it stops spend *before the next step* — including between loop iterations and after a parallel block. The step that crossed the line still completes; the workflow then exits non-zero with:
  `workflow cost budget exceeded: spent $<x> exceeds max_cost $<cap> (stopped after step '<name>')`.
- This is the workflow-level analogue of the per-session spending thresholds (see [Cost as a Control Plane](../../README.md#pillar-3--cost-as-a-control-plane)).

## Retries and timeouts

- `retries = N` — up to N additional attempts on failure (default 0 ≙ one attempt).
- A step "fails" when the subprocess exits non-zero **or** produces no assistant output.
- `timeout = S` — seconds before the subprocess is killed (default 0 ≙ no timeout). A timeout counts as a failure for retry logic.
- All retries exhausted → workflow exits non-zero with `step '<name>' failed after <N> attempts: <reason>`, where `<reason>` is the last attempt's failure — e.g. `failed exit code Some(1) (attempt N/N)`, `timed out after Ss (attempt N/N)`, `produced no assistant output (attempt N/N)`, or `spawn error: ...`.

## Progress output (stderr)

All progress goes to **stderr**. The exact rendering depends on whether stderr is a terminal:

- **Interactive (stderr is a TTY):** each step opens a `╭ <name>` box and, while it runs, a live spinner shows the latest stream event plus a dimmed running aggregate (elapsed · cost · ⚒tools). When the step finishes the spinner clears and the box closes with `╰ ✓ <name>  …stats`.
- **Piped / redirected:** no spinner — each JSONL event is streamed as one line under a `│ ` rail. The events surfaced are `ToolUse` (`▸ tool · server` plus params), `Skill`, `Status`, `McpNotification`, and `Error`. Assistant text, thinking, and cost events are not rendered as rail lines; failed tool calls are surfaced separately via the `⚒N ✗F` count in the per-step and total stats.

A complete run looks like this (color stripped):

```
workflow · my-workflow

╭ spec
│ ▸ shell · octofs
╰ ✓ spec  2.1s  · $0.0042  · 1240 tok  · ⚒3

╭ developer  [1/3] refine
╰ ✓ developer  8.4s  · $0.0156  · 3208 tok  · ⚒12

╭ tester  [1/3] refine
╰ ✓ tester  3.2s  · $0.0078  · 1450 tok  · ⚒2
· loop 'refine' exit at iteration 1

╭ evaluator
╰ ✓ evaluator  1.8s  · $0.0029  · 890 tok  · ⚒0

total · 15.5s  · $0.0305  · 6788 tok  · ⚒17
```

- The header is `workflow · <name>` and the footer is `total · <dur>  · $<cost>  · <tok> tok  · ⚒<tools>`.
- Inside a loop, the box title carries a `[i/max] <loop-name>` suffix.
- A failed attempt closes with `╰ ✗ <name>  <reason>` instead of `╰ ✓ …`.
- The `⚒N` glyph is the tool-call count; on failures it becomes `⚒N ✗F` (F = failed tool calls).

**Where the numbers come from.** Stats are sourced from the JSONL stream emitted by `octomind run --format jsonl`: cost, token totals, and per-event tool tracking. Per-step `cost`, `input_tokens`, and `output_tokens` come from the `cost` event's payload, and the **token total shown is `session_tokens`** (the session-wide total reported by the run), *not* `input + output`. Tool counts are tallied live: `⚒N` increments on each `ToolUse` event and `✗F` increments on each failed `ToolResult`. Duration is wall-clock time of the subprocess. The footer sums duration, cost, tokens, and tool counts across every step.

> **Continue-session steps report per-invocation deltas.** A `session = "continue"` step's subprocess reports *cumulative* session cost/tokens every time it resumes (each loop iteration or retry). The orchestrator subtracts the per-step running baseline so the per-step line, the footer total, and `max_cost` each count a turn's spend exactly once — without this, an N-iteration refine loop would over-count cost ~N× (compounding). Fresh and parallel steps are a new session each invocation and are reported as-is.

## Machine-readable output (`--format jsonl`)

A plain run writes nothing to stdout — it is meant to be watched on stderr. To consume a workflow's result programmatically, pass `--format jsonl`:

```bash
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml --format jsonl
```

stdout then carries newline-delimited JSON:

- One `assistant` event **per step**, emitted as that step completes: `{"type":"assistant","content":"…","step":"<step-name>","session_id":""}`. The **last** `assistant` event is the workflow's final result. In a parallel block, one event is emitted per sub-step (keyed by sub-step name) carrying that sub-step's accumulated output; the block-level aggregate and a dynamic `match` block's loop variable are not emitted.
- A single trailing `cost` event with the aggregated totals (`session_tokens`, `session_cost`, and the input/output/cache/reasoning token breakdown). Its `session_id` is empty — a workflow has no single resumable session.

Per-step progress still goes to stderr in both modes. Only `jsonl` produces stdout output; any other `--format` value (or omitting it) leaves stdout empty.

## --dry-run

`octomind workflow file.toml --dry-run` validates the file, resolves the execution graph, and prints the plan to **stdout**. (That plan is the only stdout a *default* run produces; `--format jsonl` additionally streams per-step `assistant` + `cost` events — see above.) It spawns no `octomind run` processes and never reads stdin (validation runs before the stdin step, and `--dry-run` returns immediately after). Use it to sanity-check a workflow before paying for tokens.

## Validation

Pre-flight checks (all hard-fail before any step runs):

- File exists, valid TOML.
- Step names unique across the whole file.
- `'input'` is reserved (you can't name a step `input`).
- Every `{{var}}` references either `input`, a built-in placeholder (`{{DATE}}`, `{{CWD}}`, `{{CONTEXT}}`, `{{GIT_STATUS}}`, …), or a step that completes before the referencing step.
- A `parallel` step has at least 2 sub-steps; `loop` has ≥1 sub-step + `exit_when`; `conditional` has `condition` and at least one of `on_match` / `on_no_match`.
- Regex patterns in `matches` compile.
- `model`, when specified on any step, must not be an empty string.
- `max_cost`, when set, is a positive finite number.
- `count` appears only on parallel sub-steps and is ≥ 2.
- `min_success`, when set, is between 1 and the block's total replica count; `max_parallel`, when set, is ≥ 1.
- A parallel block with `match` (dynamic): the regex compiles, it has **exactly one** sub-step, is **not** the first step, and its template does not use `count`. `min_success` (when set) is ≥ 1.

## End-to-end example

A generator/tester GAN that builds, reviews, and scores:

```toml
name   = "gan"

[[steps]]
name   = "spec"
role   = "developer:general"
prompt = "User request:\n{{input}}\n\nWrite an implementation spec."

[[steps]]
name           = "refine"
loop           = true
max_iterations = 3
exit_when      = { output = "tester", contains = "NO ISSUES" }

  [[steps.run]]
  name    = "developer"
  role    = "developer:general"
  session = "continue"
  prompt  = "Implement:\n{{spec}}"

  [[steps.run]]
  name    = "tester"
  role    = "developer:brief"
  session = "continue"
  prompt  = "Verify against spec:\n{{spec}}\n\nImplementation:\n{{developer}}"

[[steps]]
name   = "evaluator"
role   = "developer:general"
prompt = """
Score this 1-10:
Spec: {{spec}}
Code: {{developer}}
Verdict: {{tester}}

SCORE: <n>/10
VERDICT: <PASS|FAIL>
"""
```

Run it:

```bash
echo "JSON-to-CSV CLI in Rust" | octomind workflow gan.toml
```

### Fan-out → aggregate (across models)

Run the same task on three models in parallel, tolerate one failure, then have an
aggregator pick and synthesize the best answer. Each branch is a plain named
sub-step with its own `model`. A ready-to-run copy lives at
[`config-templates/workflow-fanout.toml`](../../config-templates/workflow-fanout.toml).

```toml
name        = "fan-out-aggregate"
description = "Same task on three models in parallel, one judge synthesizes"

[[steps]]
name        = "candidates"
parallel    = true
min_success = 2                     # one model may fail; two is enough

  [[steps.run]]
  name   = "opus"
  role   = "developer:general"
  model  = "anthropic:claude-opus-4-8"
  prompt = "Solve this. Be complete and correct:\n{{input}}"

  [[steps.run]]
  name   = "gpt"
  role   = "developer:general"
  model  = "openai:gpt-5"
  prompt = "Solve this. Be complete and correct:\n{{input}}"

  [[steps.run]]
  name   = "gemini"
  role   = "developer:general"
  model  = "google:gemini-3-pro"
  prompt = "Solve this. Be complete and correct:\n{{input}}"

[[steps]]
name   = "judge"
role   = "developer:general"
prompt = """
Independent solutions to the same task, one per model:

{{candidates}}

Pick the strongest, fix any flaws, and produce one final answer.
"""
```

`{{candidates}}` (the block name) expands to all three branch outputs joined under
`── opus ──`, `── gpt ──`, `── gemini ──` headers; or reference each branch directly
as `{{opus}}` / `{{gpt}}` / `{{gemini}}`. Run it:

```bash
echo "JSON-to-CSV CLI in Rust" | octomind workflow config-templates/workflow-fanout.toml
```

## Best practices

1. **Keep prompts focused.** Each step is its own session — don't try to cram a multi-stage task into one step.
2. **Use `session = "continue"` for refine loops.** The auto-replacement of the prompt with the prior step's output is the whole point of the GAN pattern.
3. **Always set `max_iterations`** on loops to bound spend.
4. **Set `timeout`** when a step might hang on an external dependency.
5. **`--dry-run` before every change** to catch unresolved variables and typos.
6. **Pick cheap models for utility steps** (briefs, classifiers) by setting `model` on individual steps in the workflow file; reserve expensive models for the main work.
7. **Watch the totals.** Stats are right there on stderr — if a workflow runs hot, the per-step breakdown shows exactly where.

## Out of scope

Intentionally not supported (use shell composition or call `octomind run` directly):

- `--var key=value` CLI variable injection (stdin is the only input)
- Workflow definitions inside `default.toml` (external file only)
- Named workflow lookup by short name (explicit path only)
- Cross-invocation session persistence for `continue` sessions
- Step artifacts written to disk
