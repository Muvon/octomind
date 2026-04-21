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

The default tap `muvon/octomind-tap` is always active and ships production-ready agents.

## Tap Priority

When multiple taps provide the same agent tag, priority is:
1. User-added taps (in order added)
2. Built-in default tap (`muvon/octomind-tap`)

## Using Tap Agents

Run a tap agent with `domain:spec` format:

```bash
octomind run octomind:assistant
octomind run octomind:developer
octomind run developer:rust
```

When you specify a tag with `:`, Octomind:
1. Searches taps for a matching agent manifest
2. Downloads and resolves dependencies
3. Merges the manifest into config
4. Starts the session with the agent's role

### Model Overrides

Set a preferred model for specific tap agents in your config:

```toml
[taps]
"developer:rust" = "ollama:glm-5"
```

This overrides the model for `octomind run developer:rust` while leaving other agents unchanged.

**Priority:** CLI `--model` > `[taps]` override > role.model > config.model

## Agent Manifests


Agent manifests are TOML files in `agents/<category>/<variant>.toml`:

```toml
# agents/developer/rust.toml
[[roles]]
name = "developer"
system = "You are a Rust development expert..."
temperature = 0.3

[roles.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["core:*", "filesystem:*"]
```

Manifests can include any config sections: roles, layers, workflows, MCP servers, etc.

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
