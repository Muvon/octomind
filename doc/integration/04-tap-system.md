# Tap System

Taps are Homebrew-style registries for distributing Octomind agents, skills, and capabilities.

## Overview

A tap is a Git repository (or local directory) containing:

```
agents/
  category/
    variant.toml         # Agent manifest (tag = category:variant)
deps/
  org/
    tool.sh              # Dependency install script (idempotent)
skills/
  skill-name/
    SKILL.md             # Skill instructions
    scripts/             # Executable scripts
    references/          # Documentation files
    assets/              # Static files
capabilities/
  capability-name/
    config.toml          # REQUIRED: triggers = [...] (drives auto-activation); optional domains = [...]
    default.toml         # Provider wiring (deps / server_refs / allowed_tools / mcp.servers)
    octocode.toml        # Alternate provider, selected via [capabilities] override
```

> Two distinct files are involved when you work with taps: the **on-disk tap repo** (the tree above — `agents/`, `deps/`, `skills/`, `capabilities/`) and **your `config.toml`** (where `[taps]`, `[capabilities]`, and `[registry]` live). Each TOML snippet below notes which file it belongs to.

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

The default tap `muvon/tap` is always present as the **last-priority fallback**. It is auto-cloned on first use, and it cannot be added (`tap muvon/tap` is rejected) or removed (`untap muvon/tap` is rejected).

## Tap Priority

When multiple taps provide the same agent tag, priority is:
1. User-added taps (in order added)
2. Built-in default tap (`muvon/tap`)

When more than one tap provides the same `category:variant`, the **first-listed tap wins** and a debug-level log line is emitted (`'…' found in multiple taps — using first match`). The same first-wins rule applies to skills and capabilities.

## Using Tap Agents

### From the CLI

Run a tap agent with `category:variant` format:

```
octomind run octomind:assistant
octomind run developer:general
```

When you specify a tag containing `:`, Octomind runs the full resolution pipeline:
1. **Fetch** the matching agent manifest from taps (first-wins; cached locally — see [Manifest Caching](#manifest-caching))
2. **Expand capabilities** — any `capabilities = [...]` declared in the manifest are resolved and merged in
3. **Resolve placeholders** — `{{INPUT:KEY}}` (prompt once, cached) and `{{ENV:KEY}}` (env var, with `.env` fallback) are substituted (see [Manifest Placeholders](#manifest-placeholders))
4. **Run dependency scripts** — any `[deps] require = [...]` scripts run before MCP init (see [Dependencies](#dependencies-deps))
5. **Inject the tag** as the role name (`category:variant` becomes the role's `name`)
6. **Merge** the manifest into config and start the session

If a manifest needs a credential it cannot find, this is where you will see an `Enter value for …` prompt (step 3) — not at startup.

### From within a session — the `tap` core tool

Inside a running session you can launch a tap role as a subagent via the `tap` core tool. Same role catalog, but invoked from the LLM mid-conversation rather than a CLI command:

```json
{"action": "discover", "intent": "review a legal contract"}
{"action": "run", "role": "lawyer:sg", "prompt": "What are the notice period rules?"}
{"action": "list"}
{"action": "stop", "session": "tap-lawyer-sg-9b2c1d"}
```

Each `run` returns a run id of the form `tap-<role-with-dashes>-<6hex>` (e.g. `tap-lawyer-sg-9b2c1d` for `lawyer:sg`). Use that id to `stop` a run or to resume it (pass it back as `session` on a subsequent `run`).

`discover` matches your `intent` semantically against each agent's title + description (cosine score must exceed `0.2`, top 5 returned) and requires the local embedding model to be ready, erroring if it is not.

Use `background: true` for long tasks; the reply lands as a user message in the next turn. See [MCP Tools — `tap`](../usage/07-mcp-tools.md#tap----run-specialist-roles-from-taps) for the full schema.

### Model Overrides

Set a preferred model for specific tap agents in your config:

```toml
# In config.toml
[taps]
"developer:general" = "ollama:glm-5"
```

This overrides the model for `octomind run developer:general` while leaving other agents unchanged.
**Priority (highest wins):** CLI `--model` > the agent's manifest/role `model` > root `config.model`. The session model is resolved in `src/session/chat/session/core.rs` as `CLI --model ?? role.model ?? config.model`; a `[taps]` entry for this tag overrides the `config.model` tier (`src/agent/resolver.rs`), so it applies only when neither `--model` nor the agent's manifest role declares its own `model`.

## Agent Manifests

Agent manifests are TOML files in `agents/<category>/<variant>.toml`. The header comment block is part of the schema — `# Title:` and `# Description:` are required and feed `tap discover`'s semantic matching (they are the embedding corpus). They are **not** used by `octomind run` shell autocomplete, which derives `category:variant` purely from the file path:

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

The role's `name` is **always force-injected from the tag** — `category:variant` is written into the first `[[roles]]` entry's `name`, overwriting any value you declare there. The shipped templates set `name` anyway for readability, but it has no effect. Manifests can include any config sections: roles, layers, MCP servers, etc.

If the role needs runtime harness control (`mcp` / `agent` / `skill` / `schedule` / `capability` tools), include `"runtime"` in `server_refs`. Most roles only need `"core"` (`plan` + `tap`) plus the data servers they actually use.

## Skills

Skills are reusable instruction packs. Auto-activation uses declarative `rules:` in SKILL.md frontmatter — see [Skills](../usage/15-skills.md) for full documentation.

Skills are **not tap-only**. They are resolved (first-wins, deduped by name) from, in order: taps, then `<workdir>/.agents/skills/`, then `~/.config/agents/skills/`. So "managed via taps" is only part of the story — local project and per-user skills are loaded too.

A skill may declare `capabilities: [...]` in its frontmatter. Activating the skill auto-loads those capabilities; `skill(action="forget", …)` offloads them (refcount-aware — a capability is only unloaded when no remaining active skill needs it) and triggers compression.

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

## Manifest Placeholders

Tap manifests may embed three placeholder forms, resolved during the [resolution pipeline](#from-the-cli) (INPUT, then ENV, then deps run, before MCP init). Escape any literal you do not want substituted by doubling the braces (`{{{{INPUT:KEY}}}}`).

- **`{{INPUT:KEY}}`** — persistent value store. Prompted from the user **once**, then saved to `~/.local/share/octomind/inputs.toml` and reused on every later run. Use it for credentials/IDs you want to enter a single time. (In a non-interactive subagent, a missing INPUT surfaces as a structured error instead of blocking on stdin.)
- **`{{ENV:KEY}}`** — environment variable. If `KEY` is set (and non-empty) it is used directly; otherwise the user is prompted and the value is appended to `./.env` in the current directory (loaded automatically next run) and set in the current process.
- **`{{CWD}}`** — the runtime current working directory (resolved by the prompt-placeholder layer, e.g. inside a role `system` prompt).

```toml
# In an agent manifest (in the tap repo)
[[roles]]
system = "Project root: {{CWD}}. Use the API at https://api.example.com."

[[roles.mcp.servers]]
name = "example"
# Token prompted once and stored; API base read from env / .env
args = ["--token", "{{INPUT:EXAMPLE_TOKEN}}", "--url", "{{ENV:EXAMPLE_API_URL}}"]
```

## Dependencies (`[deps]`)

Agent manifests (and capability provider files) declare external tool dependencies under `[deps] require`:

```toml
# In an agent manifest or capability provider .toml (in the tap repo)
[deps]
require = ["astral-sh/uv", "muvon/octocode"]
```

Each entry is an `org/tool` string that maps to a script at `<tap_root>/deps/<org>/<tool>.sh` (e.g. `astral-sh/uv` → `deps/astral-sh/uv.sh`). A flat name like `octocode` would look for `deps/octocode.sh` and fail. The scripts:

- Run in order, via `bash`, **on every resolution** of a `category:variant` tag (not just the first run) — they must be **idempotent**: exit `0` immediately if the tool is already installed, non-zero to abort.
- Run **after** `{{INPUT}}`/`{{ENV}}` placeholder resolution and **before** MCP initialization.
- Execution contract: stdin is null, stdout is suppressed (reserved for Octomind), stderr is captured and reported in the error message on failure; exit `0` = ok, non-zero = abort with an error.

## Capabilities

A capability in a tap is **two files**, not one:

```
capabilities/
  codesearch/
    config.toml      # REQUIRED: triggers = [...]; optional domains = [...]
    default.toml     # default provider wiring
    octocode.toml    # alternate provider
```

- **`config.toml`** is mandatory and carries capability-level metadata shared across all providers:
  - `triggers = [...]` — **required and non-empty**. Short phrases a user might write that activate the capability; they drive the deterministic auto-activation (semantic) routing layer. A capability with no `triggers` is rejected at load.
  - `domains = [...]` — optional. **Empty means universal** (available to every role). When non-empty, it hard-gates the capability to roles whose domain part matches (`developer:general` → `"developer"`). The gate applies everywhere — auto-activation, `capability list`/`discover`/`enable`, and `OCTOMIND_CAPABILITIES` — with no bypass.
- **`<provider>.toml`** carries the provider-specific wiring: `[deps]`, `[roles.mcp]` `server_refs`/`allowed_tools`, and `[[mcp.servers]]`.

### Provider Overrides

Override which provider file is used for a capability in your config:

```toml
# In config.toml
[capabilities]
codesearch = "octocode"
```

This selects `capabilities/codesearch/octocode.toml` within the tap, allowing different implementations of the same capability (the default is `default.toml`).

## Storage

Taps are stored under `~/.local/share/octomind/taps/`. A tap named `user/repo` lives at `taps/<user>/octomind-<repo>/` — the first path segment is the username, and every repo directory is prefixed with `octomind-`:

```
taps/
  myorg/
    octomind-my-agents/   # git clone or symlink (tap myorg/my-agents)
  muvon/
    octomind-tap/         # built-in default (muvon/tap)
```

GitHub taps clone from `github.com/<user>/octomind-<repo>` (note the `octomind-` prefix on the repo name) and are `git pull`ed each time taps load. Local taps are live symlinks (edits are picked up immediately). The default `muvon/tap` is auto-cloned on first use.

### Manifest Caching

Fetched agent manifests are cached separately from the tap repos, at:

```
~/.local/share/octomind/agents/<category>/<variant>.toml
```

Cache lifetime is controlled by `[registry] cache_ttl_hours` in your config (default `24`):

```toml
# In config.toml
[registry]
cache_ttl_hours = 24
```

When a cached manifest is **fresh**, it is used directly. When it is **stale-but-present**, the cached copy is returned immediately and refreshed in the background — so an edit to a tap manifest may take one run to appear. When there is no cache, the manifest is fetched synchronously from the taps (first-wins) and written to the cache.

Persisted `{{INPUT:KEY}}` answers live alongside the cache in `~/.local/share/octomind/inputs.toml`.

> A `category:variant@version` tag is accepted (the `@version` segment is parsed) but currently unused — version pinning is not yet enforced.
