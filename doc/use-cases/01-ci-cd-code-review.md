# Automated Code Review in CI/CD

Run Octomind as part of your CI/CD pipeline to automatically review pull requests, check for security issues, or enforce coding standards.

## The Problem

Manual code review is slow. You want AI to catch common issues — security vulnerabilities, performance problems, style violations — before human reviewers even look at the PR.

## Solution

Use non-interactive mode and read the result back as JSON.

Two facts shape everything below, so get them straight up front:

1. **`--format plain` is human-oriented output, not data.** The assistant reply
   is wrapped in `─────` horizontal rules and markdown-rendered by default;
   terminal styling is applied when the output environment supports it. It is
   not an input for `jq`.
2. **`--format jsonl` is the machine-readable surface.** It emits a *stream* of
   type-tagged JSON objects, one per line — `assistant`, `cost`, and (when they
   occur) `thinking`, `tool_use`, `tool_result`, `status`. It is NOT a single
   JSON object. To get the model's answer you filter the `assistant` line(s) out
   of the stream.

> **Structured output:** `octomind run --schema <PATH>` loads a JSON Schema and
> requires the resolved model to support schema-constrained output. `--format
> jsonl` still controls the transport: the validated model response appears in
> one or more `assistant` events within the JSONL stream.

> **Note on agents:** the commands below use the `developer:general` tap agent.
> It ships via the built-in default tap `muvon/tap`, which auto-clones on first
> use (a one-time network fetch). See [Tap System](../integration/04-tap-system.md).

### Basic: Review from Stdin

In non-interactive mode Octomind reads the **entire** message from stdin — a
single stream. Do not combine a pipe with a heredoc (`<<<`); only one of them
reaches stdin and the other is silently dropped. Build the whole prompt, diff
included, and pipe it once:

```bash
# Feed the prompt + diff to Octomind and print a human-readable review
diff=$(git diff main..HEAD)
printf 'Review this diff for security issues, performance problems, and bugs. Be specific about file and line numbers.\n\n%s' "$diff" \
  | octomind run developer:general --format plain
```

The `--format plain` output is framed and may be terminal-styled, which is fine
for a human reading the log. If you need to scrape it as text, prefer the `jsonl`
approach below, or disable rendering with `enable_markdown_rendering = false` in
config (and strip ANSI / the `─────` rules) — but JSON is the cleaner path.

### Structured: JSON Output for Pipeline Decisions

Create a schema file, pass it with `--schema`, run with `--format jsonl`, then
pull the final `assistant` payload out of the stream:

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["summary", "issues", "approval"],
  "properties": {
    "summary": {"type": "string"},
    "issues": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["file", "line", "severity", "description"],
        "properties": {
          "file": {"type": "string"},
          "line": {"type": "integer", "minimum": 0},
          "severity": {"type": "string"},
          "description": {"type": "string"}
        }
      }
    },
    "approval": {"type": "string", "enum": ["approve", "request_changes"]}
  }
}
```

Save that as `review-schema.json`, then run:

```bash
#!/bin/bash
# ci-review.sh
set -euo pipefail

diff=$(git diff main..HEAD)

# Run non-interactively. jsonl is a stream of type-tagged objects, one per line.
stream=$(printf 'Review this diff for issues. Return the requested structured review.\n\n%s' "$diff" \
  | octomind run developer:general --schema review-schema.json --format jsonl)

# Slurp the stream and join every assistant chunk in order.
review=$(echo "$stream" | jq -rsc '[.[] | select(.type == "assistant") | .content] | join("")')

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
        run: curl -fsSL https://octomind.run/install.sh | bash

      - name: Run AI Review
        env:
          OCTOHUB_API_KEY: ${{ secrets.OCTOHUB_API_KEY }}
        run: |
          diff=$(git diff origin/main..HEAD)
          # --format plain is fine here: we want a human-readable summary in the job log.
          summary=$(printf 'Review for security, performance, bugs.\n\n%s' "$diff" \
            | octomind run developer:general --format plain)
          echo "$summary" >> "$GITHUB_STEP_SUMMARY"
```

### Multi-step pipelines

For anything beyond a single review call — chaining steps and passing one step's
output into the next — use the [`octomind workflow`](../usage/09-workflows.md)
subcommand instead of hand-rolling shell. It reads the initial input from stdin
and runs each step as an independent `octomind run` subprocess:

```bash
git diff main..HEAD | octomind workflow review.toml 2> progress.log
```

Without `--format`, a workflow writes step responses and progress to stderr and
leaves stdout empty; `--dry-run` is the exception and prints its plan to stdout.
For machine-readable results, pass `--format jsonl`: stdout then receives one
`assistant` event per completed step followed by an aggregate `cost` event.
See [Workflows](../usage/09-workflows.md) for the full model.

## Authentication and Model Selection

The shipped default uses OctoHub: run `octomind login` once to mint the
`OCTOHUB_API_KEY`, store that value in the CI secret manager, and let the main
model purpose use its default `octohub:auto` model. `run` takes its message from
stdin; it has no free-text message positional because the only positional is the
role or tap tag. This example intentionally selects a concrete non-default model:

```bash
echo 'Quick check for obvious bugs' \
  | octomind run developer:general --model openai:gpt-5.6-luna --format plain
```

## Clean CI logs

To keep CI output tidy:

- Non-interactive mode (`--format plain`/`jsonl` with piped stdin) shows no
  spinner or animations — those only appear in an interactive terminal.
- Set `log_level = "none"` in config (or `octomind config --log-level none`) to
  suppress informational logging.
- Restrict filesystem writes with the `--sandbox` flag (or `sandbox = true` in
  config). The OS policy permits the working tree and the platform-specific
  state/temp paths described by the sandbox implementation. Octomind can use
  configured file and shell tools, not just the diff you pipe in.

## Key Points
- `--format plain` = human-readable output (rules, markdown, and optional
  terminal styling). `--format jsonl` = machine-readable stream of type-tagged JSON
  objects, one per line.
- Both run non-interactively and read the whole message from stdin. Use one
  stdin source — never a pipe AND a heredoc together.
- Extract the answer from jsonl with `jq -rsc '[.[] | select(.type=="assistant") | .content] | join("")'`.
- The default model is `octohub:auto`; `--model` explicitly overrides the
  main model purpose for that run.
- `--schema PATH` enables schema-constrained output when the resolved model
  supports it; `--format jsonl` makes the transport machine-readable.
- Octomind has full tool access — it can read and write files, not just the
  diff you pipe in. Use `--sandbox` to confine writes in CI.
