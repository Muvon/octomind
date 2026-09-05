# Workflows

Use workflows to run repeatable multi-step tasks from local TOML files or installed taps. This guide covers sequential,
parallel, loop, conditional, and graph execution for CLI automation.

## Get started

Save this as `myflow.toml`. The `assistant` role is included in the default config:

```toml
name = "summarize"

[[steps]]
name = "summary"
role = "assistant"
prompt = "Summarize this in one sentence:\n{{input}}"
```

```bash
octomind workflow myflow.toml --dry-run
printf '%s\n' 'The deployment succeeded. Two checks remain.' | octomind workflow myflow.toml --format jsonl
```

Real runs show progress and responses on stderr. For structured stdout, use `--format jsonl`; without it, stdout is
empty. `--dry-run` prints a plan without reading stdin or spawning steps.

For input preprocessing inside a session, see [Guardrails](18-guardrails.md#pipe--pre-model-input-transform).

## Concept

```text
stdin ─► octomind workflow NAME|file.toml
                    │
                    ├── step "spec"      → octomind run (subprocess)
                    ├── step "developer" → octomind run (subprocess)  ─┐
                    └── step "tester"    → octomind run (subprocess)  ─┘  loop
                    │
                    ▼
        stderr: per-step responses + progress, cost, tokens, totals (human)
        stdout: empty by default · --format jsonl → per-step + cost events · --dry-run → plan
```

A workflow file is a portable TOML document — no workflow definitions are added to the main config. Its roles and required tools must already resolve. Each step
invokes `octomind run --format jsonl`, streams the JSONL event log, accumulates assistant text and cost/token totals,
then hands the captured output to the next step.

## Choose a workflow

```bash
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml

# If research appears in the list, run that installed tap workflow
echo "research this topic" | octomind workflow research

# List public tap workflows
octomind workflow

# Validate + print execution plan without spawning anything
octomind workflow myflow.toml --dry-run
```

- The selected local file or fetched tap workflow is TOML-parsed and validated before stdin is read. `--dry-run`
  therefore never reads stdin. Tap workflows are additionally restricted to public tap roles; local files are not.
- stdin is required for a real run (not for `--dry-run`). Both a terminal stdin (nothing piped) and an empty piped stdin
  (empty after trimming) fail with the same error: `workflow requires input via stdin`.
- stderr receives each step's assistant message (rendered as markdown when `enable_markdown_rendering` is on), progress
  lines, per-step stats, warnings, and the final total — the human view. **stdout is empty by default**; pass `--format
  jsonl` for a machine-readable result on stdout (per-step `assistant` + final `cost` events — see [Machine-readable
  output](#machine-readable-output---format-jsonl)), or `--dry-run` to print the plan.

## File format

Save the following complete example as `my-workflow.toml`. Use installed role tags; the examples use
`developer:general`, or you can substitute a local `[[roles]]` name. Model names shown are explicit overrides, not
shipped defaults; they must be available through your configured provider.

```toml
name        = "my-workflow"
description = "Specify, implement, review, and score a requested change"

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
# model = "anthropic:claude-sonnet-4-6"   # optional main-purpose name override
# skills = ["code-review"]              # use an exact installed skill name
# capabilities = ["cron", "docker"]      # exact capability names, forwarded through OCTOMIND_CAPABILITIES

# ── Parallel block — sub-steps run concurrently ───────────────────────
[[steps]]
name     = "review"
parallel = true

  [[steps.run]]
  name   = "security"
  role   = "developer:general"
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
  role    = "developer:general"
  session = "continue"
  prompt  = "Verify against spec:\n{{spec}}\n\nCode:\n{{developer}}\nReply NO ISSUES only when verification passes."

# ── Conditional block — branch on a pattern match ─────────────────────
[[steps]]
name        = "route"
conditional = true
condition   = { output = "spec", contains = "security" }
on_match    = ["deep-dive"]
on_no_match = ["quick-summary"]

  [[steps.run]]
  name   = "deep-dive"
  role   = "developer:general"
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

Run that file after saving it:

```bash
octomind workflow my-workflow.toml --dry-run
printf '%s\n' 'Build a JSON-to-CSV CLI in Rust' | octomind workflow my-workflow.toml
```

## Graph routing

Ordered workflows remain the default. To connect the same step types as a bounded control-flow graph, declare `entry`,
`max_transitions`, and ordered `[[edges]]`:

```toml
name            = "review-cycle"
entry           = "implement"
max_transitions = 12

[[steps]]
name   = "implement"
role   = "developer:general"
prompt = "Implement:\n{{input}}"

[[steps]]
name    = "review"
role    = "developer:general"
session = "continue"
prompt  = "Review the implementation. Reply PASS when complete."

[[steps]]
name    = "fix"
role    = "developer:general"
session = "continue"
prompt  = "Apply this review:\n{{review}}"

[[edges]]
from = "implement"
to   = "review"

[[edges]]
from = "review"
to   = "$end"
when = { contains = "PASS" }

[[edges]]
from = "review"
to   = "fix"       # required unconditional route, declared last

[[edges]]
from = "fix"
to   = "review"
```

Edges from a node are tested in declaration order. A conditional edge uses the completed node's canonical output unless
`when.output` names another available output. Every node must finish with exactly one unconditional edge; `$end` is the
reserved terminal target. Cycles are allowed, while `max_transitions` strictly bounds total node executions.

All existing step kinds are graph nodes, so composition does not require recursive syntax: a parallel block can route
into a loop, a conditional block, another parallel block, or back to an earlier node. If neither `entry` nor `[[edges]]`
is present, declaration-order behavior is unchanged. A ready-to-run parallel-review/fix cycle is available at
[`config-templates/workflow-graph.toml`](../../config-templates/workflow-graph.toml).

## Variable substitution

Every step prompt is resolved in **three passes**; the last two reuse the chat helpers:

**Pass 1 — workflow variables.** Anywhere in a prompt, `{{name}}` is substituted with:

| Variable           | Value                                                                  |
|--------------------|------------------------------------------------------------------------|
| `{{input}}`        | The raw stdin content (trimmed)                                        |
| `{{step_name}}`    | The full text output of a previously completed step (by name)          |
| `{{parallel_step}}`| A parallel **block's** name → every sub-step's output joined; an expanded sub-step's name → all its replica outputs joined (see [Parallel](#parallel-parallel--true)). In a **dynamic parallel block** (with `match`), the block's name is the per-item loop variable inside the template and becomes the joined block output after fan-out completes (see [Dynamic fan-out](#dynamic-fan-out-match)). |

An unknown `{{var}}` is left **untouched** in this pass so the next pass can claim it as a built-in.

**Pass 2 — built-in placeholders.** The same canonical chat helper then expands these built-ins (no quotes, used bare in
the prompt):

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

**Pass 3 — context file inlining.** Any `<context>path</context>` or `<context>path:start:end</context>` block is
replaced with the named file's contents rendered as XML (the same file-context path chat uses). Use `path:start:end` to
inline only a line range. Because this runs on every step prompt, a step can also emit a `<context>src/main.rs</context>` block
in *its own* response and the next step that interpolates `{{that_step}}` will receive the file inlined.

In ordered workflows, forward references (`{{later}}` from an earlier step) are rejected before execution. Graph
workflows permit declared outputs in any file order, but fail at runtime if the selected route has not produced them.
Step names must be unique across the entire file, including all sub-steps. `<context>` blocks use angle brackets rather
than `{{ }}`, so they are not treated as variable references.

For example, add this sequential step after `spec` to include both its output and a source file:

```toml
[[steps]]
name = "source-review"
role = "developer:general"
prompt = "Review in {{CWD}}:\n{{spec}}\n<context>src/main.rs:1:40</context>"
```

## Step types

### Sequential (default)

Runs `octomind run` once with the resolved prompt. No flag needed — any `[[steps]]` table without
`parallel`/`loop`/`conditional = true` is sequential.

Optional fields on any sequential step (including sub-steps inside parallel/loop/conditional blocks):

| Field | Default | Description |
|-------|---------|-------------|
| `session` | `"fresh"` | Session reuse policy (see [Session modes](#session-modes)) |
| `timeout` | `0` | Seconds before the subprocess is killed; 0 = no timeout |
| `retries` | `0` | Extra attempts on non-zero exit or empty output |
| `model` | _(role default)_ | Override the main-purpose model for this step; use `provider:model` format (e.g. `anthropic:claude-sonnet-4-6`). Forwarded as `--model` to the subprocess. Must not be empty when specified. |
| `workdir` | _(orchestrator cwd)_ | Child working directory; relative paths resolve against the orchestrator cwd. Must exist and be a directory at execution time. |
| `skills` | _(inherited environment)_ | List of skill names to force-load in the subprocess before its first turn. Forwarded as `OCTOMIND_SKILLS` (comma-joined) — same env-loading mechanism an interactive session uses. |
| `capabilities` | _(inherited environment)_ | Exact installed capability names to force-load before the first turn. Forwarded as `OCTOMIND_CAPABILITIES` (comma-joined); no aliases or fuzzy matching are applied. |

### Parallel (`parallel = true`)

Sub-steps run concurrently and are joined before the block completes. The next top-level step starts only after every
sub-step completes. Sub-steps cannot reference each other; only outer scope.

A `session = "continue"` field on a parallel sub-step is **silently ignored** — parallel sub-steps always run with a
fresh session. Continue-session reuse applies to sequential execution, including loop iterations and repeated graph
visits.

**Block fields** (on the `[[steps]]` table with `parallel = true`):

| Field | Default | Description |
|-------|---------|-------------|
| `min_success` | _(all)_ | Minimum replicas (counted across the whole block, after `count` expansion) that must succeed for the block to pass. Lets a fan-out tolerate a flaky branch. Out of range → pre-flight error. |
| `max_parallel` | _(unbounded)_ | Cap on how many replicas run concurrently (semaphore-throttled). Omit to launch all at once. Must be ≥ 1. |

**Different models / different prompts** are just plain named sub-steps — each carries its own `model` and `prompt`.
There is no special "model sweep" field; copy a `[[steps.run]]` block per branch (names are unique, so each branch is
referenceable). The only fan-out field is `count`, for repeating one identical sub-step:

| Field | Default | Description |
|-------|---------|-------------|
| `count` | _(1)_ | Run this sub-step N times **unchanged** — same `role`, `model`, and `prompt`. Sampling can produce different outputs; an aggregator then picks/merges the best (best-of-N sampling). Just shorthand for copy-pasting the same block N times. Must be ≥ 2. Valid **only** on a parallel sub-step; rejected elsewhere. |

```toml
name = "parallel-candidates"

[[steps]]
name        = "candidates"
parallel    = true
min_success = 2          # two successes out of five replicas are enough
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

- `{{<sub-step-name>}}` — a sub-step with `count` resolves to **all its replica outputs joined** under `── <name> #N ──`
  headers. A plain sub-step resolves to its single raw output, exactly as before.
- `{{<parallel-step-name>}}` — resolves to **every sub-step's (aggregated) output joined**, so an aggregator can
  reference the whole block at once instead of listing each branch.

Failed replicas (under `min_success`) are skipped in both joins.

### Dynamic fan-out (`match`)

Everything above is **static** — branches are fixed in the file. To fan out a **runtime-determined** number of branches
(e.g. a planner step emits a list, and you want one branch per item), add a `match` regex to the parallel block. Its
presence flips the block to **dynamic** mode:

- `match` is a regex applied to the explicitly named `source` output. Each match is one branch.
- The block has **exactly one** sub-step — the per-item template.
- The block's own name is the **loop variable**. Inside the template, `{{<block-name>}}` resolves to *this branch's
  matched item* (one task). Each branch's output accumulates under the **sub-step's name**, so a later step reads
  `{{<sub-step-name>}}` to get *all branches joined*.
- Item text = **capture group 1** of the regex (the regex must define one). Trimmed; empty matches dropped.
- Branch count is unknown until runtime. `max_parallel` bounds concurrency; `max_cost` is checked after the block, so it
  cannot cap in-flight fan-out spend; `min_success` is an absolute count.

```toml
name = "dynamic-research"

[[steps]]
name   = "plan"
role   = "developer:general"
prompt = "Break this into independent research tasks, each wrapped in <task>…</task>:\n{{input}}"

[[steps]]
name         = "research"
parallel     = true
source       = "plan"
match        = "(?s)<task>(.*?)</task>"   # one branch per <task> block
max_parallel = 4
min_success  = 1
  [[steps.run]]
  name   = "researcher"
  role   = "developer:general"
  prompt = "Research this task thoroughly:\n{{research}}"     # {{research}} = THIS branch's one task

[[steps]]
name   = "summary"
role   = "developer:general"
prompt = "Synthesize all findings:\n\n{{researcher}}"         # {{researcher}} = every branch's output joined
```

`(?s)` lets a task body span lines; `(.*?)` is non-greedy so each `<task>…</task>` is its own item. The two names play
distinct roles during fan-out: `{{research}}` (the block) is the loop variable — one matched task per branch — while
`{{researcher}}` (the sub-step) accumulates every branch's output. After the block completes, both `{{researcher}}` and
the canonical block output `{{research}}` contain the joined result. A ready-to-run copy is at
[`config-templates/workflow-research.toml`](../../config-templates/workflow-research.toml).

### Loop (`loop = true`)

Sub-steps run sequentially within each iteration. Between iterations, `exit_when` is checked against the named step's
output:

- `exit_when = { output = "tester", contains = "NO ISSUES" }` — substring match
- `exit_when = { output = "tester", matches = "^PASS" }` — Rust regex match
- omit `output` to test the most recent step's output

If `max_iterations` is reached without exit, the loop exits with the last iteration's outputs and a warning to stderr
(the workflow does **not** fail).

### Conditional (`conditional = true`)

`condition` tests a prior step output (same shape as `exit_when`). On match, the names in `on_match` run; otherwise
`on_no_match` runs. Skipped sub-step names resolve to empty strings in later substitutions.

Omitting `output` in the `condition` tests the most recently completed step. If **no** step has completed yet (the
conditional is the first step), the workflow fails with `conditional step '<name>': no prior step output to test`.

## Session modes

| Mode                          | Behaviour                                                              |
|-------------------------------|------------------------------------------------------------------------|
| `session = "fresh"` (default) | New session every invocation; earlier session history is not reused. Session files and tool side effects can persist.                 |
| `session = "continue"`        | First run: new session, ID is remembered. Subsequent runs (loop iter 2+, or retry): the same session name is reused; after a successful prior run, a best-effort `/done` is sent before resuming. Automatic reuse is limited to this workflow invocation. |

**Continue-session prompt rule:** on the *first* run of a continue-session, the templated prompt is sent. After a
successful prior invocation, on subsequent runs, the templated prompt is **replaced** with the most recent prior step's
raw output — the session already holds the full context, so it just needs the latest signal to react to. This is what
makes the generator↔tester GAN pattern work without re-feeding the whole spec each iteration.

Each step owns its own session ID. In a loop, `developer` and `tester` accumulate independent histories. The generated
session name has the form `wf-<sanitized-workflow-name>-<step-name>-<short-uuid>` (workflow name sanitized to ASCII
alphanumerics and `-`; short-uuid is the first segment of a UUIDv4). The workflow remembers these names only for this
invocation and does not reuse them in a later workflow run. This does not delete their persisted session files or undo
filesystem changes.

## Cost budget (`max_cost`)

Fresh steps start separate sessions; continued steps retain their own cumulative session totals. `max_cost` adds a
workflow-wide stop check against accounted spend after successful sequential steps and after parallel blocks. It does
not interrupt in-flight work, and retries, timeouts, or failed parallel replicas can leave spend outside the reported
total. Treat it as a stop threshold, not a strict provider billing limit.

Set a positive USD amount before any `[[steps]]` table, for example:

```toml
name = "budgeted-summary"
max_cost = 1.0

[[steps]]
name = "summary"
role = "assistant"
prompt = "Summarize:\n{{input}}"
```

Omitting `max_cost` leaves the workflow uncapped. An exceeded budget exits non-zero after completed work; `--dry-run`
displays the configured amount. See [Configuration reference](../reference/03-config-reference.md) for session spending
fields.

## Retries and timeouts

- `retries = N` — up to N additional attempts on failure (default 0 ≙ one attempt).
- A step "fails" when the subprocess exits non-zero **or** produces no assistant output.
- `timeout = S` — seconds before the subprocess is killed (default 0 ≙ no timeout). A timeout counts as a failure for
  retry logic.
- All retries exhausted → workflow exits non-zero with `step '<name>' failed after <N> attempts: <reason>`, where
  `<reason>` is the last attempt's failure — e.g. `failed exit code Some(1) (attempt N/N)`, `timed out after Ss (attempt
  N/N)`, `produced no assistant output (attempt N/N)`, or a spawn error naming the failed executable.

To bound each attempt, set these fields on a sequential step before the next table (replace existing values):

```toml
timeout = 300
retries = 1
workdir = "."
```

Prompt placeholders and `<context>` expansion run in the orchestrator before the child starts; setting `workdir` changes
the child process directory, not the base directory used for that initial expansion.

## End-to-end example

Save this generator/tester loop as `gan.toml`; it builds, reviews, and scores:

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
  role    = "developer:general"
  session = "continue"
  prompt  = "Verify against spec:\n{{spec}}\n\nImplementation:\n{{developer}}\nReply NO ISSUES only when verification passes."

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

Save this as `fanout.toml` to run the same task on three models in parallel, tolerate one failure, then have an
aggregator pick and synthesize the best answer. Each branch is a plain named sub-step with its own `model`. A
ready-to-run copy lives at [`config-templates/workflow-fanout.toml`](../../config-templates/workflow-fanout.toml).

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

`{{candidates}}` (the block name) expands to all three branch outputs joined under `── opus ──`, `── gpt ──`, `── gemini
──` headers; or reference each branch directly as `{{opus}}` / `{{gpt}}` / `{{gemini}}`. Run it:

```bash
echo "JSON-to-CSV CLI in Rust" | octomind workflow fanout.toml
```

## Progress output (stderr)

All progress goes to **stderr**. The exact rendering depends on whether stderr is a terminal:

- **Interactive (stderr is a TTY):** each step opens a `╭ <name>` box and, while it runs, a live spinner shows the
  latest stream event plus a dimmed running aggregate (elapsed · cost · ⚒tools). When the step finishes the spinner
  clears and the box closes with `╰ ✓ <name>  …stats`.
- **Piped / redirected:** no spinner — each JSONL event is streamed as one line under a `│ ` rail. The events surfaced
  are `ToolUse` (`▸ tool · server` plus params), `Skill`, `Status`, `McpNotification`, and `Error`. Assistant text,
  thinking, and cost events are not rendered as rail lines; failed tool calls are surfaced separately via the `⚒N ✗F`
  count in the per-step and total stats.

An abridged run looks like this with color and the accounting column omitted:

```text
workflow · my-workflow

╭ spec
│ ▸ shell · octofs
╰ ✓ spec  2.1s  · 1240 tok  · ⚒3

╭ developer  [1/3] refine
╰ ✓ developer  8.4s  · 3208 tok  · ⚒12

╭ tester  [1/3] refine
╰ ✓ tester  3.2s  · 1450 tok  · ⚒2
· loop 'refine' exit at iteration 1

╭ evaluator
╰ ✓ evaluator  1.8s  · 890 tok  · ⚒0

total · 15.5s  · 6788 tok  · ⚒17
```

- The header is `workflow · <name>`; the actual footer includes duration, aggregate accounting, tokens, and tool counts.
- Inside a loop, the box title carries a `[i/max] <loop-name>` suffix.
- A failed attempt closes with `╰ ✗ <name>  <reason>` instead of `╰ ✓ …`.
- The `⚒N` glyph is the tool-call count; on failures it becomes `⚒N ✗F` (F = failed tool calls).

**Where the numbers come from.** Stats are sourced from the JSONL stream emitted by `octomind run --format jsonl`: cost,
token totals, and per-event tool tracking. Per-step `cost`, `input_tokens`, and `output_tokens` come from the `cost`
event's payload, and the **token total shown is `session_tokens`** (the session-wide total reported by the run), *not*
`input + output`. Tool counts are tallied live: `⚒N` increments on each `ToolUse` event and `✗F` increments on each
failed `ToolResult`. Duration is wall-clock time of the subprocess. The footer sums duration, cost, tokens, and tool
counts across every step.

> **Continue-session steps report per-invocation deltas.** A `session = "continue"` step's subprocess reports *cumulative* session cost/tokens every time it resumes (each loop iteration or retry). The orchestrator subtracts the per-step running baseline so the per-step line, the footer total, and `max_cost` avoid re-counting previously reported session spend — without this, an N-iteration refine loop would over-count cost ~N× (compounding). Fresh and parallel steps are a new session each invocation and are reported as-is.

## Machine-readable output (`--format jsonl`)

A plain run writes nothing to stdout — it is meant to be watched on stderr. To consume a workflow's result
programmatically, pass `--format jsonl`:

```bash
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml --format jsonl
```

stdout then carries newline-delimited JSON:

- Sequential steps emit one `assistant` event when they complete:
  `{"type":"assistant","content":"…","step":"<step-name>","session_id":""}`. For a sequential final step, its
  `assistant` event is the final result. If execution ends on a parallel block, the last event contains only the last
  sub-step output; add a sequential aggregator to emit one combined result. After all parallel branches join, one event
  is emitted per sub-step (keyed by sub-step name) carrying that sub-step's accumulated output; the block-level
  aggregate and a dynamic `match` block's loop variable are not emitted.
- On successful workflow completion, a single trailing `cost` event with the aggregated totals (`session_tokens`,
  `session_cost`, and the input/output/cache/reasoning token breakdown). Its `session_id` is empty — a workflow has no
  single resumable session.

Per-step progress still goes to stderr in both modes. For a real execution, only `jsonl` produces result events on
stdout. Other `--format` strings are accepted but use the plain workflow path.

## Check the Execution Plan

`octomind workflow myflow.toml --dry-run` validates the file, resolves the execution graph, and prints the plan to
**stdout**. (That plan is the only stdout a *default* run produces; `--format jsonl` additionally streams per-step
`assistant` + `cost` events — see above.) It spawns no `octomind run` processes and never reads stdin (validation runs
before the stdin step, and `--dry-run` returns immediately after). Use it to sanity-check a workflow before paying for
tokens.

```bash
octomind workflow myflow.toml --dry-run
```

## Best practices

1. **Keep prompts focused.** Each step is its own session — don't try to cram a multi-stage task into one step.
2. **Use `session = "continue"` for refine loops.** The auto-replacement of the prompt with the prior step's output is
   the whole point of the GAN pattern.
3. **Set `max_iterations`** to bound loop iterations; use `max_cost` and per-step timeouts as additional controls.
4. **Set `timeout`** when a step might hang on an external dependency.
5. **`--dry-run` before every change** to catch unresolved variables and typos.
6. **Choose step models deliberately** by setting `model` only where a workflow step should override its resolved role.
7. **Watch the totals.** Stats are right there on stderr — if a workflow runs hot, the per-step breakdown shows exactly
   where.
8. **Keep graph nodes top-level.** Compose existing parallel, loop, and conditional blocks with edges instead of deeply
   nesting syntax.

## Out of scope

Intentionally not supported (use shell composition or call `octomind run` directly):

- `--var key=value` CLI variable injection (stdin is the only input)
- Workflow definitions inside `default.toml` (external file only)
- Automatic cross-invocation reuse of workflow session names
- A dedicated workflow artifact field (a step can still write files through its tools)

## Common questions

**Why is stdout empty?** Use `--format jsonl`. To capture both channels separately:

```bash
printf '%s\n' 'Summarize the deployment status' | octomind workflow myflow.toml --format jsonl > result.jsonl 2> progress.log
```

**Why does a variable fail in a graph?** The producer must execute on the selected route before the consumer. File order
alone does not establish availability in graph mode.

**Why did the loop stop without success?** Reaching `max_iterations` emits a warning and keeps the latest outputs. Make
the exit marker explicit in the evaluator's prompt and inspect its verdict.

**Why did dry-run pass but execution fail?** Dry-run checks structure; it does not execute roles, check credentials, or
verify that a step's `workdir` exists. It may fetch a named tap workflow to resolve it.

## Validation

Pre-flight checks (all hard-fail before any step runs):

- File exists, valid TOML.
- Step names unique across the whole file.
- `'input'` is reserved (you can't name a step `input`).
- Every `{{var}}` references `input`, a built-in placeholder (`{{DATE}}`, `{{CWD}}`, `{{CONTEXT}}`, `{{GIT_STATUS}}`,
  …), or a declared output. Ordered workflows require the producer earlier in declaration order; graph workflows fail at
  runtime if the selected route reaches a consumer before that producer has run.
- A static `parallel` step has at least 2 sub-steps; `loop` has ≥1 sub-step + `exit_when`; `conditional` has `condition`
  and at least one of `on_match` / `on_no_match`.
- Regex patterns in `matches` compile.
- `model`, when specified on any step, must not be an empty string.
- `max_cost`, when set, is a positive finite number.
- Graph mode requires `entry`, `max_transitions >= 1`, and at least one edge. Every edge target must exist or be `$end`;
  every node must be reachable and have exactly one last, unconditional route; at least one route to `$end` must be
  reachable.
- `count` appears only on parallel sub-steps and is ≥ 2.
- `min_success`, when set, is between 1 and the block's total replica count; `max_parallel`, when set, is ≥ 1.
- A parallel block with `match` (dynamic): `source` names an available output, the regex compiles, it has **exactly
  one** sub-step, and its template does not use `count`. `min_success` (when set) is ≥ 1.

## Source Reference

- [CLI](../../src/commands/workflow.rs)
- [Workflow TOML fields](../../src/workflow/schema.rs)
- [Validation](../../src/workflow/validate.rs)
- [Routing, sessions, and accounting](../../src/workflow/run.rs)
- [Child processes and environment](../../src/workflow/proc.rs)

## See also

- [Commands and layers](10-commands-and-layers.md)
- [Structured output](11-structured-output.md)
- [Guardrails](18-guardrails.md)
- [Tap system](../integration/04-tap-system.md)
