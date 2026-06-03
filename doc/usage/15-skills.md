# Skills

Skills are reusable instruction packs that inject domain knowledge into AI sessions on demand. They follow the [AgentSkills specification](https://agentskills.io/specification) and are distributed via taps.

## How Skills Work

A skill is a directory containing a `SKILL.md` file (frontmatter metadata + instruction body). When activated, the skill's full content is injected into the session context, giving the AI domain-specific knowledge.

> New to taps? Skills are most commonly distributed via taps. See [Tap System](../integration/04-tap-system.md) for how to add one (`octomind tap <org/repo>`) before any tap skill becomes available.

### Skill Locations

Octomind discovers skills from three locations, scanned in this order (**first-wins** deduplication by skill `name` — tap skills shadow universal ones with the same name):

1. **Taps** (highest priority) — `<tap>/skills/<name>/SKILL.md`
2. **Project universal dir** — `<workdir>/.agents/skills/<name>/SKILL.md`
3. **Global universal dir** — `~/.config/agents/skills/<name>/SKILL.md`

The two universal directories follow the open `npx skills` ecosystem layout, so a skill pack dropped into either path works without a tap.

### Three Activation Methods

**1. Environment variable** — preload skills at session start:
```bash
OCTOMIND_SKILLS=programming-rust,git-workflow octomind run developer:general
```

**2. Auto-activation** — skills with declarative `rules:` in frontmatter activate based on project context (e.g., `Cargo.toml` detected, user mentions "rust"). Auto-activation requires **both** a non-empty `rules:` list **and** a `domains:` entry matching the current agent's role — `rules:` alone is not enough (skills without a matching domain are never placed in the activation pool). Auto-activation also only runs on fresh user messages and skips already-active skills.

**3. Manual** — via the `/skill` command or the `skill` MCP tool:
```
/skill programming-rust                  # interactive command (toggles on/off by name)
skill(action="use", name="...")          # MCP tool (AI-initiated)
```

### Skill Directory Structure

```
<name>/
  SKILL.md      # Required: metadata (frontmatter) + instructions (body)
  validate      # Optional: validation script (exit 0 = valid, stderr = error)
  scripts/      # Optional: executable scripts the skill references
  references/   # Optional: supplementary documentation
  assets/       # Optional: templates, config files, resources
```

When a skill is activated, any files in `scripts/`, `references/`, and `assets/` are listed (with their absolute paths) in a `## Skill Resources` section appended to the injected skill block, so the AI can open them on demand via `shell`/`view`.

## SKILL.md Format

### Frontmatter

```yaml
---
name: programming-rust
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

Octomind's parser reads exactly these keys: `name`, `description`, `compatibility`, `license`, `allowed-tools`, `capabilities`, `domains`, and `rules`. Any other key (including the AgentSkills-spec `title`) is silently ignored — adding it does nothing. The skill is loaded only if **both `name` and `description` are present**; if either is missing, the skill is skipped entirely.

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Lowercase, hyphens. Used as the skill identifier. Must equal the directory name to be activatable by name (`use`/`forget`/auto-activation look up `skills/<name>/` and verify the frontmatter `name` matches). The `/skill` and `skill` list views use the frontmatter `name` regardless of directory. |
| `description` | yes | What the skill does and when to use it. |
| `capabilities` | no | Capabilities to auto-load when skill activates. Space-delimited or `["git", "memory"]`. |
| `domains` | no | Agent categories for auto-activation scoping. Without this, skill is manual-only. |
| `allowed-tools` | no | Space-delimited pre-approved tools. Also drives a live compatibility check against the current session tool map: `/skill` list flags ` ⚠️ [missing tools: ...]` and `use` appends a warning if any declared tool is still unavailable after capability loading. |
| `license` | no | License name (e.g., `Apache-2.0`). |
| `compatibility` | no | Free-text environment requirements (shown in the skill list). |
| `rules` | no | Declarative activation rules. Each `- ` line is an OR-group; space-separated checks within a line are AND. Empty = manual-only. |

> Octomind performs **no length validation** on any frontmatter field. The 64/60/1024/500-character limits in the [AgentSkills specification](https://agentskills.io/specification) are author guidance, not enforced by Octomind — the only enforced rule is the presence of `name` and `description`.

### Body

The body after frontmatter is the skill's instructions. Structure: Conventions, Tooling, Best Practices, Examples.

## Declarative Activation Rules

The `rules:` field in SKILL.md frontmatter defines when a skill should auto-activate. Rules are evaluated in-process (no script spawning) on each fresh user message that carries enough intent (see [Activation Gating](#activation-gating) below).

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
| `semantic` | `semantic(phrase)` or `semantic(phrase, threshold)` | BGE-small embedding cosine match of `phrase` against the user message (via `muvon/octomind-embed`). Fires when cosine ≥ threshold (default `0.45`). Example: `semantic(deploying to production)` matches paraphrases like "ship to prod" that `content`/`match` would miss. |

### Evaluation Context

- **`file`**, **`grep`**, **`workdir`** — evaluated against the project working directory.
- **`content`**, **`match`**, **`semantic`** — evaluated against the user's message text.
- **`env`** — evaluated against environment variables.
- **`bin`** — evaluated against the system PATH.
- **`session`** — evaluated against the current session name.
- Already-active skills are skipped.

### Activation Gating

Auto-activation does not run on every message — it is gated to avoid expensive false-positive MCP server loads:

- **Intent gate** — the user message must have **at least 8 non-whitespace characters** (after XML stripping). Short acknowledgments like `try`, `ok`, `do it`, or `fix bug` never trigger any skill (or capability) auto-activation.
- **XML-block stripping** — `<tag>...</tag>` blocks (injected `<skill>` content, `<validation>` feedback, log pastes, system tags) are removed from the message before any `content`/`match`/`semantic` evaluation, so injected context cannot trigger false positives.
- **Semantic abstain-on-tie** — when a skill matches *only* via `semantic(...)` checks, it must win by a margin of **0.08** cosine over the next-best semantic candidate across the activation pool; if two skills are near-tied, **neither** activates. Skills that match via any deterministic check (`file`/`content`/`grep`/`match`/etc.) bypass this margin gate. `semantic` checks evaluate to `false` when the embedding model isn't ready.

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
- Each name is validated against available skills across all locations (taps and the universal `.agents/skills` / `~/.config/agents/skills` dirs)
- Unknown skill names are skipped with a warning
- Already-active skills are not re-injected

## Validate Script

An executable script at `skills/<name>/validate` that checks LLM output quality deterministically.

**Protocol:**
- Runs only on the final assistant message (end of turn)
- The script file must be executable (`chmod +x`); a non-executable script simply fails to spawn (it is run directly via `Command::new(<path>)`)
- `argv[1]` = `"assistant"` (always — the script receives the assistant's response)
- `stdin` = the assistant message content
- Runs in the project working directory
- **exit 0** = output is valid (also resets the per-skill retry counter)
- **failure (retry + LLM feedback)** requires **non-zero exit AND non-empty captured output**. The captured output is stderr, or stdout when stderr is empty. A non-zero exit that produces no stderr/stdout output yields **no feedback** and does **not** increment the retry counter — so don't write a silent `exit 1` and expect the model to be corrected.

**Example:**
```bash
#!/usr/bin/env bash
set -euo pipefail
[ -f Cargo.toml ] || exit 0
cargo clippy --quiet --all-targets -- -D warnings 2>&1
```

If clippy fails, its output is pushed back to the model wrapped in a `<validation>` block:

```
<validation skill="programming-rust">
Validation failed: <clippy output>
Please fix the issue.
</validation>
```

The model gets another turn to fix the issue. Retries are capped by `max_retries` in `[skills]` config.

## Capabilities Auto-Loading

When a skill declares `capabilities: git memory`, activating the skill automatically:

1. Resolves each capability from taps. A capability is split across two files in `<tap>/capabilities/<name>/`:
   - `config.toml` — capability-level metadata: a **required** `triggers = [...]` array and an optional `domains = [...]` binding.
   - `<provider>.toml` — provider-specific MCP wiring (`deps`, `server_refs`, `allowed_tools`, `mcp.servers`).
2. Loads the backing MCP servers via the dynamic server manager
3. Updates the tool map so tools become available

When a skill is later forgotten, its capabilities' MCP servers are offloaded — but a shared server is only fully shut down when the **last** active capability referencing it is removed (refcounted; otherwise only that capability's tools are stripped from the tool map).

Users can override the provider backing a capability in config:
```toml
[capabilities]
memory = "octobrain"
codesearch = "octocode"
```

### Semantic Capability Auto-Activation

Independently of skills, capabilities can auto-activate from their own `triggers` when the master switch is on:

```toml
auto_capabilities = true   # root config field (enabled by default)
```

When enabled, the user message is matched against every capability's `triggers` using BGE-small embeddings:

- **Cosine floor** `0.45` and **abstain-on-tie margin** `0.08` (same gating as semantic skill rules)
- Score is the **mean of the top-3** matching trigger phrases per capability
- At most **4 capabilities** stay active at once; a new activation that would exceed this evicts the least-recently-used capability first
- The same intent gate (≥ 8 non-whitespace chars) and domain filtering apply

### OCTOMIND_CAPABILITIES (boot-time activation)

Force-activate capabilities at session start (comma-delimited), bypassing semantic matching but still subject to domain gating:

```bash
OCTOMIND_CAPABILITIES=cron,docker octomind run -r developer:general
```

This gives long-running or resumed sessions a deterministic tool surface. Capabilities that fail to load (or that aren't available in the current role's domain) are reported on stderr and skipped.

## /skill Command

List or toggle skills interactively during a session:

```
/skill                     # list all skills (active first, then alphabetical), page 1
/skill <page>              # list a specific page (numeric arg, 15 skills per page)
/skill *pattern*           # filter the list by glob pattern (matches name OR description)
/skill <name>              # toggle: enable if inactive, disable if active
```

Tab completion suggests available skill names after `/skill `.

### `skill` MCP tool

The AI-facing `skill` tool exposes three actions: `list`, `use`, and `forget`. The `list` action accepts optional `pattern` (substring filter on name/description), `offset`, and `limit` (default `20`) parameters for pagination.

## Configuration

Required `[skills]` section in config (shipped in the default template — each field has a default value, but the section itself must be present, otherwise config loading fails):

```toml
[skills]
auto_activation = true       # enable/disable auto-activation via declarative rules
auto_validation = false      # enable/disable auto-validation via validate scripts (default: false)
activation_timeout = 3      # reserved (rules are in-process, no timeout needed)
validation_timeout = 60      # seconds per validate script, 0 = unlimited
max_retries = 3              # max validation retries per skill before giving up
```

All fields have defaults in `config-templates/default.toml`, where `auto_validation` ships as `false`. However, the hardcoded in-process fallback used when **no session config is loaded** defaults `auto_validation` to `true`. To get deterministic behavior across all code paths, set `auto_validation` explicitly rather than relying on the omitted-section default.

## Authoring Checklist

There is no shipped lint tool. When authoring a skill, verify by hand:

- `SKILL.md` has valid frontmatter with both `name` and `description` (missing either silently drops the skill).
- The frontmatter `name` matches the skill's directory name (required for `use`/`forget`/auto-activation).
- If you ship a `validate` script, make it executable (`chmod +x`) — a non-executable script fails to spawn at end-of-turn.
- A `validate` script that should correct the model must exit non-zero **and** write to stderr/stdout; a silent `exit 1` produces no feedback.
