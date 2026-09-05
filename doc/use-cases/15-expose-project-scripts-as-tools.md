# Let the agent run your project checks correctly

Expose project scripts as typed tools so agents run the right builds, tests, and release commands.

## The problem

Your team already has scripts for builds, tests, and releases, but a new agent session does not know their names or
arguments. You keep explaining the same commands, and the agent sometimes substitutes a generic test command that
misses your project's checks. Give your script a discoverable description and typed arguments so the agent can call it
directly.

## What you will set up

- [Local tools](../usage/17-local-tools.md) discovered from `.agents/tools/` in your working directory.
- [Tool descriptions and argument schemas](../usage/17-local-tools.md) generated from script headers.
- [MCP inspection](../reference/02-session-commands.md) with `/mcp list` and `/mcp full`.
- [Piped sessions](../usage/05-sessions.md) for repeating the same check from your terminal.

## Prerequisites

Use macOS or Linux with Bash and the usual command-line utilities. Check the binary and shell:

```bash
octomind --version
bash --version
command -v mkdir cat chmod test printf
```

Check your existing Octomind login. If it has expired, complete the browser sign-in that this command starts:

```bash
octomind login
```

You will use the shipped `assistant` role and its configured model. Check the configuration before creating files:

```bash
octomind config --validate
```

Look for `Configuration is valid.` No external MCP server or package manager is needed for the example script.

## Steps

### 1. Create a small project with a real check script

Run these commands in Bash from a directory where you keep experiments. Use a new directory so the example cannot
overwrite an existing project's scripts. The check will verify that the project README exists and has content.

```bash
mkdir octomind-tools-lab
cd octomind-tools-lab
mkdir -p scripts .agents/tools
printf '%s\n' '# Tools lab' 'A project with a bespoke documentation check.' > README.md
cat > scripts/check.sh <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

suite="${1:?suite is required}"
verbose="${2:-false}"

if [[ "$suite" != "docs" ]]; then
  printf 'Unsupported suite: %s; choose docs\n' "$suite" >&2
  exit 2
fi

if [[ ! -s README.md ]]; then
  printf 'README.md is missing or empty\n' >&2
  exit 1
fi

if [[ "$verbose" == "true" ]]; then
  printf 'Checked README.md from the project root\n'
fi
printf 'PASS: docs\n'
EOF
bash scripts/check.sh docs true
```

### 2. Wrap the script as a local tool

Save the following wrapper at `octomind-tools-lab/.agents/tools/project_check`. Its filename becomes the tool name.
Use letters, digits, underscores, or hyphens; do not add a `.sh` extension to the tool filename.

The contiguous comment block supplies the schema. `*suite` makes the string argument required. `verbose` is an optional
boolean. The script still checks its own inputs; a schema is not a replacement for script validation.

```bash
cat > .agents/tools/project_check <<'EOF'
#!/usr/bin/env bash
# @description Run the project's documentation checks. Use suite docs; report the real exit result.
# @param *suite string Check suite to run. The supported value is docs.
# @param verbose boolean Include the names of checked files. Defaults to false.

set -euo pipefail
: "${OCTOMIND_PARAM_SUITE:?suite is required}"

exec bash scripts/check.sh \
  "$OCTOMIND_PARAM_SUITE" \
  "${OCTOMIND_PARAM_VERBOSE:-false}"
EOF
chmod +x .agents/tools/project_check
```

### 3. Check the wrapper before asking a model to use it

Octomind runs the wrapper with the session working directory as its current directory. Arguments arrive both as a JSON
object on stdin and as `OCTOMIND_PARAM_<UPPERCASE_NAME>` environment variables. This wrapper uses the environment form.

Successful stdout becomes the tool result. A nonzero exit produces an error result; stderr is included with an
`[stderr]` marker. For this direct shell check, expect two lines: `Checked README.md from the project root`, then
`PASS: docs`.

```bash
OCTOMIND_PARAM_SUITE=docs OCTOMIND_PARAM_VERBOSE=true ./.agents/tools/project_check
```

### 4. Inspect the discovered tool

Start the session from `octomind-tools-lab`, where `.agents/tools/` lives. A script in a different working directory
does not become a tool for this session.

```bash
octomind run assistant
```

### 5. Read the schema and call the check

Type the following entries separately at the Octomind prompt. `/mcp list` should include `project_check` under `local`.
`/mcp full` should show its description, required string `suite`, and optional boolean `verbose`.

The final entry is your request to the model. Its wording may vary; the script's successful output is `PASS: docs`.
Check the actual tool activity before accepting the agent's summary.

```text
/mcp list
/mcp full
Call project_check with suite "docs" and verbose true. Report its output and whether it succeeded.
```

### 6. Confirm that script failures reach the agent

Ask for an unsupported suite. Your wrapper forwards it to the script, which writes `Unsupported suite: missing; choose
docs` to stderr and exits with status 2. The agent should report a failed check, not claim that the docs passed.

Finish with `/exit` to return to your shell after the reply.

```text
Call project_check with suite "missing". Do not change any files; report the tool's error.
/exit
```

### 7. Repeat the successful call without an interactive session

Pipe the prompt through stdin. The positional argument to `run` is the role, not the message. This invocation uses the
same working directory and discovers the same wrapper.

```bash
printf '%s\n' 'Call project_check with suite "docs" and verbose true. Report the result.' | \
  octomind run assistant --format plain
```

## Verify it works

Start another session in `octomind-tools-lab`:

```bash
octomind run assistant
```

Inspect the schema and request a call:

```text
/mcp full
Call project_check with suite "docs" and verbose false.
```

Look for `project_check`, its two parameters, and a successful tool result containing `PASS: docs`. A sentence claiming
success without a tool call does not verify discovery or execution.

## Variations

- **Existing test script.** Replace the wrapper's `exec bash scripts/check.sh` invocation with your team's documented
  script and supported arguments. Keep parameter values quoted and retain the script's exit status.
- **Build or deployment script.** Create a separate executable wrapper with a distinct name and description. If your
  deployment script has a documented dry-run mode, expose that mode explicitly before enabling real deployments.
- **Structured arguments.** Use `array` or `object` header types and parse the stdin JSON in your script. Complex values
  are also JSON strings in their corresponding environment variables.
- **Automation.** Use the piped command from step 7 in a job that already has Octomind credentials and the project
  checkout. Select `--format jsonl` when the consumer needs structured session events.

## Troubleshooting

**The tool is missing from `/mcp list`.** Check the session working directory, the executable bit, and the filename.
The header needs a nonempty `@description`. Keep all header tags together above the first blank line after the header
starts or executable statement. Only the first 80 lines are scanned. The shipped `assistant` role has configured MCP
servers; a custom role with no active servers can hit the tool-list's early return before local discovery.

**The script sees an empty argument.** Use `OCTOMIND_PARAM_SUITE`, not `SUITE`. Optional parameters can be absent, so
provide their defaults in the wrapper. Keep the required-argument check even though the header marks `suite` required.

**The wrapper works manually but cannot find project files in the session.** Relative paths resolve from the session
working directory, not `.agents/tools/`. Start Octomind at the project root or use the provided `OCTOMIND_WORKDIR`
environment variable inside your wrapper.

**A long script times out.** Local-tool execution has a 300-second timeout. Break the operation into bounded checks or
use your existing job system for longer work. A `timeout` comment in the script header does not configure this limit.

## See also

- [Local Tools](../usage/17-local-tools.md)
- [MCP Tools](../usage/07-mcp-tools.md)
- [Session Commands](../reference/02-session-commands.md)
- [Sessions](../usage/05-sessions.md)
