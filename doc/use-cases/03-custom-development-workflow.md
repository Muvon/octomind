# Use Case: Custom Development Workflow

Build a multi-stage AI pipeline that refines, researches, and validates a task before another agent executes the fix.

## The Problem

Asking an AI to "fix the login bug" often produces mediocre results because it starts coding immediately without understanding context. You want a structured pipeline: first understand the task, then gather context, then execute.

## Solution

Use the external `octomind workflow <file.toml>` CLI to chain multiple independent `octomind run` invocations. Each step is its own session with its own role, model, and tools. Outputs flow between steps via `{{step_name}}` substitution.

> The session-internal `[[workflows]]` system and `/workflow` command have been removed. Workflows now sit **above** sessions, not inside them. See [Workflows](../usage/09-workflows.md) for the full reference.

### Architecture

```
echo "fix the login bug" | octomind workflow dev.toml
    |
    v
[refine]        Clarify the request, guess relevant files (gpt-4.1-mini)
    |
    v
[research]      Read code, search patterns, gather context (gemini-flash)
    |
    v
[execute]       Produce the fix with full understanding (claude-sonnet)
    |
    v
stdout
```

### Workflow File

Drop this at `dev.toml`:

```toml
name   = "dev"
result = "execute"

[[steps]]
name    = "refine"
role    = "developer:general"
session = "fresh"
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
prompt  = """
Implement the task using the gathered context.

Task:
{{refine}}

Context:
{{research}}
"""
```

### Run It

```bash
echo "fix the login bug" | octomind workflow dev.toml
```

Per-step timing, cost, and tokens stream to stderr; the final `execute` step's output lands on stdout.

## Advanced: Validation Loop

Add an iterative validate-fix cycle. Researcher and tester each get their own continuing session via `session = "continue"`:

```toml
name   = "validated_dev"
result = "execute"

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

## Cost Optimization

Each step is a separate `octomind run` invocation, so you can give each one its own role with its own model:

| Step | Role | Why |
|------|------|-----|
| refine | cheap fast model | Simple text refinement |
| research | fast model with large context | Code reading |
| tester | small judge model | Yes/no decision |
| execute | powerful model | Complex reasoning + code generation |

Configure the per-role model in your normal `[[roles]]` config (or use `[taps]` overrides per agent tag). The workflow file just names the role; cost lives in role config.

## Key Points

- Workflow steps are **separate sessions** — they don't share context unless `session = "continue"` is set
- Loop step exits as soon as `exit_when.contains` matches the named output
- Stdin → `{{input}}`; final stdout = the step named by `result =` (default: last step)
- All progress, timing, cost, tokens print to **stderr** — stdout stays clean for piping
- Use `--dry-run` to validate the file and print the execution plan without spawning any sessions
