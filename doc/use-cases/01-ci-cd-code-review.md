# Use Case: Automated Code Review in CI/CD

Run Octomind as part of your CI/CD pipeline to automatically review pull requests, check for security issues, or enforce coding standards.

## The Problem

Manual code review is slow. You want AI to catch common issues -- security vulnerabilities, performance problems, style violations -- before human reviewers even look at the PR.

## Solution

Use non-interactive mode and read the result back as JSON.

Two facts shape everything below, so get them straight up front:

1. **`--format plain` is decorated terminal text, not data.** In a pipeline the
   assistant reply is wrapped in `─────` horizontal rules, colored, and
   markdown-rendered by default. It is meant for humans (e.g. a job summary), not
   for `jq`. Use it only for human-readable output.
2. **`--format jsonl` is the machine-readable surface.** It emits a *stream* of
   type-tagged JSON objects, one per line -- `assistant`, `cost`, and (when they
   occur) `thinking`, `tool_use`, `tool_result`, `status`. It is NOT a single
   JSON object. To get the model's answer you filter the `assistant` line(s) out
   of the stream.

> **Note on "structured output":** the CLI does not enforce a JSON Schema on the
> model's reply. There is no `--schema` flag. What you get is *prompt-coaxed*
> JSON -- you ask the model to reply as JSON and parse it best-effort. Provider-
> enforced schema output is only available through the WebSocket and ACP server
> interfaces. See [Structured Output](../usage/11-structured-output.md).

> **Note on roles:** the commands below use the built-in `assistant` role. The
> built-in roles are `assistant`, `task_refiner`, `task_researcher`, and `reduce`
> (the default tag is `assistant:concierge`). A dedicated `developer:general`
> agent also exists; it ships via the built-in default tap `muvon/tap`, which
> auto-clones on first use (a one-time network fetch). See [Tap System](../integration/04-tap-system.md).

### Basic: Review from Stdin

In non-interactive mode Octomind reads the **entire** message from stdin -- a
single stream. Do not combine a pipe with a heredoc (`<<<`); only one of them
reaches stdin and the other is silently dropped. Build the whole prompt, diff
included, and pipe it once:

```bash
# Feed the prompt + diff to Octomind and print a human-readable review
diff=$(git diff main..HEAD)
printf 'Review this diff for security issues, performance problems, and bugs. Be specific about file and line numbers.\n\n%s' "$diff" \
  | octomind run assistant --format plain
```

The `--format plain` output is colorized and framed for a terminal, which is fine
for a human reading the log. If you need to scrape it as text, prefer the `jsonl`
approach below, or disable rendering with `enable_markdown_rendering = false` in
config (and strip ANSI / the `─────` rules) -- but JSON is the cleaner path.

### Structured: JSON Output for Pipeline Decisions

Ask the model to reply as JSON, run with `--format jsonl`, then pull the
`assistant` payload out of the stream and parse the JSON the model put inside it:

```bash
#!/bin/bash
# ci-review.sh
set -euo pipefail

diff=$(git diff main..HEAD)

# Run non-interactively. jsonl is a stream of type-tagged objects, one per line.
stream=$(printf 'Review this diff for issues. Respond ONLY with JSON of the form: {"summary": "...", "issues": [{"file": "...", "line": 0, "severity": "...", "description": "..."}], "approval": "approve|request_changes"}.\n\n%s' "$diff" \
  | octomind run assistant --model openai:gpt-4o --format jsonl)

# Slurp the stream, keep the last assistant line, take its .content (the model's text).
review=$(echo "$stream" | jq -rsc 'map(select(.type == "assistant")) | last | .content')

# $review is the model's JSON string — parse it as JSON now.
approval=$(echo "$review" | jq -r '.approval')
errors=$(echo "$review" | jq '[.issues[] | select(.severity == "error")] | length')

echo "Review: $approval ($errors errors)"

if [ "$approval" = "request_changes" ] || [ "$errors" -gt 0 ]; then
  echo "$review" | jq '.issues[]'
  exit 1
fi
```

```yaml
# .github/workflows/ai-review.yml
name: AI Code Review
on: [pull_request]

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Install Octomind
        run: curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

      - name: Run AI Review
        env:
          OPENROUTER_API_KEY: ${{ secrets.OPENROUTER_API_KEY }}
        run: |
          diff=$(git diff origin/main..HEAD)
          # --format plain is fine here: we want a human-readable summary in the job log.
          summary=$(printf 'Review for security, performance, bugs.\n\n%s' "$diff" \
            | octomind run assistant --format plain)
          echo "$summary" >> "$GITHUB_STEP_SUMMARY"
```

### Multi-step pipelines

For anything beyond a single review call -- chaining steps and passing one step's
output into the next -- use the [`octomind workflow`](../usage/09-workflows.md)
subcommand instead of hand-rolling shell. It reads the initial input from stdin
and runs each step as an independent `octomind run` subprocess:

```bash
git diff main..HEAD | octomind workflow review.toml 2> progress.log
```

Note an important quirk: a real workflow run writes **everything** -- per-step
responses, progress, cost, and totals -- to **stderr**. stdout is empty (it is
used only by `--dry-run`, which prints the execution plan). So you cannot capture
a workflow's final answer from stdout; if a downstream step needs a result, have
that step write it to a file itself. See [Workflows](../usage/09-workflows.md)
for the full model.

## Cost Control

Octomind has two spending thresholds, in USD, and they behave **differently** --
this matters in CI:

```toml
# Hard-stops a single request once it exceeds the limit. Safe for CI.
max_request_spending_threshold = 0.50  # max $0.50 per review

# PROMPTS the user before continuing once the cumulative session cost exceeds
# the limit. The prompt is interactive — in a non-interactive CI run there is
# nobody to answer it, so the run can block. Leave at 0.0 in CI, or rely on the
# request threshold instead.
max_session_spending_threshold = 0.0   # 0.0 = no limit
```

`>0` enables a threshold; `0.0` disables it. For CI, prefer
`max_request_spending_threshold` (it stops execution) and keep
`max_session_spending_threshold` at `0.0` to avoid a blocking prompt.

Or use a cheaper model for initial triage. `run` takes its message from stdin --
there is no free-text message argument (the only positional is the role/tag):

```bash
echo 'Quick check for obvious bugs' \
  | octomind run assistant -m openrouter:openai/gpt-4o-mini --format plain
```

## Clean CI logs

To keep CI output tidy:

- Non-interactive mode (`--format plain`/`jsonl` with piped stdin) shows no
  spinner or animations -- those only appear in an interactive terminal.
- Set `log_level = "none"` in config (or `octomind config --log-level none`) to
  suppress informational logging.
- Restrict filesystem writes with the `--sandbox` flag (or `sandbox = true` in
  config) so a review run can read your tree but cannot write outside the current
  working directory. Octomind has full tool access by default -- it can read and
  write files, not just the diff you pipe in -- so sandboxing is worth enabling in
  CI.

## Key Points
- `--format plain` = human-readable, decorated terminal text (rules + color +
  markdown). `--format jsonl` = machine-readable stream of type-tagged JSON
  objects, one per line.
- Both run non-interactively and read the whole message from stdin. Use one
  stdin source -- never a pipe AND a heredoc together.
- Extract the answer from jsonl with `jq -rsc 'map(select(.type=="assistant")) | last | .content'`.
- `--model` to balance cost vs quality per pipeline step.
- The CLI gives prompt-coaxed JSON, not schema-enforced output. For enforced
  schemas use the WebSocket/ACP servers -- see
  [Structured Output](../usage/11-structured-output.md).
- Octomind has full tool access -- it can read and write files, not just the
  diff you pipe in. Use `--sandbox` to confine writes in CI.
