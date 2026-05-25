# Workflows

`octomind workflow <file.toml>` is an external orchestrator that chains multiple `octomind run` invocations into a multi-step pipeline. Each step is an independent subprocess; outputs flow between steps by name; the final result goes to stdout — making workflows composable with shell pipes.

> **In-session pipelines** for deterministic pre-processing live separately — see [Pipelines](14-pipelines.md). Workflows sit *above* sessions; pipelines sit *inside* one.

## Concept

```
stdin ─► octomind workflow file.toml ─► stdout (final step output)
                    │
                    ├── step "spec"      → octomind run (subprocess)
                    ├── step "developer" → octomind run (subprocess)  ─┐
                    └── step "tester"    → octomind run (subprocess)  ─┘  loop
                                                                          │
                                                                          ▼
                                                              stderr: per-step
                                                                progress, cost,
                                                                tokens, totals
```

A workflow file is a portable TOML document — no edits to `default.toml` or any role config are needed. Each step invokes `octomind run --format jsonl`, streams the JSONL event log, accumulates assistant text and cost/token totals, then hands the captured output to the next step.

## CLI

```bash
echo "build a JSON-to-CSV CLI in Rust" | octomind workflow myflow.toml

# Validate + print execution plan without spawning anything
octomind workflow myflow.toml --dry-run
```

- stdin is required (unless `--dry-run`); empty stdin is a hard error.
- stdout receives only the final result (the step named by `result`, or the last top-level step if unset).
- stderr receives progress lines, per-step stats, warnings, and the final total.

## File format

```toml
name        = "my-workflow"
description = "Optional human description"
result      = "evaluator"   # which step's output goes to stdout; default: last step

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

Anywhere in a prompt, `{{name}}` is substituted with:

| Variable           | Value                                                                  |
|--------------------|------------------------------------------------------------------------|
| `{{input}}`        | The raw stdin content                                                  |
| `{{step_name}}`    | The full text output of a previously completed step (by name)          |

Forward references (`{{later}}` from an earlier step) are rejected at pre-flight validation. Step names must be unique across the entire file, including all sub-steps.

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

### Parallel (`parallel = true`)
Sub-steps run concurrently via `tokio::join_all`. The next top-level step starts only after every sub-step completes. Sub-steps cannot reference each other; only outer scope.

### Loop (`loop = true`)
Sub-steps run sequentially within each iteration. Between iterations, `exit_when` is checked against the named step's output:

- `exit_when = { output = "tester", contains = "NO ISSUES" }` — substring match
- `exit_when = { output = "tester", matches = "^PASS" }` — Rust regex match
- omit `output` to test the most recent step's output

If `max_iterations` is reached without exit, the loop exits with the last iteration's outputs and a warning to stderr (the workflow does **not** fail).

### Conditional (`conditional = true`)
`condition` tests a prior step output (same shape as `exit_when`). On match, the names in `on_match` run; otherwise `on_no_match` runs. Skipped sub-step names resolve to empty strings in later substitutions.

## Session modes

| Mode                          | Behaviour                                                              |
|-------------------------------|------------------------------------------------------------------------|
| `session = "fresh"` (default) | Brand-new session every invocation. No state persists.                 |
| `session = "continue"`        | First run: new session, ID is remembered. Subsequent runs (loop iter 2+, or retry): the same session is resumed; `/done` is sent first to compress prior context. The session is ephemeral to a single `octomind workflow` invocation. |

**Continue-session prompt rule:** on the *first* run of a continue-session, the templated prompt is sent. On *subsequent* runs, the templated prompt is **replaced** with the most recent prior step's raw output — the session already holds the full context, so it just needs the latest signal to react to. This is what makes the generator↔tester GAN pattern work without re-feeding the whole spec each iteration.

Each step owns its own session ID. In a loop, `developer` and `tester` accumulate independent histories.

## Retries and timeouts

- `retries = N` — up to N additional attempts on failure (default 0 ≙ one attempt).
- A step "fails" when the subprocess exits non-zero **or** produces no assistant output.
- `timeout = S` — seconds before the subprocess is killed (default 0 ≙ no timeout). A timeout counts as a failure for retry logic.
- All retries exhausted → workflow exits non-zero with `step '<name>' failed after <N> attempts`.

## Progress output (stderr)

```
╭ workflow: my-workflow
  ► spec           running...
  ✓ spec           2.1s  $0.0042  1240 tok
  ► refine [1/3]   developer    running...
  ✓ refine [1/3]   developer    8.4s  $0.0156  3208 tok
  ► refine [1/3]   tester       running...
  ✓ refine [1/3]   tester       3.2s  $0.0078  1450 tok
  ✓ exit condition matched at iteration 1
  ► evaluator      running...
  ✓ evaluator      1.8s  $0.0029  890 tok
╰ Total: 15.5s  $0.0305  6788 tok
```

Stats come from the `cost` events in the JSONL stream emitted by `octomind run --format jsonl`. Cost (`session_cost`), input/output/cache/reasoning tokens, and wall-clock duration are aggregated per step and totalled at the end.

## --dry-run

`octomind workflow file.toml --dry-run` validates the file, resolves the execution graph, and prints the plan to stdout without spawning any `octomind run` processes or reading stdin. Use it to sanity-check a workflow before paying for tokens.

## Validation

Pre-flight checks (all hard-fail before any step runs):

- File exists, valid TOML.
- Step names unique across the whole file.
- `'input'` is reserved (you can't name a step `input`).
- Every `{{var}}` references either `input` or a step that completes before the referencing step.
- A `parallel` step has at least 2 sub-steps; `loop` has ≥1 sub-step + `exit_when`; `conditional` has `condition` and at least one of `on_match` / `on_no_match`.
- `result` must point at a sequential step that produces output (not a composite container name).
- Regex patterns in `matches` compile.
- `model`, when specified on any step, must not be an empty string.

## End-to-end example

A generator/tester GAN that builds, reviews, and scores:

```toml
name   = "gan"
result = "evaluator"

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
- Structured (JSON) output from the workflow command itself
