# Custom Development Workflow

Use this guide to build a development pipeline that refines a request, researches the checkout, and implements a fix. It
covers sequential steps and a bounded review loop for users writing local workflow TOML.

## Get started

Use the external `octomind workflow <file.toml>` CLI to chain multiple independent `octomind run` invocations. Each step
is its own session with its own role, model, and tools. Outputs flow between steps via `{{step_name}}` substitution.

Run workflows from the shell with the `workflow` subcommand; the shared session command dispatcher does not implement
`/workflow`. Install and authenticate Octomind before running the examples:

```bash
octomind login
```

> This use-case covers **sequential** steps and the **loop** step. Workflows also support **parallel** and
> **conditional** steps, plus **graph routing** (a bounded `[[edges]]` control-flow graph with `entry` and
> `max_transitions`) — see [Workflows](../usage/09-workflows.md) for those, plus the full reference for `retries`,
> `timeout`, `model`, and variable substitution.

### Architecture

```text
echo "fix the login bug" | octomind workflow dev.toml
    |
    v
[refine]        Clarify the request and identify likely files
    |
    v
[research]      Read code, search patterns, gather context
    |
    v
[execute]       Produce the fix with full understanding
    |
    v
stderr (each step's response + per-step stats + totals)
```

## Configure the workflow

Save this as `dev.toml` in your checkout. Its context block reads `AGENTS.md`; change that path to your project
instructions file if needed. The role tags resolve through configured taps, with `muvon/tap` as the built-in fallback
(cloned on first use, with updates attempted on later resolution). You can substitute installed tags or explicitly
defined local `[[roles]]`. The execution step demonstrates an Anthropic model override and requires `ANTHROPIC_API_KEY`
plus access to that model; omit its `model` line to use the resolved role's model.

```toml
name   = "dev"

[[steps]]
name    = "refine"
role    = "developer:general"
session = "fresh"
prompt  = """
Refine this request into a clear, actionable task. Guess which files might
be relevant, labeling guesses. Do not edit files. If already clear, return unchanged. Respond ONLY with the
refined task.

Request:
{{input}}
"""

[[steps]]
name    = "research"
role    = "developer:general"
session = "fresh"
workdir = "."                          # relative to the directory running the workflow
timeout = 300                          # seconds; 0 = no timeout (default)
prompt  = """
Gather the key context for this task. Search relevant files, read
implementations needed to verify behavior, and note conventions. Do not edit files. Output:
- Starting Points: key files/functions
- Patterns: code conventions
- Context: dependencies / related components

Task:
{{refine}}

Working directory: {{CWD}}
Project instructions:
<context>AGENTS.md</context>
"""

[[steps]]
name    = "execute"
role    = "developer:general"
session = "fresh"
model   = "anthropic:claude-sonnet-4-6"     # main model name override
retries = 1                                # one extra attempt on failure
prompt  = """
Implement the task using the gathered context.

Task:
{{refine}}

Context:
{{research}}
"""
```

The examples use these optional sequential-step fields (defaults come from `src/workflow/schema.rs`, not global config):

| Field | Default | What it does |
|-------|---------|--------------|
| `session` | `"fresh"` | `"fresh"` = brand-new session; `"continue"` = resume the same session across loop iterations |
| `model` | _(role default)_ | `provider:model` override forwarded as `--model`; must not be empty when set |
| `timeout` | `0` | Seconds before the subprocess is killed; `0` = no timeout. A timeout counts as a failure |
| `retries` | `0` | Extra attempts when the step fails (total attempts = `retries + 1`) |
| `workdir` | Inherit workflow cwd | Subprocess directory; relative paths resolve from the workflow process, not its TOML file |

A step **fails** when its `octomind run` subprocess exits non-zero, produces no assistant output, or hits its `timeout`.
When all attempts are exhausted the whole workflow stops and exits non-zero with `step '<name>' failed after <N>
attempts: <reason>`.

## Run and inspect results

```bash
octomind workflow dev.toml --dry-run
printf 'Fix the login bug: valid credentials return 401.\n' | octomind workflow dev.toml
```

Each step's accumulated assistant text is rendered to stderr, alongside timing, cost, and tokens. Multiple assistant
events from a subprocess are joined with newlines. Plain workflow execution leaves stdout empty. To capture
machine-readable output, run:

```bash
printf 'Fix the login bug: valid credentials return 401.\n' \
  | octomind workflow dev.toml --format jsonl > results.jsonl 2> progress.log
jq -rs '[.[] | select(.type == "assistant")] | last | .content' results.jsonl
```

Workflow JSONL emits completed step responses with a `step` name, followed by an aggregate `cost` event. `--dry-run`
validates and prints a plan without reading stdin or spawning sessions; it does not verify provider access or task
success.

> **Workflow step prompts resolve three kinds of `{{var}}`:** `{{input}}` (stdin), any prior `{{step_name}}` output, and
> built-in placeholders (`{{DATE}}`, `{{CWD}}`, `{{GIT_STATUS}}`, …). Pre-flight validation (`src/workflow/validate.rs`,
> run even under `--dry-run`) rejects **any other** `{{var}}` before the workflow runs. You can also inline a file with
> a `<context>path</context>` / `<context>path:start:end</context>` block. See [Workflows → Variable
> substitution](../usage/09-workflows.md#variable-substitution).

## Add a bounded validation loop

Save this alternative as `validated-dev.toml` to add a researcher/tester cycle. Both sub-steps keep a continuing session
via `session = "continue"`, so each loop iteration builds on the last instead of starting cold:

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
exit_when      = { output = "tester", matches = '^\s*READY\s*$' }

  [[steps.run]]
  name    = "research"
  role    = "developer:general"
  session = "continue"
  prompt  = "Gather context for: {{refine}}"

  [[steps.run]]
  name    = "tester"
  role    = "developer:general"
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

Run it with:

```bash
octomind workflow validated-dev.toml --dry-run
printf 'Fix the login bug: valid credentials return 401.\n' | octomind workflow validated-dev.toml
```

Continue-sessions behave as follows:

- **Iteration 1** runs `research` with the templated prompt (`Gather context for: <refined task>`), then `tester` with
  its templated prompt.
- **Iteration 2+**: for each continue-session step, `octomind` first attempts `/done` (best effort) to compress the
  prior context, then **replaces the templated prompt with the most recent prior step's raw output**. So in round 2
  `research` does not re-receive `Gather context for: …`; it receives `tester`'s last verdict and reacts to it. Likewise
  `tester` reacts to the fresh `research` output. The session already holds the full task, so each round only feeds it
  the latest signal.
- After every iteration, `exit_when` is tested. The loop stops as soon as `tester`'s output is `READY`, allowing
  surrounding whitespace. The anchored regex avoids matching `NOT READY`.

Continue-session name reuse is **limited to a single `octomind workflow` invocation** — their generated session names
(`wf-<workflow>-<step>-<uuid>`) are generated anew on each run; their persisted history is not automatically deleted.
For the full reference on this behavior, see [Workflows → Session modes](../usage/09-workflows.md#session-modes).

Reaching `max_iterations` without a match warns and continues to `execute` with the last outputs. This is a bounded
refinement loop, not a mandatory approval gate. Use [conditional or graph routing](../usage/09-workflows.md#step-types)
when implementation must depend on a verdict.

## Common questions

- **Why is stdout empty?** Plain workflows render to stderr. Use the JSONL command above to collect step output.
- **Why did the researcher receive the tester's reply?** On reuse, a continuing step receives the most recent prior
  step's output instead of its original template. Each named step retains its own conversation; steps do not share one.
- **Why did a step repeat file changes?** Retries rerun failed steps; they do not roll back filesystem effects. The
  example permits one retry for `execute`, so use it only when repeating partial work is acceptable.
- **Why is a variable rejected?** Sequential prompts can reference `input`, completed outputs, and built-in
  placeholders. `--dry-run` catches unavailable forward references and malformed loop conditions before work starts.

## Model Purpose and Overrides

Octomind has exactly three model purposes: **main**, **supervisor**, and **compression**. Workflow steps are ordinary
`octomind run` subprocesses, so a step's optional `model` changes only the main-purpose model for that subprocess; it
does not create a fourth purpose. The shipped default uses OctoHub through `octomind login`, with `octohub:auto` for all
three purposes; the workflow above intentionally demonstrates a concrete main-purpose override instead.

The main-purpose model name for a workflow step is selected in this priority order (highest wins):

1. **Per-step `model = "provider:model"`** — the simplest and most direct lever, shown above. It overrides only the name
   and preserves all resolved parameters.
2. The model declared by the step's role/tap-agent definition (a plain `[[roles]]` entry, or a tap agent's manifest
   role).
3. A scalar `[taps]` model-name mapping keyed by the agent tag.
4. The required `[model]` baseline.

## See also

- [Workflows reference](../usage/09-workflows.md)
- [CI/CD code review](01-ci-cd-code-review.md)
- [Multi-agent delegation](05-multi-agent-delegation.md)
