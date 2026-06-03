# Workflows

`octomind workflow <file.toml>` is an external orchestrator that chains multiple `octomind run` invocations into a multi-step process. Each step is an independent subprocess; outputs flow between steps by name; everything you see — per-step responses, progress, costs, totals — is written to **stderr** for a human to watch.

> **There is no machine-readable stdout result.** A real run prints nothing to stdout; stdout is used *only* by `--dry-run` to print the execution plan. Don't build shell pipelines that consume the workflow's stdout — they will get nothing. If you need a step's text downstream, read it from stderr or have the final step write to a file itself.

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
        stderr: per-step responses + progress, cost, tokens, totals
        stdout: (nothing — only --dry-run prints the plan here)
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
- stderr receives each step's assistant message (rendered as markdown when `enable_markdown_rendering` is on), progress lines, per-step stats, warnings, and the final total. **stdout carries nothing except the `--dry-run` plan.**

## File format

```toml
name        = "my-workflow"
description = "Optional human description"

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

Every step prompt is resolved in **three passes**, in order, exactly like the interactive chat resolves user input:

**Pass 1 — workflow variables.** Anywhere in a prompt, `{{name}}` is substituted with:

| Variable           | Value                                                                  |
|--------------------|------------------------------------------------------------------------|
| `{{input}}`        | The raw stdin content (trimmed)                                        |
| `{{step_name}}`    | The full text output of a previously completed step (by name)          |

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

> **Caveat — built-in placeholders are rejected by pre-flight validation today.** Validation (run for every workflow, including `--dry-run`, in `src/workflow/validate.rs`) flags **any** `{{var}}` that is not `{{input}}` or a declared step name as an *unknown variable* and aborts before the step runs. The built-ins above are not step names, so a step prompt that contains one (e.g. `{{DATE}}`) fails validation and never reaches this expansion pass. In practice, only `{{input}}` and `{{step_name}}` references are usable in workflow step prompts; put date/context information into the step prompt text directly instead.

**Pass 3 — context file inlining.** Any `<context>path</context>` or `<context>path:start:end</context>` block is replaced with the named file's contents rendered as XML (the same file-context path chat uses). Use `path:start:end` to inline only a line range. Because this runs on every step prompt, a step can also emit a `<context>...</context>` block in *its own* response and the next step that interpolates `{{that_step}}` will receive the file inlined.

Forward references (`{{later}}` from an earlier step) are rejected at pre-flight validation — which rejects **any** `{{var}}` that is not `{{input}}` or an already-defined step name (see the caveat above). Step names must be unique across the entire file, including all sub-steps. `<context>` blocks use angle brackets rather than `{{ }}`, so they are not treated as variable references.

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

A `session = "continue"` field on a parallel sub-step is **silently ignored** — parallel sub-steps always run with a fresh session. Continue-session state only makes sense across the sequential iterations of a loop.

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

## --dry-run

`octomind workflow file.toml --dry-run` validates the file, resolves the execution graph, and prints the plan to **stdout** — the one and only thing a workflow ever writes to stdout. It spawns no `octomind run` processes and never reads stdin (validation runs before the stdin step, and `--dry-run` returns immediately after). Use it to sanity-check a workflow before paying for tokens.

## Validation

Pre-flight checks (all hard-fail before any step runs):

- File exists, valid TOML.
- Step names unique across the whole file.
- `'input'` is reserved (you can't name a step `input`).
- Every `{{var}}` references either `input` or a step that completes before the referencing step.
- A `parallel` step has at least 2 sub-steps; `loop` has ≥1 sub-step + `exit_when`; `conditional` has `condition` and at least one of `on_match` / `on_no_match`.
- Regex patterns in `matches` compile.
- `model`, when specified on any step, must not be an empty string.

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
- Any machine-readable stdout result. Everything is human-facing on stderr; stdout only ever carries the `--dry-run` plan.
