# Tap System

Taps are Homebrew-style registries for distributing Octomind agents, skills, and capabilities.

## Overview

A tap is a Git repository (or local directory) containing:

```
agents/
  category/
    variant.toml    # Agent manifest
deps/
  [dependency definitions]
skills/
  skill-name/
    SKILL.md        # Skill instructions
    scripts/        # Executable scripts
    references/     # Documentation files
    assets/         # Static files
capabilities/
  capability-name/
    provider.toml   # Provider-specific capability config
```

## Managing Taps

### List Taps

```bash
octomind tap
```

### Add a Tap

```bash
# From GitHub
octomind tap myorg/my-agents

# From local directory (symlink)
octomind tap myorg/my-agents /path/to/local/tap
```

### Remove a Tap

```bash
octomind untap myorg/my-agents
```

### Built-in Tap

The default tap `muvon/tap` is always active and ships production-ready agents.

## Tap Priority

When multiple taps provide the same agent tag, priority is:
1. User-added taps (in order added)
2. Built-in default tap (`muvon/tap`)

## Using Tap Agents

### From the CLI

Run a tap agent with `domain:spec` format:

```
octomind run octomind:assistant
octomind run developer:general
```

When you specify a tag with `:`, Octomind:
1. Searches taps for a matching agent manifest
2. Downloads and resolves dependencies
3. Merges the manifest into config
4. Starts the session with the agent's role

### From within a session — the `tap` core tool

Inside a running session you can launch a tap role as a subagent via the `tap` core tool. Same role catalog, but invoked from the LLM mid-conversation rather than a CLI command:

```json
{"action": "discover", "intent": "review a legal contract"}
{"action": "run", "role": "lawyer:sg", "prompt": "What are the notice period rules?"}
{"action": "list"}
{"action": "stop", "session": "tap-lawyer-sg-9b2c1d"}
```

Use `background: true` for long tasks; the reply lands as a user message in the next turn. See [MCP Tools — `tap`](../usage/07-mcp-tools.md#tap----run-specialist-roles-from-taps) for the full schema.

### Model Overrides

Set a preferred model for specific tap agents in your config:

```toml
[taps]
"developer:general" = "ollama:glm-5"
```

This overrides the model for `octomind run developer:general` while leaving other agents unchanged.
**Priority:** CLI `--model` > `[taps]` override > role.model > config.model

## Agent Manifests


Agent manifests are TOML files in `agents/<category>/<variant>.toml`. The header comment block is part of the schema — `# Title:` and `# Description:` are required (used by `tap discover` and `octomind run` autocomplete):

```toml
# agents/developer/general.toml
# Agent: developer:general
# Title: General Developer
# Description: Elite senior developer. Pragmatic, precise, zero waste.

[[roles]]
system = "You are an expert software developer..."
temperature = 0.3

[roles.mcp]
server_refs = ["core", "runtime", "filesystem"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*"]
```

The role's `name` is auto-injected from the file path (`category:variant` becomes the name) — manifests don't need to declare it. Manifests can include any config sections: roles, layers, workflows, MCP servers, etc.

If the role needs runtime harness control (`mcp` / `agent` / `skill` tools), include `"runtime"` in `server_refs`. Most roles only need `"core"` (planning, scheduling, capability, tap) plus the data servers they actually use.

## Skills

Skills are reusable instruction packs managed via the `skill` MCP tool. Auto-activation uses declarative `rules:` in SKILL.md frontmatter — see [Skills](../usage/15-skills.md) for full documentation.

### Skill Structure

```
skills/code-review/
  SKILL.md              # Instructions (injected into context)
  validate              # Optional: validation script
  scripts/
    lint.sh             # Executable scripts
    test.sh
  references/
    style-guide.md      # Documentation for AI to read
    patterns.md
  assets/
    config.json         # Static files
```

### Using Skills in Session

```
# Discover skills
skill(action="list")

# Activate a skill
skill(action="use", name="code-review")

# Deactivate (triggers compression to clean up)
skill(action="forget", name="code-review")
```

When activated, the skill's `SKILL.md` is injected into context, and a resource catalog lists all available scripts, references, and assets with absolute paths.

## Dependencies

Agent manifests can declare dependencies:

```toml
dependencies = ["octocode", "octofs"]
```

Dependencies are resolved and installed automatically on first run.

## Capability Overrides

Taps provide capability implementations. Override which provider is used:

```toml
# In config.toml
[capabilities]
codesearch = "octocode"
```

This maps to `capabilities/codesearch/octocode.toml` within the tap, allowing different implementations of the same capability.

## Storage

Taps are stored in `~/.local/share/octomind/taps/`:

```
taps/
  user/
    myorg-my-agents/     # Git clone or symlink
  muvon/
    octomind-tap/        # Built-in default
```
