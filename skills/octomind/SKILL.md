---
name: octomind
description: Delegate coding to the Octomind CLI — a token-efficient, model-agnostic coding agent. Use when the user asks to use Octomind, or to hand off features, fixes, refactors, reviews, or long-running coding work to an external agent.
license: Apache-2.0
compatibility: Requires the octomind CLI (Linux, macOS, Windows) and model access via `octomind login` or a provider API key
metadata:
  author: Muvon
  version: "1.0.0"
---

# Octomind CLI

Use Octomind as an autonomous coding worker orchestrated by Hermes terminal/process tools. Octomind is an open-source,
model-agnostic coding agent in one Rust binary, built for long autonomous runs at low token cost (adaptive cache-aware
context compaction, out-of-band planning and loop detection).

## When to Use

- User explicitly asks to use Octomind
- You want an external coding agent to implement, fix, refactor, or review code
- Long-running coding sessions where token cost matters
- Parallel task execution in isolated workdirs/worktrees

## Prerequisites

- Install: `brew install muvon/tap/octomind` (macOS/Linux) or `cargo install octomind`; release binaries:
  https://github.com/muvon/octomind/releases
- Model access: `octomind login` (Octomind Cloud), or set a provider key such as `OPENROUTER_API_KEY` and choose that
  provider's model in `~/.local/share/octomind/config/config.toml`
- Verify: `octomind --version`
- Git repository for code tasks (recommended)

## One-Shot Tasks

`octomind run` takes no message argument — its positional argument is the agent tag. Pipe the task through stdin; piped
input runs non-interactively and exits when the work is done:

```
terminal(command="echo 'Add retry logic to API calls and update tests' | octomind run developer:general --format plain", workdir="~/project")
```

Machine-readable events instead of text:

```
terminal(command="echo 'List the TODO items in src/' | octomind run developer:general --format jsonl", workdir="~/project")
```

Force a specific model:

```
terminal(command="echo 'Refactor the auth module' | octomind run developer:general --format plain --model openrouter:anthropic/claude-sonnet-4", workdir="~/project")
```

Restrict file writes to the working directory with `--sandbox`.

## Iterative Work (Daemon + send)

For multi-step work, keep one named session alive and inject follow-ups from separate commands:

```
terminal(command="echo 'Implement the OAuth refresh flow' | octomind run developer:general --name oauth --daemon --format jsonl", workdir="~/project", background=true)
# Returns session_id

# Monitor progress
process(action="poll", session_id="<id>")
process(action="log", session_id="<id>")

# Send a follow-up to the running session
terminal(command="octomind send --name oauth 'Now add error handling for token expiry'", workdir="~/project")

# Stop
process(action="kill", session_id="<id>")
```

## Resuming Sessions

Sessions persist on disk. `--name <name>` resumes an existing session of that name; `--resume-recent` picks the most
recent session for the working directory:

```
terminal(command="echo 'Continue with the remaining tests' | octomind run developer:general --name oauth --format plain", workdir="~/project")
```

## Common Flags

| Flag | Use |
|------|-----|
| `run <tag>` | Start a specialist: `developer:general` for coding; other `domain:spec` tags come from Octomind's tap registry |
| `--format plain` / `--format jsonl` | Non-interactive output for piped input |
| `--name <name>` / `-n` | Create or resume a named session |
| `--resume-recent` | Resume the latest session in this directory |
| `--model <provider:model>` / `-m` | Override the model |
| `--daemon` | Keep the session alive for `octomind send` |
| `--sandbox` | Limit filesystem writes to the working directory |
| `--schema <file>` | Constrain output to a JSON Schema (model must support structured output) |

Hosts that speak the Agent Client Protocol (ACP) can run the same specialist as a sub-agent over stdio:
`octomind acp developer:general`.

## Procedure

1. Verify readiness: `terminal(command="octomind --version")`.
2. For bounded tasks, pipe the task into `octomind run developer:general --format plain`.
3. For iterative tasks, start a named `--daemon` session in the background and use `octomind send`.
4. Monitor long tasks with `process(action="poll"|"log")`.
5. Summarize file changes, test results, and remaining risks back to the user.

## Parallel Work Pattern

Use separate workdirs/worktrees to avoid collisions:

```
terminal(command="echo 'Fix issue #101 and commit' | octomind run developer:general --format plain", workdir="~/.hermes/cache/scratch/issue-101", background=true)
terminal(command="echo 'Add parser regression tests and commit' | octomind run developer:general --format plain", workdir="~/.hermes/cache/scratch/issue-102", background=true)
process(action="list")
```

## Pitfalls

- Passing the task as an argument (`octomind run 'fix it'`) treats the text as an agent tag — always pipe the task.
- `--format` at a terminal without piped input errors unless combined with `--daemon`.
- The first run of a specialist fetches its tap and installs tool dependencies, so it is slower than later runs.
- A missing `OCTOHUB_API_KEY` with the default `octohub:auto` model means `octomind login` was not run.
- Avoid sharing one working directory across parallel Octomind sessions.
- Run inside the project's git repository: the code-search server `octocode` refuses to start outside one, leaving
  the agent without semantic search.

## Verification

Smoke test:

```
terminal(command="echo 'Respond with exactly: OCTOMIND_SMOKE_OK' | octomind run developer:general --format plain")
```

Success criteria:

- Output includes `OCTOMIND_SMOKE_OK`
- Command exits without provider/model errors
- For code tasks: expected files changed and tests pass

## Rules

1. Prefer piped one-shot runs for automation — they exit on their own.
2. Use a named daemon session only when iteration is needed.
3. Always scope Octomind sessions to a single repo/workdir.
4. For long tasks, provide progress updates from `process` logs.
5. Report concrete outcomes (files changed, tests, remaining risks).
