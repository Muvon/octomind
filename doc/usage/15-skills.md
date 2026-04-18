# Skills

Skills are reusable instruction packs that inject domain knowledge into AI sessions on demand. They follow the [AgentSkills specification](https://agentskills.io/specification) and are distributed via taps.

## How Skills Work

Skills are stored in taps at `skills/<name>/SKILL.md`. When activated, the skill's full content is injected into the session context, giving the AI domain-specific knowledge.

### Three Activation Methods

**1. Environment variable** — preload skills at session start:
```bash
OCTOMIND_SKILLS=programming-rust,git-workflow octomind run developer:general
```

**2. Auto-activation** — skills with `activate` scripts activate based on project context (e.g., `Cargo.toml` detected). Scoped by `domains` field to the current agent's role.

**3. Manual** — via `/skill` command or the `skill` MCP tool:
```
/skill use programming-rust        # CLI command
skill(action="use", name="...")     # MCP tool (AI-initiated)
```

### Skill Directory Structure

```
skills/<name>/
  SKILL.md      # Required: metadata (frontmatter) + instructions (body)
  activate      # Optional: auto-activation script (exit 0 = activate)
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

### Body

The body after frontmatter is the skill's instructions. Structure: Conventions, Tooling, Best Practices, Examples.

## Environment Variable: OCTOMIND_SKILLS

Preload skills at session start. Comma-delimited skill names:

```bash
export OCTOMIND_SKILLS=programming-rust,git-workflow
octomind run developer:general
```

- Skills are activated immediately as permanent skills
- No `activate` scripts are run for env-loaded skills
- Each name is validated against available skills in taps
- Unknown skill names are skipped with a warning
- Already-active skills are not re-injected

## Activate Script

An executable script at `skills/<name>/activate` that decides whether the skill should be active.

**Protocol:**
- `argv[1]` = event type: `user` | `assistant` | `turn`
- `stdin` = event content (user message, assistant response, or turn summary)
- Runs in the project working directory
- **exit 0** = activate this skill
- **exit non-zero** = don't activate (or deactivate if currently active)
- Already-active skills are skipped (no script executed)

**Events:**
- `user` — real user typed input (not auto-injected)
- `assistant` — assistant finished responding, awaiting user
- `turn` — tool execution done, ready for next loop. Content includes tool names + params.

**Example:**
```bash
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  user) grep -qi '\brust\b\|cargo\b' && exit 0 ;;
  *)    [ -f Cargo.toml ] && exit 0 ;;
esac
exit 1
```

## Validate Script

An executable script at `skills/<name>/validate` that checks LLM output quality deterministically.

**Protocol:**
- Runs at the end of each assistant turn
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

## Domain Scoping

The `domains` field limits which agents check this skill's `activate` script:

```yaml
domains: developer devops
```

- Only agents with matching role names run this skill's `activate` script
- Reduces the activation pool to relevant skills only
- Skills without `domains` are manual-only (backward compatible)

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
auto_activation = true       # enable/disable auto-activation
activation_timeout = 3        # seconds per activate script, 0 = unlimited
validation_timeout = 60       # seconds per validate script, 0 = unlimited
max_retries = 3               # max validation retries per skill before giving up
```

All fields are required. The `[skills]` section must be present in the config file.

## Lint Validation

Both `activate` and `validate` scripts must be executable (`chmod +x`). The tap lint script checks this:

```bash
bash scripts/lint-skills.sh
```
