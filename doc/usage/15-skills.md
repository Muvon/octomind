# Skills

Skills are reusable instruction packs that inject domain knowledge into AI sessions on demand. They follow the [AgentSkills specification](https://agentskills.io/specification) and are distributed via taps.

## How Skills Work

Skills are stored in taps at `skills/<name>/SKILL.md`. When activated, the skill's full content is injected into the session context, giving the AI domain-specific knowledge.

### Three Activation Methods

**1. Environment variable** — preload skills at session start:
```bash
OCTOMIND_SKILLS=programming-rust,git-workflow octomind run developer:general
```

**2. Auto-activation** — skills with declarative `rules:` in frontmatter activate based on project context (e.g., `Cargo.toml` detected, user mentions "rust"). Scoped by `domains` field to the current agent's role.

**3. Manual** — via `/skill` command or the `skill` MCP tool:
```
/skill use programming-rust        # CLI command
skill(action="use", name="...")     # MCP tool (AI-initiated)
```

### Skill Directory Structure

```
skills/<name>/
  SKILL.md      # Required: metadata (frontmatter) + instructions (body)
  validate      # Optional: validation script (exit 0 = valid, stderr = error)
  scripts/      # Optional: executable scripts the skill references
  references/   # Optional: supplementary documentation
  assets/       # Optional: templates, config files, resources
```

## SKILL.md Format

### Frontmatter

```yaml
---
name: programming-rust
title: "Rust Development"
description: "Rust conventions, idiomatic patterns, and cargo tooling. Auto-activates in Rust projects."
license: Apache-2.0
compatibility: "Requires cargo and rustc."
capabilities: programming-rust       # capabilities to auto-load (space-delimited or array)
domains: developer                   # agent categories that check this skill
allowed-tools: shell text_editor     # pre-approved tools
rules:                                # declarative activation rules
  - file(Cargo.toml)
  - content(rust)
  - content(rust) file(Cargo.toml)
---
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Max 64 chars. Lowercase, hyphens. Must match directory name. |
| `title` | yes | 5-60 chars. Human-readable label. |
| `description` | yes | 20-1024 chars. What the skill does and when to use it. |
| `capabilities` | no | Capabilities to auto-load when skill activates. Space-delimited or `["git", "memory"]`. |
| `domains` | no | Agent categories for auto-activation scoping. Without this, skill is manual-only. |
| `allowed-tools` | no | Space-delimited pre-approved tools. |
| `license` | no | License name (e.g., `Apache-2.0`). |
| `compatibility` | no | Max 500 chars. Environment requirements. |
| `rules` | no | Declarative activation rules. Each `- ` line is an OR-group; space-separated checks within a line are AND. Empty = manual-only. |

### Body

The body after frontmatter is the skill's instructions. Structure: Conventions, Tooling, Best Practices, Examples.

## Declarative Activation Rules

The `rules:` field in SKILL.md frontmatter defines when a skill should auto-activate. Rules are evaluated in-process (no script spawning) on every user message.

### Logic

- Each `- ` line is an **OR-group** — if **any** group matches, the skill activates.
- Space-separated checks within a line are **AND** — **all** must match for the group to activate.
- Empty `rules:` (or omitted) = manual-only skill.

```yaml
rules:
  - file(Cargo.toml)                    # OR: Cargo.toml exists
  - content(rust)                       # OR: user mentions "rust"
  - content(rust) file(Cargo.toml)      # OR: user mentions "rust" AND Cargo.toml exists
```

### Check Types

| Check | Syntax | Description |
|-------|--------|-------------|
| `file` | `file(pattern)` | File or glob exists in working directory. Example: `file(Cargo.toml)`, `file(*.go)` |
| `content` | `content(word)` | Case-insensitive word-boundary match against user message. Example: `content(rust)` matches "rust" but not "thrust" |
| `grep` | `grep(pattern)` or `grep(pattern, glob)` | Search file contents in working directory (respects .gitignore). Example: `grep(fn main)`, `grep(TODO, *.rs)` |
| `env` | `env(VAR)` or `env(VAR=val)` | Environment variable is set and non-empty, or equals a specific value. Example: `env(CI)`, `env(CI=true)` |
| `match` | `match(regex)` | Regex match against user message content. Example: `match(\brust\b)` |
| `bin` | `bin(name)` | Executable is findable on PATH (case-sensitive). Example: `bin(cargo)`, `bin(node)` |
| `session` | `session(pattern)` | Case-insensitive substring match on current session name. Example: `session(octomind)` matches "260421-octomind-a1b2" |
| `workdir` | `workdir(pattern)` | Case-insensitive substring match on working directory path. Example: `workdir(rust)` matches "/home/dev/rust-project" |

### Evaluation Context

- **`file`**, **`grep`**, **`workdir`** — evaluated against the project working directory.
- **`content`**, **`match`** — evaluated against the user's message text.
- **`env`** — evaluated against environment variables.
- **`bin`** — evaluated against the system PATH.
- **`session`** — evaluated against the current session name.
- Rules are checked on every user message. Already-active skills are skipped.

### Domain Scoping

The `domains` field limits which agents evaluate this skill's rules:

```yaml
domains: developer devops
```

- Only agents with matching role names evaluate this skill's rules
- Reduces the activation pool to relevant skills only
- Skills without `domains` are manual-only (backward compatible)

## Environment Variable: OCTOMIND_SKILLS

Preload skills at session start. Comma-delimited skill names:

```bash
export OCTOMIND_SKILLS=programming-rust,git-workflow
octomind run developer:general
```

- Skills are activated immediately as permanent skills
- Declarative rules are not evaluated for env-loaded skills
- Each name is validated against available skills in taps
- Unknown skill names are skipped with a warning
- Already-active skills are not re-injected

## Validate Script

An executable script at `skills/<name>/validate` that checks LLM output quality deterministically.

**Protocol:**
- Runs only on the final assistant message (end of turn)
- `argv[1]` = `"assistant"` (always — the script receives the assistant's response)
- `stdin` = the assistant message content
- Runs in the project working directory
- **exit 0** = output is valid
- **exit non-zero** = output is invalid. stderr (or stdout if stderr empty) is captured and fed back to the LLM as an error message for correction.

**Example:**
```bash
#!/usr/bin/env bash
set -euo pipefail
[ -f Cargo.toml ] || exit 0
cargo clippy --quiet --all-targets -- -D warnings 2>&1
```

If clippy fails, its output is sent back to the LLM: "Validation failed (programming-rust): <clippy output>". The LLM gets another turn to fix the issue. Retries are capped by `max_retries` in `[skills]` config.

## Capabilities Auto-Loading

When a skill declares `capabilities: git memory`, activating the skill automatically:

1. Resolves each capability from taps (`capabilities/<name>/<provider>.toml`)
2. Loads the backing MCP servers via the dynamic server manager
3. Updates the tool map so tools become available

Users can override providers in config:
```toml
[capabilities]
memory = "octobrain"
codesearch = "octocode"
```

## /skill Command

Toggle skills interactively during a session:

```
/skill                     # list all skills with active status
/skill <name>              # toggle: enable if inactive, disable if active
```

Tab completion suggests available skill names after `/skill `.

## Configuration

Required `[skills]` section in config:

```toml
[skills]
auto_activation = true       # enable/disable auto-activation via declarative rules
auto_validation = false      # enable/disable auto-validation via validate scripts (default: false)
activation_timeout = 3      # reserved (rules are in-process, no timeout needed)
validation_timeout = 60      # seconds per validate script, 0 = unlimited
max_retries = 3              # max validation retries per skill before giving up
```

All fields have defaults in `config-templates/default.toml`. The `[skills]` section can be omitted from user config if defaults are acceptable.

## Lint Validation

`validate` scripts must be executable (`chmod +x`). The tap lint script checks this:

```bash
bash scripts/lint-skills.sh
```
