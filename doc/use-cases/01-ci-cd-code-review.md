# Automated Code Review in CI/CD

Use this guide to add Octomind reviews to a CI pipeline. It covers stdin prompts, structured review results, and a shell
gate for pull requests.

## Get started

Install Octomind using the [installation guide](../usage/01-installation.md), and make `git` and `jq` available in your
CI runner. Authenticate locally, then store the minted `OCTOHUB_API_KEY` in your CI secret manager:

```bash
octomind login
```

The login output shows the file containing the key. The shipped main model is `octohub:auto`; inject the secret as
`OCTOHUB_API_KEY` in the runner environment. `run` takes only a role or tap tag as its positional argument; the prompt
comes from stdin.

## Choose the output format

Two facts shape everything below, so get them straight up front:

1. **`--format plain` is human-oriented output, not data.** The assistant reply is wrapped in `─────` horizontal rules
   and markdown-rendered by default; terminal styling is applied when the output environment supports it. It is not an
   input for `jq`.
2. **`--format jsonl` is the machine-readable surface.** It emits a *stream* of type-tagged JSON objects, one per line —
   `assistant`, `cost`, and (when they occur) `thinking`, `tool_use`, `tool_result`, `status`. It is NOT a single JSON
   object. To get the model's answer you filter the `assistant` line(s) out of the stream.

> **Structured output:** `octomind run --schema <PATH>` loads a JSON Schema and requires the resolved model to support
> schema-constrained output. `--format jsonl` still controls the transport: the model response appears in `assistant`
> events within the JSONL stream. Strict schema enforcement is requested from the provider.

> **Note on agents:** the commands below use the `developer:general` tap agent. It ships via the built-in default tap
> `muvon/tap`, which auto-clones on first use and attempts updates on subsequent resolution. See [Tap
> System](../integration/04-tap-system.md).

### Basic: Review from Stdin

In non-interactive mode Octomind reads the **entire** message from stdin — a single stream. Do not combine a pipe with a
here-string (`<<<`); only one of them reaches stdin and the other is silently dropped. Build the whole prompt, diff
included, and pipe it once:

```bash
# Feed the prompt + diff to Octomind and print a human-readable review
diff=$(git diff main..HEAD)
printf 'Review this diff for bugs and security issues. Cite files and lines.\n\n%s' "$diff" \
  | octomind run developer:general --format plain
```

The `--format plain` output is framed and may be terminal-styled, which is fine for a human reading the log. Use JSONL
when extracting the answer programmatically.

## Configure a structured review

Create a schema file, pass it with `--schema`, run with `--format jsonl`, then pull the final `assistant` payload out of
the stream:

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
          "severity": {"type": "string", "enum": ["error", "warning", "info"]},
          "description": {"type": "string"}
        }
      }
    },
    "approval": {"type": "string", "enum": ["approve", "request_changes"]}
  }
}
```

Save that as `review-schema.json` at the repository root. Save the following script as `ci-review.sh`:

```bash
#!/bin/bash
# ci-review.sh
set -euo pipefail

diff=$(git diff main..HEAD)

# Run non-interactively. jsonl is a stream of type-tagged objects, one per line.
stream=$(printf 'Review this diff for issues. Return the requested structured review.\n\n%s' "$diff" \
  | octomind run developer:general --schema review-schema.json --format jsonl)

# Assistant events contain complete responses. Select the last one and fail on missing/invalid JSON.
review=$(printf '%s\n' "$stream" | jq -esc '
  [.[] | select(.type == "assistant") | .content] | last | fromjson
  | if (.approval == "approve" or .approval == "request_changes") and (.issues | type == "array")
    then . else error("missing review fields") end')

# Read the checked fields used by this gate.
approval=$(echo "$review" | jq -r '.approval')
errors=$(echo "$review" | jq '[.issues[] | select(.severity == "error")] | length')

echo "Review: $approval ($errors errors)"

if [ "$approval" = "request_changes" ] || [ "$errors" -gt 0 ]; then
  echo "$review" | jq '.issues[]'
  exit 1
fi
```

## Run the CI gate

Run from the checked-out repository root after fetching your target branch. These examples compare `main` with `HEAD`;
substitute your repository's base branch if it differs. Configure your CI job to fail when this script exits non-zero:

```bash
bash ci-review.sh
```

The script exits 1 for a requested change or an issue with severity `error`. `set -euo pipefail` also makes command,
provider, and JSON parsing failures fail the job. A human-readable `--format plain` review alone does not gate on
findings.

For a multi-step review, see [custom development workflows](03-custom-development-workflow.md).

## Clean CI logs

To keep CI output tidy:

- Non-interactive mode (`--format plain`/`jsonl` with piped stdin) shows no spinner or animations — those only appear in
  an interactive terminal.
- Set `log_level = "none"` in config (or `octomind config --log-level none`) to suppress informational logging.
- Restrict filesystem writes with the `--sandbox` flag (or `sandbox = true` in config). The OS policy permits the
  working tree and the platform-specific state/temp paths described by the sandbox implementation. Octomind can use
  configured file and shell tools, not just the diff you pipe in.

```bash
octomind config --log-level none
printf 'Review the current checkout for bugs. Do not edit files.\n' \
  | octomind run developer:general --sandbox --format jsonl
```

The sandbox restricts writes on supported Linux/macOS systems; it permits state paths as well as the checkout. It does
not make the checkout read-only. Unsupported platforms warn and continue without write restrictions; Linux can also
continue without enforcement when Landlock is unavailable.

## Common questions

- **Why does stdin appear empty?** Pipe a non-empty prompt. A diff can be empty, so include the instruction even when no
  files changed. `--format` with terminal stdin fails unless you use daemon mode.
- **Why does `jq` fail?** Parse JSONL as a stream, select the last `assistant` response, and then parse its `content`.
  Do not concatenate separate assistant responses or feed framed plain output to `jq`.
- **Why does the schema request fail?** The resolved model must support structured output, and the provider must accept
  your schema. `--schema` reads a JSON object from a file; it does not turn plain transport into JSONL.

## See also

- [Configuration reference](../reference/03-config-reference.md)
- [Tap system](../integration/04-tap-system.md)
- [Custom development workflows](03-custom-development-workflow.md)
