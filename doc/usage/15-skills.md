# Skills

Skills are reusable instruction packs that inject domain knowledge into AI sessions on demand. They follow the [AgentSkills specification](https://agentskills.io/specification) and are distributed via taps.

## How Skills Work

Skills are stored in taps at `skills/<name>/SKILL.md`. When activated, the skill's full content is injected into the session context, giving the AI domain-specific knowledge.

### Manual Activation

The AI can activate skills via the `skill` MCP tool:

```
skill(action="list")                        # discover available skills
skill(action="use", name="git-workflow")    # inject skill into context
skill(action="forget", name="git-workflow") # remove skill from context
```

### Auto-Activation

Skills can declare an `activate` script that runs on conversation events. When the script returns exit 0, the skill is automatically activated — no AI decision needed.

```
skills/rust-dev/
  SKILL.md      # metadata + instructions
  activate      # executable script — decides when to activate
  validate      # executable script — validates LLM output quality
```

The system scans skills matching the current agent's domain and runs their `activate` scripts on each event.

## SKILL.md Format

### Frontmatter

```yaml
---
name: rust-dev
title: "Rust Development"
description: "Rust development conventions and validation."
license: Apache-2.0
compatibility: "Requires cargo and rustc."
capabilities: git memory              # capabilities to auto-load (space-delimited or array)
domains: developer devops             # agent categories that check this skill
allowed-tools: shell text_editor      # pre-approved tools (existing)
---
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Max 64 chars. Lowercase, hyphens. Must match directory name. |
| `title` | yes | 5–60 chars. Human-readable label. |
| `description` | yes | 20–1024 chars. What the skill does and when to use it. |
| `capabilities` | no | Capabilities to auto-load when skill activates. Space-delimited or `["git", "memory"]`. |
| `domains` | no | Agent categories for auto-activation scoping. Without this, skill is manual-only. |
| `allowed-tools` | no | Space-delimited pre-approved tools. |
| `license` | no | License name (e.g., `Apache-2.0`). |
| `compatibility` | no | Max 500 chars. Environment requirements. |

### Body

The body after frontmatter is the skill's instructions — what the AI reads and follows. Structure: Overview → Instructions → Examples → References.

## Activate Script

An executable script at `skills/<name>/activate` that decides whether the skill should be active.

**Protocol:**
- `argv[1]` = event type: `user` | `assistant` | `turn`
- `stdin` = event content (user message, assistant response, or turn summary)
- Runs in the project working directory
- **exit 0** = activate this skill
- **exit non-zero** = don't activate (or deactivate if currently active)

**Events:**
- `user` — real user typed input (not auto-injected)
- `assistant` — assistant finished responding, awaiting user
- `turn` — tool execution done, ready for next loop. Content includes tool names + params.

**Example:**
```bash
#!/bin/bash
case "$1" in
  user) grep -qi "rust\|cargo" && exit 0 ;;
  *)    [ -f Cargo.toml ] && exit 0 ;;
esac
exit 1
```

## Validate Script

An executable script at `skills/<name>/validate` that checks LLM output quality.

**Protocol:**
- Same events and invocation as `activate`
- **exit 0** = output is valid
- **exit non-zero** = output is invalid. stderr (or stdout if stderr empty) is captured and fed back to the LLM as an error message.
- The script decides whether to validate based on the event type

**Example:**
```bash
#!/bin/bash
[ "$1" = "turn" ] || exit 0
[ -f Cargo.toml ] && cargo test 2>&1
```

If `cargo test` fails, its output is sent back to the LLM: "Validation failed (rust-dev): <test output>". The LLM gets another turn to fix the issue.

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

- Only agents with matching role names (e.g., `developer:rust`) run this skill's `activate` script
- Reduces the activation pool from hundreds of skills to ~10 relevant ones
- Skills without `domains` are manual-only (backward compatible)

## Configuration

```toml
[skills]
auto_activation = true       # enable/disable auto-activation (default: true)
activation_timeout = 3        # seconds per activate script, 0 = unlimited (default: 3)
validation_timeout = 60       # seconds per validate script, 0 = unlimited (default: 60)
max_retries = 3               # max validation retries per turn (default: 3)
```

## Validation

Both `activate` and `validate` scripts must be executable (`chmod +x`). The lint script checks this:

```bash
bash scripts/lint-skills.sh
```
