# Use Case: Custom Development Workflow

Build a multi-stage AI pipeline that refines, researches, and validates a task before another agent executes the fix.

## The Problem

Asking an AI to "fix the login bug" often produces mediocre results because it starts coding immediately without understanding context. You want a structured pipeline: first understand the task, then gather context, then execute.

## Solution

Use the external `octomind workflow <file.toml>` CLI to chain multiple independent `octomind run` invocations. Each step is its own session with its own role, model, and tools. Outputs flow between steps via `{{step_name}}` substitution.

> The session-internal `[[workflows]]` system and `/workflow` command have been removed. Workflows now sit **above** sessions, not inside them. See [Workflows](../usage/09-workflows.md) for the full reference.

> This use-case covers **sequential** steps and the **loop** step. Workflows also support **parallel** and **conditional** steps, plus **graph routing** (a bounded `[[edges]]` control-flow graph with `entry` and `max_transitions`) — see [Workflows](../usage/09-workflows.md) for those, plus the full reference for `retries`, `timeout`, `model`, and variable substitution.

### Architecture

```
echo "fix the login bug" | octomind workflow dev.toml
    |
    v
[refine]        Clarify the request, guess relevant files (cheap fast model)
    |
    v
[research]      Read code, search patterns, gather context (large-context model)
    |
    v
[execute]       Produce the fix with full understanding (powerful model)
    |
    v
stderr (each step's response + per-step stats + totals)
```

### Workflow File

Drop this at `dev.toml`. The `role` values here (`developer:general`, `developer:brief`) are **tap agents** from the built-in default tap `muvon/tap` (auto-cloned on first use), not local `[[roles]]` shipped in `default.toml` — they resolve out of the box, or swap in any role/tag you already have. The optional per-step `model` profile overlays that role's resolved profile; omitted fields continue to inherit.

```toml
name   = "dev"

[[steps]]
name    = "refine"
role    = "developer:general"
session = "fresh"
model   = { name = "openai:gpt-5-mini" }   # cheap fast model for simple refinement
prompt  = """
Refine this request into a clear, actionable task. Guess which files might
be relevant. If already clear, return unchanged. Respond ONLY with the
refined task.

Request:
{{input}}
"""

[[steps]]
name    = "research"
role    = "developer:general"
session = "fresh"
model   = { name = "openrouter:google/gemini-2.5-flash-preview" }   # large-context model for code reading
timeout = 300                          # seconds; 0 = no timeout (default)
prompt  = """
Gather the key context for this task. Search relevant files, read
signatures (not full bodies), note conventions. Output:
- Starting Points: key files/functions
- Patterns: code conventions
- Context: dependencies / related components

Task:
{{refine}}
"""

[[steps]]
name    = "execute"
role    = "developer:general"
session = "fresh"
model   = { name = "anthropic:claude-sonnet-4-6", reasoning_effort = "high" }
retries = 1                                # one extra attempt on failure
prompt  = """
Implement the task using the gathered context.

Task:
{{refine}}

Context:
{{research}}
"""
```

Each sequential step (including sub-steps inside a loop) accepts these optional fields:

| Field | Default | What it does |
|-------|---------|--------------|
| `session` | `"fresh"` | `"fresh"` = brand-new session; `"continue"` = resume the same session across loop iterations |
| `model` | _(role default)_ | `provider:model` override forwarded as `--model`; must not be empty when set |
| `timeout` | `0` | Seconds before the subprocess is killed; `0` = no timeout. A timeout counts as a failure |
| `retries` | `0` | Extra attempts when the step fails (total attempts = `retries + 1`) |

A step **fails** when its `octomind run` subprocess exits non-zero, produces no assistant output, or hits its `timeout`. When all attempts are exhausted the whole workflow stops and exits non-zero with `step '<name>' failed after <N> attempts: <reason>`.

### Run It

```bash
echo "fix the login bug" | octomind workflow dev.toml
```

Each step's assistant message is rendered to **stderr** as it completes (with markdown rendering when enabled), alongside per-step timing, cost, and tokens. A plain run produces **no stdout** (pass `--format jsonl` for a machine-readable result on stdout) — see [Key Points](#key-points). A run looks like this (color stripped):

```
workflow · dev

╭ refine
╰ ✓ refine  1.4s  · $0.0009  · 420 tok  · ⚒0

╭ research
│ ▸ view · octofs
╰ ✓ research  6.2s  · $0.0071  · 2980 tok  · ⚒5

╭ execute
╰ ✓ execute  9.1s  · $0.0188  · 4120 tok  · ⚒8

total · 16.7s  · $0.0268  · 7520 tok  · ⚒13
```

> **Workflow step prompts resolve three kinds of `{{var}}`:** `{{input}}` (stdin), any prior `{{step_name}}` output, and built-in placeholders (`{{DATE}}`, `{{CWD}}`, `{{GIT_STATUS}}`, …) — the same three-pass substitution the interactive chat uses. Pre-flight validation (`src/workflow/validate.rs`, run even under `--dry-run`) rejects **any other** `{{var}}` before the workflow runs. You can also inline a file with a `<context>path</context>` / `<context>path:start:end</context>` block. See [Workflows → Variable substitution](../usage/09-workflows.md#variable-substitution).

## Advanced: Validation Loop

Add an iterative researcher↔tester refine cycle. Both sub-steps keep a continuing session via `session = "continue"`, so each loop iteration builds on the last instead of starting cold:

```toml
name   = "validated_dev"

[[steps]]
name    = "refine"
role    = "developer:general"
session = "fresh"
prompt  = "Refine: {{input}}"

[[steps]]
name           = "verify"
loop           = true
max_iterations = 3
exit_when      = { output = "tester", contains = "READY" }

  [[steps.run]]
  name    = "research"
  role    = "developer:general"
  session = "continue"
  prompt  = "Gather context for: {{refine}}"

  [[steps.run]]
  name    = "tester"
  role    = "developer:brief"
  session = "continue"
  prompt  = """
Is the gathered context sufficient to proceed?
- Yes  → reply READY
- No   → state what's missing

Context:
{{research}}
"""

[[steps]]
name    = "execute"
role    = "developer:general"
session = "fresh"
prompt  = "Implement: {{refine}}\n\nContext:\n{{research}}"
```

**How the loop actually runs.** This is the GAN-style refine pattern, and continue-sessions behave in a specific way you should understand before reading the prompts above literally:

- **Iteration 1** runs `research` with the templated prompt (`Gather context for: <refined task>`), then `tester` with its templated prompt.
- **Iteration 2+**: for each continue-session step, `octomind` first sends `/done` to compress the prior context, then **replaces the templated prompt with the most recent prior step's raw output**. So in round 2 `research` does not re-receive `Gather context for: …`; it receives `tester`'s last verdict and reacts to it. Likewise `tester` reacts to the fresh `research` output. The session already holds the full task, so each round only feeds it the latest signal.
- After every iteration, `exit_when` is tested. The loop stops as soon as `tester`'s output contains `READY`.

Continue-sessions are **ephemeral to a single `octomind workflow` invocation** — their generated session names (`wf-<workflow>-<step>-<uuid>`) are never reused across runs. For the full reference on this behavior, see [Workflows → Session modes](../usage/09-workflows.md#session-modes).

## Cost Optimization

Each step is a separate `octomind run` invocation, so you can match the model to the job — cheap models for simple steps, a powerful model only where it matters:

| Step | Job | Suggested model |
|------|-----|-----------------|
| refine | Simple text refinement | `openai:gpt-5-mini` |
| research | Code reading, large context | `openrouter:google/gemini-2.5-flash-preview` |
| tester | Yes/no judge decision | small judge model, e.g. `openai:gpt-5-mini` |
| execute | Complex reasoning + code generation | `anthropic:claude-sonnet-4-6` |

**How to actually set a step's model**, in priority order (highest wins):

1. **Per-step `model = { name = "provider:model", ... }` profile** — the simplest and most direct lever, shown above. Every explicitly set field is forwarded to the subprocess.
2. The model declared by the step's role/tap-agent definition (a plain `[[roles]]` entry, or a tap agent's manifest role).
3. A `[taps."tag".model]` override keyed by the agent tag.
4. The required `[model]` baseline.

> The historical scalar `model = "provider:model"` remains accepted as a name-only shorthand, so existing workflow files do not break.

## Key Points

- Workflow steps are **separate sessions** — they don't share context unless `session = "continue"` is set
- A loop exits as soon as its `exit_when` matches the named output via `contains` (substring) **or** `matches` (Rust regex); `exit_when` must set at least one of the two
- If a loop reaches `max_iterations` without `exit_when` matching, it prints a `⚠ … reached max_iterations` warning to stderr and continues with the last iteration's outputs — the workflow does **not** fail
- A step **fails** on non-zero exit, empty assistant output, or `timeout`. `retries = N` gives `N+1` total attempts; when all are exhausted the workflow exits non-zero with `step '<name>' failed after <N> attempts: <reason>`
- Stdin → `{{input}}`; each step's last assistant message prints to **stderr** as it completes (with markdown rendering when enabled)
- All progress, timing, cost, tokens print to **stderr**; a plain run produces **no stdout** — pass `--format jsonl` to stream per-step `assistant` + a final `cost` event to stdout for machine parsing
- `--dry-run` validates the file and prints the execution plan to **stdout** (the only stdout a *default* run produces; `--format jsonl` adds per-step + cost events), then exits — it never reads stdin and spawns no sessions
- Pre-flight validation (before any step runs) rejects: empty workflows, duplicate step names, a step named `input` (reserved), forward references to a not-yet-completed step, parallel blocks with fewer than 2 sub-steps, loops missing `exit_when`, an invalid `matches` regex, and an empty `model` string
- This page covers sequential and loop steps; for **parallel** and **conditional** steps see [Workflows](../usage/09-workflows.md#step-types), and for **graph routing** (`entry` + `[[edges]]` + `max_transitions`) see [Workflows → Graph routing](../usage/09-workflows.md#graph-routing)
