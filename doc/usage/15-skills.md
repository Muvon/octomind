# Skills

Use skills to load reusable instructions and supporting resources into a session. This guide covers using skills,
authoring their metadata and activation rules, and configuring optional validators.

## Get started

Create a project skill from the project root, then start a new session so its automatic activation pool includes it:

```bash
mkdir -p .agents/skills/project-review
cat > .agents/skills/project-review/SKILL.md <<'EOF'
---
name: project-review
description: Review project changes with explicit evidence and validation limits.
domains: developer
rules:
  - content(review)
---
Report each finding with its file path, the observed behavior, and the smallest correction.
Separate source inspection from checks you actually ran. Do not claim a test passed without its result.
EOF
octomind run developer:general
```

Activate it manually, or send a fresh request such as `Review the current changes and report concrete findings.`:

```text
/skill
/skill project-review
```

Typing `/skill project-review` again toggles it off. Manual activation does not require matching domains or rules.

## How Skills Work

A skill is a directory containing a `SKILL.md` file (frontmatter metadata + instruction body). When activated, the
skill’s instruction body is injected into the session context, giving the AI domain-specific knowledge.

> New to taps? Skills are most commonly distributed via taps. See [Tap System](../integration/04-tap-system.md) for how
> to add one (for example, register your local tap with `octomind tap myorg/my-skills ./my-skills`) before any tap skill
> becomes available.

### Skill Locations

Octomind discovers skills in this order, with **first-wins** deduplication by frontmatter `name`:

1. **Taps** (highest priority) — `<tap>/skills/<name>/SKILL.md`
2. **Project universal dir** — `<workdir>/.agents/skills/<name>/SKILL.md`
3. **Global universal dir** — `~/.config/agents/skills/<name>/SKILL.md`
4. **Agent plugins** — skills below discovered plugin roots (`plugins/<plugin>/skills/<name>/SKILL.md`)
5. **Generated evolution skills** — eligible machine-local trial/active artifacts, always lowest authority

A skill pack in either universal directory works without a tap. Plugin skills support listing and manual/env loading,
but the automatic activation pool currently scans taps, universal directories, and generated bindings only.

## Configuration

The shipped template provides this required `[skills]` section. All five fields are required in the resolved
configuration; edit the existing section rather than replacing the whole config with a partial snippet:

```toml
[skills]
auto_activation = true       # enable/disable auto-activation via declarative rules
auto_validation = false      # enable/disable auto-validation via validate scripts (default: false)
activation_timeout = 3       # reserved (rules are in-process, no timeout needed)
validation_timeout = 60      # seconds per script; 0 uses a one-hour timeout
max_retries = 3              # per-skill failure cap; 0 disables the cap
```

## Activate skills

**1. Environment variable** — preload skills at session start:
```bash
OCTOMIND_SKILLS=project-review octomind run developer:general
```

**2. Auto-activation** — skills with declarative `rules:` in frontmatter activate based on project context (e.g.,
`Cargo.toml` detected, user mentions "rust"). Auto-activation requires **both** a non-empty `rules:` list **and** a
`domains:` entry matching the current agent's role — `rules:` alone is not enough (skills without a matching domain are
never placed in the activation pool). Automatic user-input activation skips already-active skills; the runtime
capability intent path can also evaluate skill rules.

**3. Manual** — via the `/skill` command or the `skill` MCP tool:
```text
/skill project-review
```

For the AI-facing `skill` MCP tool, use this arguments object:

```json
{"action":"use","name":"project-review"}
```

### Skill Directory Structure

```text
<name>/
  SKILL.md      # Required: metadata (frontmatter) + instructions (body)
  validate      # Optional: validation script (exit 0 = valid, stderr = error)
  scripts/      # Optional: executable scripts the skill references
  references/   # Optional: supplementary documentation
  assets/       # Optional: templates, config files, resources
```

When a skill is activated, immediate files in `scripts/`, `references/`, and `assets/` (not nested descendants) are
listed (with their absolute paths) in a `## Skill Resources` section appended to the injected skill block, so the AI can
open them on demand via `shell`/`view`.

## SKILL.md Format

### Frontmatter

```yaml
---
name: programming-rust
description: "Rust conventions, idiomatic patterns, and cargo tooling. Auto-activates in Rust projects."
license: Apache-2.0
compatibility: "Requires cargo and rustc."
domains: developer
allowed-tools: shell text_editor
rules:
  - file(Cargo.toml)
  - content(rust)
  - content(rust) file(Cargo.toml)
---
```

Octomind's parser reads exactly these keys: `name`, `description`, `compatibility`, `license`, `allowed-tools`,
`capabilities`, `domains`, and `rules`. Any other key (including the AgentSkills-spec `title`) is silently ignored —
adding it does nothing. The skill is loaded only if **both `name` and `description` are present**; if either is missing,
the skill is skipped entirely.

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Use a lowercase hyphenated name by convention. Used as the skill identifier. Must equal the directory name to be activatable by name (`use`/`forget`/auto-activation look up `skills/<name>/` and verify the frontmatter `name` matches). The `/skill` and `skill` list views use the frontmatter `name` regardless of directory. |
| `description` | yes | What the skill does and when to use it. |
| `capabilities` | no | Capabilities to auto-load when skill activates. Space-delimited or `["git", "memory"]`. |
| `domains` | no | Agent categories for auto-activation scoping. Without this, skill is manual-only. |
| `allowed-tools` | no | Space-delimited expected tool names, not permission grants. The MCP `skill` list/use results warn about missing tools; the slash-command list does not show that compatibility warning. |
| `license` | no | License name (e.g., `Apache-2.0`). |
| `compatibility` | no | Free-text environment requirements (shown by the MCP skill list). |
| `rules` | no | Declarative activation rules. Each `- ` line is an OR-group; space-separated checks within a line are AND. Empty = manual-only. |

The parser is a small line-oriented frontmatter reader, not a general YAML loader. Keep metadata on one line; use
space-separated values or inline arrays for `domains` and `capabilities`. Avoid inline comments on values: `domains:
developer # review` would include extra domain tokens. Unknown keys are ignored; name/description lengths and naming
conventions are not validated.

### Body

The body after frontmatter contains the instructions, as in the complete `project-review` example above. Keep actions
concrete and put longer supporting material in `references/` for the agent to read when needed.

## Declarative Activation Rules

The `rules:` field in SKILL.md frontmatter defines when a skill should auto-activate. Rules are evaluated in-process (no
script spawning) on each fresh user message that carries enough intent (see [Activation Gating](#activation-gating)
below).

### Logic

- Each `- ` line is an **OR-group** — if **any** group matches, the skill activates.
- Space-separated checks within a line are **AND** — **all** must match for the group to activate.
- Empty `rules:` (or omitted) = manual-only skill.

```yaml
rules:
  - content(rust) file(Cargo.toml)
  - content(cargo) bin(cargo)
```

### Check Types

| Check | Syntax | Description |
|-------|--------|-------------|
| `file` | `file(pattern)` | File or glob exists in working directory. Example: `file(Cargo.toml)`, `file(*.go)` |
| `content` | `content(word)` | Case-insensitive word-boundary match against user message. Example: `content(rust)` matches "rust" but not "thrust" |
| `grep` | `grep(pattern)` or `grep(pattern, glob)` | Search file contents in working directory (respects .gitignore). Example: `grep(fn main)`, `grep(fn main, *.rs)` |
| `env` | `env(VAR)` or `env(VAR=val)` | Environment variable is set and non-empty, or equals a specific value. Example: `env(CI)`, `env(CI=true)` |
| `match` | `match(regex)` | Regex match against user message content. Example: `match(\brust\b)` |
| `bin` | `bin(name)` | Executable is findable on PATH using the platform lookup rules. Example: `bin(cargo)`, `bin(node)` |
| `session` | `session(pattern)` | Case-insensitive substring match on current session name. Example: `session(octomind)` matches "260421-octomind-a1b2" |
| `workdir` | `workdir(pattern)` | Case-insensitive substring match on working directory path. Example: `workdir(rust)` matches "/home/dev/rust-project" |
| `semantic` | `semantic(phrase)` or `semantic(phrase, threshold)` | Local embedding cosine match of `phrase` against the user message (via `muvon/octomind-embed`). Fires when cosine ≥ threshold (default `0.45`). Example: `semantic(deploying to production)` can match paraphrases when their computed score reaches the threshold. |

### Evaluation Context

- **`file`**, **`grep`**, **`workdir`** — evaluated against the project working directory.
- **`content`**, **`match`**, **`semantic`** — evaluated against the user's message text.
- **`env`** — evaluated against environment variables.
- **`bin`** — evaluated against the system PATH.
- **`session`** — evaluated against the current session name.
- Already-active skills are skipped.

### Activation Gating

Auto-activation does not run on every message — it is gated to avoid expensive false-positive MCP server loads:

- **Intent gate** — the user message must have **at least 8 non-whitespace characters** (after XML stripping). Short
  acknowledgments like `try`, `ok`, `do it`, or `fix bug` never trigger any skill (or capability) auto-activation.
- **XML-block stripping** — `<tag>...</tag>` blocks (injected `<skill>` content, `<validation>` feedback, log pastes,
  system tags) are removed from the message before any `content`/`match`/`semantic` evaluation, so injected context
  cannot trigger false positives.
- **Semantic abstain-on-tie** — when every matching rule group contains a `semantic(...)` check, it must win by a margin
  of **0.08** cosine over the next-best semantic candidate across the activation pool; if two skills are near-tied,
  **neither** activates. A matching group containing no semantic check bypasses this margin gate; adding `file(...)` to
  a semantic group does not bypass it. `semantic` checks evaluate to `false` when the embedding model isn't ready.

### Domain Scoping

The `domains` field limits which agents evaluate this skill's rules:

```yaml
domains: developer devops
```

- Matching uses the role base before `:`: `developer:general` matches `domains: developer`. `domains: "*"` matches any
  domain
- Reduces the activation pool to relevant skills only
- Skills without `domains` are manual-only (backward compatible)

## Environment Variable: OCTOMIND_SKILLS

Preload skills at session start. Values are comma-delimited exact skill names:

```bash
export OCTOMIND_SKILLS=project-review
octomind run developer:general
```

- Skills are loaded at startup and marked as env-loaded; successful `/done` compression omits their injected blocks,
  while automatic compression preserves active skill blocks
- Declarative rules are not evaluated for env-loaded skills
- Each name is validated against available skills across all locations (taps and the universal `.agents/skills` /
  `~/.config/agents/skills` dirs)
- Unknown skill names are skipped with a warning
- No alias, substring, glob, or semantic lookup is applied to `OCTOMIND_SKILLS`
- Already-active skills are not re-injected

## Validate Script

An optional executable script at `<tap>/skills/<name>/validate` checks the assistant response. Automatic validation
currently searches tap directories only: a validator in a universal directory or plugin is not run by this path. Enable
`[skills].auto_validation` to use it.

**Protocol:**

- Runs only on the final assistant message (end of turn)
- On Unix the script must be executable; Windows routes scripts through Git Bash
- `argv[1]` = `"assistant"` (always — the script receives the assistant's response)
- `stdin` = the assistant message content
- Runs in the project working directory
- **exit 0** = output is valid (also resets the per-skill retry counter)
- **failure (retry + LLM feedback)** requires **non-zero exit AND non-empty captured output**. The captured output is
  stderr, or stdout when stderr is empty. A non-zero exit that produces no stderr/stdout output yields **no feedback**
  and does **not** increment the retry counter — so don't write a silent `exit 1` and expect the model to be corrected.

For example, in a local tap you own, install this validator for `project-review`. It checks that the response states its
validation limits and emits actionable feedback on failure:

```bash
mkdir -p my-skills/skills/project-review
cp .agents/skills/project-review/SKILL.md my-skills/skills/project-review/SKILL.md
cat > my-skills/skills/project-review/validate <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if grep -qi 'validation' ; then
  exit 0
fi
printf '%s\n' 'Include a validation statement saying what you inspected or ran.' >&2
exit 1
EOF
chmod +x my-skills/skills/project-review/validate
octomind tap myorg/my-skills ./my-skills
```

Enable validation in the existing config and start a new session:

```toml
[skills]
auto_validation = true
```

```bash
OCTOMIND_SKILLS=project-review octomind run developer:general
```

The model gets another turn to fix the issue. Retries are capped by `max_retries` in `[skills]` config.

## Capabilities Auto-Loading

When a skill declares capability names from your installed taps, for example:

```yaml
capabilities: git memory
```

Activation then:

1. Resolves the named capabilities and selected provider files from taps.
2. Skips capabilities with missing required environment variables while still activating the skill body.
3. Enables backing servers and records the servers loaded by the skill for later release.

Forgetting a skill decrements those server references. A shared server is disabled/removed when the last tracked
reference is released. This is separate from the capability tool’s four-entry LRU and does not run its domain gate or
dependency-only activation path. Plugin skills can also load the plugin’s `mcp.json` servers.

For installed `memory`/`codesearch` capabilities with these provider files, override the provider in config:
```toml
[capabilities]
memory = "octobrain"
codesearch = "octocode"
```

For capability metadata, semantic routing, tap-qualified references, and `OCTOMIND_CAPABILITIES`, see [Token
efficiency](16-token-efficiency.md).

## /skill Command

List or toggle skills interactively during a session:

| Command | Effect |
|---------|--------|
| `/skill` | List active skills first, then alphabetical; 15 per page |
| `/skill 2` | Show page two, if present |
| `/skill *review*` | Filter names/descriptions by a case-insensitive `*` pattern |
| `/skill project-review` | Toggle the exact skill name |

```text
/skill
/skill *review*
/skill project-review
```

Tab completion suggests available skill names after `/skill `.

### `skill` MCP tool

The AI-facing `skill` tool exposes three actions: `list`, `use`, and `forget`. The `list` action accepts optional
`pattern` (substring filter on name/description), `offset`, and `limit` (default `20`) parameters for pagination. Pass
one of these argument objects to the `skill` tool:

```json
{"action":"list","pattern":"review","offset":0,"limit":20}
```

```json
{"action":"forget","name":"project-review"}
```

## Authoring Checklist

There is no shipped lint tool. When authoring a skill, verify by hand:

- `SKILL.md` has valid frontmatter with both `name` and `description` (missing either silently drops the skill).
- The frontmatter `name` matches the skill's directory name (required for `use`/`forget`/auto-activation).
- If you ship a `validate` script, make it executable (`chmod +x`) — a non-executable script fails to spawn at
  end-of-turn.
- A `validate` script that should correct the model must exit non-zero **and** write to stderr/stdout; a silent `exit 1`
  produces no feedback.

## Common questions

**Why did my rule not activate?** Check that the skill is in a supported automatic location, its name matches the
directory, and its domains contain the role base or `*`. Restart after adding a skill, and send a request with at least
eight non-whitespace characters. Use `/skill project-review` to test manual loading independently.

**Why did validation not run?** It is disabled by default and the runner searches taps only. Check the executable bit on
Unix. `validation_timeout = 0` means one hour in the current implementation, not an unlimited wait. Timeouts and spawn
errors are debug-logged; they do not generate model repair feedback.

```text
/loglevel debug
/skill
Review the current changes and include a validation statement.
/loglevel info
```

**Why are tools missing after activation?** `allowed-tools` does not install or authorize tools. Capability loading can
fail or skip missing required environment variables while the skill body still activates. Inspect the AI-facing `skill`
list/use result and your installed capability configuration.

## Source reference

| Surface | Source |
|---------|--------|
| Metadata, discovery, and tool actions | [src/mcp/runtime/skill.rs](../../src/mcp/runtime/skill.rs) |
| Activation, env loading, and validators | [src/mcp/runtime/skill_auto.rs](../../src/mcp/runtime/skill_auto.rs) |
| Slash command | [src/session/chat/session/commands/skill.rs](../../src/session/chat/session/commands/skill.rs) |
| Defaults and configuration | [config-templates/default.toml](../../config-templates/default.toml), [src/config/mod.rs](../../src/config/mod.rs) |
| Compression boundary | [src/session/chat/conversation_compression/mod.rs](../../src/session/chat/conversation_compression/mod.rs) |

## See also

- [Tap system](../integration/04-tap-system.md)
- [MCP tools](07-mcp-tools.md)
- [Cross-session learning](13-learning.md)
- [Token efficiency](16-token-efficiency.md)
- [Configuration reference](../reference/03-config-reference.md)
