# Build a source-backed map of an unfamiliar codebase

Build a source-backed map of an unfamiliar codebase, from entry points and state ownership to tests and one execution path.

## The problem

You join a team with a legacy repository and need to make a change before you understand how it fits together. The
README describes the product, but it does not tell you where requests enter, which code owns state, or which tests
matter. Use a persistent conversation to move from the file tree to one concrete execution path, keeping source
evidence separate from guesses.

## What you will set up

- [Project instructions and template variables](../usage/03-configuration.md) in `AGENTS.md`.
- [Variable inspection](../reference/01-cli-reference.md) with `octomind vars`.
- [A custom role](../usage/06-roles.md) with a [local file-reading tool](../usage/17-local-tools.md).
- [Context and model inspection](../reference/02-session-commands.md) with `/context` and `/info`.
- [A named session](../usage/05-sessions.md) you can resume during onboarding.

## Prerequisites

Use Bash on macOS or Linux, from the root of the repository you want to understand. The repository must already have
tracked source files and a `README.md`. Check your tools and login:

```bash
octomind --version
octomind login
git --version
bash --version
command -v nl cat chmod
```

Confirm the checkout and README before starting. `git ls-files` should print source paths:

```bash
git status --short
git ls-files
test -s README.md
```

You need permission to send this repository's content to the configured provider. For a local provider setup, use
[Keep private code in local model sessions](18-private-code-with-local-models.md) first. The following walkthrough uses
your existing Octomind configuration and credentials.

## Steps

### 1. Inspect the context Octomind can collect

Run the variable commands at the repository root. `{{README}}` comes from `README.md` in the working directory.
`{{GIT_TREE}}` is built from Git's tracked file list; it does not include untracked source files or their contents.

The expanded view should contain your README text and recognizable tracked paths. Resolve a wrong directory or empty
README before continuing.

```bash
octomind vars
octomind vars --expand
```

### 2. Save the onboarding instructions

Save this block in `<repository-root>/AGENTS.md`. If the file already exists, append the block in your editor while
preserving the team's existing instructions. Save it before opening the new session.

Octomind loads `AGENTS.md` as an instructions message at session start and expands its variables. Keeping the goal and
evidence rules here avoids repeating them in every question. These instructions guide the model; they do not enforce a
filesystem permission boundary.

```markdown
Onboarding instructions:

Explain this repository to a developer who has not worked on it before.
Do not edit files or run builds, tests, migrations, or deployment commands.
Use read_project_file to inspect tracked text files when you need source evidence.
Cite the path and line number for implementation claims.
Separate README claims, source observations, and unverified behavior.
If you cannot establish a fact, say what file or runtime check would settle it.
Explain unfamiliar project terms when you first use them.

Tracked project files:
{{GIT_TREE}}

Project overview:
{{README}}
```

### 3. Give the role a way to read source

Create `<repository-root>/.agents/tools/read_project_file`. This wrapper checks that a path is tracked before reading
it and adds line numbers for citations. Its only argument is a path relative to the project root.

Keep this tool for source text. A huge generated file or binary is not useful onboarding context. The wrapper is a
convenient reader, not a security sandbox; tracked symlinks can refer elsewhere.

```bash
mkdir -p .agents/tools
cat > .agents/tools/read_project_file <<'EOF'
#!/usr/bin/env bash
# @description Read a tracked text file with line numbers to support repository explanations.
# @param *path string Path of a tracked text file relative to the repository root.

set -euo pipefail
path="${OCTOMIND_PARAM_PATH:?path is required}"
git ls-files --error-unmatch -- "$path" >/dev/null
nl -ba "$OCTOMIND_WORKDIR/$path"
EOF
chmod +x .agents/tools/read_project_file
```

### 4. Save a role for evidence-based explanations

Locate the active config directory. This command handles both the default data directory and an explicit
`OCTOMIND_CONFIG_PATH`. Run it in the shell you will use for the remaining commands:

```bash
tutorial_config_file="${OCTOMIND_CONFIG_PATH:-${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/config/config.toml}"
tutorial_config_dir="$(dirname "$tutorial_config_file")"
printf '%s\n' "$tutorial_config_dir/onboarding.toml"
```

Save the following complete role declaration at the printed `onboarding.toml` path. Use a new file; if you already have
one with that name, choose another unused `.toml` filename in the same directory. Octomind merges these files with the
main config. The `core` reference uses a shipped builtin server and keeps the MCP tool-list path active for discovery.

```toml
[[roles]]
name = "onboarding"
system = """
Help a new teammate understand the current repository.
Read source before explaining implementation details.
Use short explanations and cite file paths with line numbers.
Follow the project onboarding instructions.
"""
welcome = "Ask about one execution path. Working directory: {{CWD}}"

[roles.mcp]
server_refs = ["core"]
allowed_tools = ["core:*"]
```

### 5. Open a named session and inspect its initial context

Validate the merged configuration, then start the session. Use a new name for your first walkthrough:

```bash
octomind config --validate
octomind run onboarding --name legacy-map
```

### 6. Check what the agent actually received

Type each command separately. `/info` identifies the `onboarding` role, model, and session name. `/context user` should
contain the expanded instructions, including README content and the tracked tree. `/mcp full` should contain
`read_project_file` with a required string `path` argument.

The context display inspects stored messages. It does not read new files just because you ask to see context.

```text
/info
/context user
/mcp full
```

### 7. Ask for the smallest useful repository map

Start with orientation instead of asking the agent to explain every file. Send this message and wait for the answer.
You want three paths you can inspect next, with a clear distinction between what the README says and what source proves.

```text
I am new to this repository. From the README and tracked tree, explain its purpose in five sentences.
Then identify three files worth reading first and explain why. Label filename-based guesses as guesses.
Use read_project_file to inspect the most likely entry point before describing how execution begins.
```

### 8. Follow one execution path and challenge the explanation

Send these questions one at a time, waiting for each answer. The first narrows the scope; the second checks a branch
that an overview often misses; the third connects the explanation to tests.

The repository determines the answers. Do not accept invented filenames or a claim that tests passed when this session
has only read their source.

```text
Choose one common operation shown by the entry point. Read its implementation and follow it to its result.
Explain the callers, state changes, and return value. Cite path:line for every link in the chain.
What happens when that operation receives invalid input? Read the relevant branch before answering.
Which tracked tests cover this operation? Read their assertions and distinguish coverage from a passing test run.
```

### 9. Save the stopping point and resume it

Ask for a compact handoff in the conversation, then exit. Wait for the handoff before typing `/exit`:

```text
Summarize the execution path we established, the source files we read, and three unresolved questions.
Suggest the next file to read. Do not modify files or execute tests.
/exit
```

Resume from the same repository root. Omitting the role on resume preserves the session's saved role:

```bash
octomind run --resume legacy-map
```

## Verify it works

In the resumed session, inspect its identity and recorded tool results:

```text
/info
/context tool
```

Look for session `legacy-map`, role `onboarding`, and file-reading results from the earlier discussion. Ask one follow-up
about the selected path; the existing handoff should still be available in the conversation. Check one cited line
against the actual file in your editor before using the explanation to make a change.

## Variations

- **Another subsystem.** Start `octomind run onboarding --name billing-map` and focus on a different operation. A name
  already in use resumes that conversation instead of starting a blank one.
- **Shorter startup context.** Keep the README in `AGENTS.md` and remove `{{GIT_TREE}}` when the tracked tree is too
  large. Supply a small set of relevant paths in your first question.
- **Return to recent work.** Use `octomind run --resume-recent` from the same working directory, or bare
  `octomind run --resume` for the interactive picker.

## Troubleshooting

**The tree is empty or misses a file.** `{{GIT_TREE}}` uses `git ls-files`. Check that you are in the intended checkout
and that the file is tracked. An untracked file needs to be supplied separately; the reader also requires tracked paths.

**New instructions do not appear.** Save `AGENTS.md` at the session working-directory root and start a fresh named
session. Do not assume editing the file rewrites an existing conversation. Use `/context user` to verify the loaded text.

**The agent cannot read source.** Inspect `/mcp full`, check the wrapper's executable bit, and confirm the `onboarding`
role still references the shipped `core` server. A missing tool call is different from a tool call that failed because
the requested path is untracked.

**The explanation sounds certain but lacks evidence.** Ask the agent to read the cited file and quote the relevant
lines. A README describes intended behavior, and a test body describes an assertion; neither proves a current runtime
result. Keep unsupported conclusions in the handoff's unresolved questions.

## See also

- [Configuration](../usage/03-configuration.md)
- [CLI Reference](../reference/01-cli-reference.md)
- [Roles](../usage/06-roles.md)
- [Local Tools](../usage/17-local-tools.md)
- [Session Commands](../reference/02-session-commands.md)
- [Sessions](../usage/05-sessions.md)
