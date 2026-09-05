# Tap System

Use taps to install and author reusable Octomind agents, skills, capabilities, workflows, and plugins. This guide covers
tap management, manifest configuration, and the cache behavior you need when developing a tap.

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
octomind tap myorg/my-agents ./octomind-my-agents
```

The local directory must already exist. Use `./` or an absolute path so the symlink target resolves correctly.

### Remove a Tap

```bash
octomind untap myorg/my-agents
```

Removing a local tap deletes its registration and symlink, preserving the source directory. Removing a GitHub tap leaves
its clone on disk. Neither operation clears the separate agent manifest cache.

### Create a New Tap

```bash
octomind tap init myorg/my-agents --agent my-agents:assistant
```

Bootstraps a ready-to-use tap from the default tap's scaffold (`scaffolds/tap/` in the locally downloaded built-in tap).
In one command it:

1. Renders the scaffold into `./octomind-my-agents/` (override with `--dir`), substituting `__TOKEN__` placeholders in
  file paths and contents. Rendering fails if any token remains unresolved, and refuses a non-empty destination.

2. Runs the optional command configured by `[post_create] validate` in `scaffold.toml`, using `sh -c`.
3. Initializes a Git repository.
4. Registers the directory as a local tap.

The starter domain defaults to the repo name; its spec comes from `[defaults] agent_spec` in the downloaded scaffold.
The explicit `--agent` above makes the resulting tag predictable:

```bash
octomind run my-agents:assistant
```

To choose the output directory and starter tag together:

```bash
octomind tap init myorg/review-tools --dir ./review-tools --agent review-tools:reviewer
```

Use a tap ID under your own GitHub account when publishing. Remote installation expects a repository named
`octomind-<repo>`; for `myorg/my-agents`, that is `myorg/octomind-my-agents`.

### Built-in Tap

The default tap `muvon/tap` is always present as the **last-priority fallback**. It is auto-cloned on first use, and it
cannot be added (`tap muvon/tap` is rejected) or removed (`untap muvon/tap` is rejected).

## Tap Priority

On an uncached manifest lookup, priority is:

1. User-added taps (in order added)
2. Built-in default tap (`muvon/tap`)

When more than one tap provides the same `category:variant`, the **first-listed tap wins** and a debug-level log line is
emitted (`'…' found in multiple taps — using first match`). Skills also use first-wins lookup. Bare capability
references in manifests try the agent's own tap first; pinned references and provider overrides further constrain
capability lookup, as described below.

## Tap Layout

A tap is a Git repository (or local directory) containing:

```text
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
    local.toml           # Alternate provider, selected via [capabilities] override
workflows/
  review.toml            # Public external workflow definition
plugins/
  plugin-name/           # Agent Plugin package discovered from the tap
```

> Two distinct files are involved when you work with taps: the **on-disk tap repo** (the tree above — `agents/`,
> `deps/`, `skills/`, `capabilities/`, `workflows/`, and `plugins/`) and **your `config.toml`** (where `[taps]`,
> `[capabilities]`, and `[registry]` live). Each TOML snippet below notes which file it belongs to.

## Agent Manifests

Agent manifests are TOML files in `agents/<category>/<variant>.toml`. For discovery, `# Title:` and `# Description:`
header comments are required and feed `tap discover`'s semantic matching (they are the embedding corpus). They are
**not** used by `octomind run` shell autocomplete, which derives `category:variant` purely from the file path:

```toml
# agents/developer/general.toml
# Agent: developer:general
# Title: General Developer
# Description: Review source code and explain implementation choices.

[[roles]]
name = "developer:general"
system = "Review source code and explain implementation choices. Working directory: {{CWD}}."
welcome = ""

[roles.model]
temperature = 0.3

[roles.mcp]
server_refs = ["core", "runtime", "orchestration"]
allowed_tools = ["core:*", "runtime:*", "orchestration:*"]
```

The role's `name` is **always force-injected from the tag** — `category:variant` is written into the first `[[roles]]`
entry's `name`, overwriting any value you declare there. Declare `name` for readability, while treating the filename tag
as authoritative. Manifests can include any config sections: roles, layers, MCP servers, etc.

If the role needs tools to manage its configuration (`mcp` / `agent` / `skill` / `capability`), include `"runtime"` in
`server_refs`. Include `"orchestration"` for `tap`, `schedule`, or `monitor`. Planning is supervisor-internal, and
`core` is only needed for conditional `recall` when compression attention or governance is enabled.

## Skills

Skills are reusable instruction packs. Auto-activation uses declarative `rules:` in SKILL.md frontmatter — see
[Skills](../usage/15-skills.md) for full documentation.

Skills are **not tap-only**. They are resolved (first-wins, deduped by name) from, in order: taps, then
`<workdir>/.agents/skills/`, then `~/.config/agents/skills/`. Plugin skills are searched next, followed by active
generated skills from learning evolution. Later sources cannot shadow an authored skill already found by name.

A skill may declare `capabilities: [...]` in its frontmatter. Activation loads their servers. Forgetting the skill
releases its server references and offloads servers no remaining skill owns; old injected content is cleaned up at the
next compression.

### Skill Structure

```text
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

MCP `skill` calls use these JSON arguments. First list skills, then use an exact installed name (here, `code-review`):

```json
{"action":"list"}
```

```json
{"action":"use","name":"code-review"}
```

```json
{"action":"forget","name":"code-review"}
```

For authoring instructions and complete `SKILL.md` examples, see [Skills](../usage/15-skills.md).

When activated, the skill instructions and a resource catalog of scripts, references, and assets are injected into
context; resource paths are absolute.

## Manifest Placeholders

INPUT and ENV placeholders resolve during the [resolution pipeline](#from-the-cli), before dependency scripts and MCP
initialization. CWD resolves later when rendering prompts. Escape any literal you do not want substituted by doubling
the braces (`{{{{INPUT:KEY}}}}`).

- **`{{INPUT:KEY}}`** — persistent value store. Prompted from the user **once**, then saved to `inputs.toml` under the
  data root and reused on every later run. Use it for credentials/IDs you want to enter a single time. (The in-process
  non-interactive resolver scope fails on missing values, but the CLI ACP subprocess path does not enter that scope.
  Populate inputs before running unattended.)

- **`{{ENV:KEY}}`** — environment variable. If `KEY` is set, even to an empty string, it is used directly; otherwise the
  user is prompted and the value is appended to `./.env` in the current directory (loaded automatically next run) and
  set in the current process.

- **`{{CWD}}`** — the runtime current working directory (resolved by the prompt-placeholder layer, e.g. inside a role
  `system` prompt).

For example, use a stored project label and an environment-supplied service URL in a manifest:

```toml
# agents/developer/service.toml
# Title: Service Reviewer
# Description: Review the service identified by your configured project label and URL.
[[roles]]
name = "developer:service"
system = "Project {{INPUT:PROJECT_LABEL}} at {{ENV:SERVICE_URL}}. Working directory: {{CWD}}."
welcome = ""
```

Run once interactively to answer `PROJECT_LABEL`; an existing `.env` is loaded at CLI startup and overrides process
environment values, so keep its `SERVICE_URL` consistent with your intended value:

```bash
SERVICE_URL=http://127.0.0.1:9000 octomind run developer:service
```

## Dependencies (`[deps]`)

Agent manifests (and capability provider files) declare external tool dependencies under `[deps] require`:

```toml
# In an agent manifest or capability provider .toml (in the tap repo)
[deps]
require = ["local/git"]
```

Each entry is an `org/tool` string that maps to a script at `<tap_root>/deps/<org>/<tool>.sh` (e.g. `local/git` →
`deps/local/git.sh`). Flat names are not rejected syntactically; they resolve directly to `deps/<name>.sh` and work only
if that file exists. The scripts:

- Run in order, via `bash`, **on every resolution** of a `category:variant` tag (not just the first run) — they must be
  **idempotent**: exit `0` immediately if the tool is already installed, non-zero to abort.

- Run **after** `{{INPUT}}`/`{{ENV}}` placeholder resolution and **before** MCP initialization.
- Execution contract: stdin is null, stdout is suppressed (reserved for Octomind), stderr is captured and reported in
  the error message on failure; exit `0` = ok, non-zero = abort with an error.

A dependency check script at `deps/local/git.sh` can fail clearly when its prerequisite is absent:

```bash
#!/bin/bash
if command -v git >/dev/null 2>&1; then
  exit 0
fi
printf '%s\n' 'Install Git and rerun the agent.' >&2
exit 1
```

## Capabilities

A runtime-discoverable capability needs metadata plus at least one provider file:

```text
capabilities/
  git-tools/
    config.toml      # REQUIRED: triggers = [...]; optional domains = [...]
    default.toml     # default provider wiring
    local.toml       # alternate provider
```

`config.toml` carries metadata shared by all provider files:

| Key | Requirement | Behavior |
|-----|-------------|----------|
| `triggers` | Non-empty array | Phrases used for semantic auto-activation; runtime loading rejects missing/empty triggers. |
| `domains` | Optional array; empty means universal | Runtime activation requires an exact match with the role domain, such as `developer` for `developer:general`. |

The domain gate applies to runtime auto-activation, `capability list`/`discover`/`enable`, and
`OCTOMIND_CAPABILITIES`. Static `capabilities = [...]` expansion in an agent manifest reads provider wiring directly
and does not enforce this metadata gate. Each provider file carries `[deps]`, `[roles.mcp]` references and tool
filters, and optional `[[mcp.servers]]` definitions.

For `capabilities/git-tools/config.toml`:

```toml
triggers = ["inspect git history", "check repository changes"]
domains = ["developer"]
```

Save the capability files under the `octomind-my-agents` tap created earlier, and keep the `deps/local/git.sh`
script from the dependency example. These metadata keys are parsed by the registry, not as top-level Octomind config.
Create the default provider:

```toml
# capabilities/git-tools/default.toml
[deps]
require = ["local/git"]
```

This is a dependency-only capability: activating it checks that Git is installed and exposes no extra MCP tools.
A provider can also bind an existing configured server for static manifest expansion:

```toml
# capabilities/runtime-control/default.toml
[roles.mcp]
server_refs = ["runtime"]
allowed_tools = ["runtime:*"]
```

`[roles.mcp]` here is a provider fragment consumed by manifest expansion, not a role declaration. In an agent manifest,
keep `[[roles]]` with an explicit `name`. Runtime capability enabling needs concrete `[[mcp.servers]]` wiring or
dependencies to install; a fragment containing only references is for static manifest expansion.

### Referencing Capabilities

Capability references — `capabilities = [...]` in an agent manifest, `capabilities: [...]` in skill frontmatter,
`capability(action="enable", name="git-tools")`, and `OCTOMIND_CAPABILITIES` — accept the same three forms:

- `git-tools` — searched across taps in order, first hit wins (an agent manifest tries its own tap first).
- `octomind/git-tools` — looks only in the built-in baseline tap; fails if it does not ship that capability.
- `myorg/git-tools` — pinned to the first connected tap whose owner is `myorg`.

Pinning matters when a third-party tap ships a capability under the same name as a baseline one: a bare reference
resolves to whichever tap is listed first. A pinned capability's `[deps]` scripts run against its own tap, not the
referencing agent's. Provider overrides (below) and the active-capability registry are keyed by the bare name, so
`myorg/git-tools` and `git-tools` are the same capability once loaded.

For example, replace the earlier `agents/developer/general.toml` in your local tap with this manifest. Put capability
references at the top level, before any table header:

```toml
# agents/developer/general.toml
# Title: General Developer
# Description: Review source code after checking that Git is installed.
capabilities = ["myorg/git-tools"]

[[roles]]
name = "developer:general"
system = "Review source code and explain your findings."
welcome = ""
```

For runtime activation, list the available capabilities first and choose an exact name:

```json
{"action":"list"}
```

```json
{"action":"enable","name":"myorg/git-tools"}
```

To request it at startup for a developer-domain role:

```bash
OCTOMIND_CAPABILITIES=myorg/git-tools octomind run developer:general
```

### Provider Overrides

To choose an alternate provider, create its file first. This example uses the same dependency check:

```toml
# capabilities/git-tools/local.toml
[deps]
require = ["local/git"]
```

Then select that provider in `config/config.toml`:

```toml
# In config.toml
[capabilities]
git-tools = "local"
```

This selects `capabilities/git-tools/local.toml` within the tap. Without the override, the filename is `default.toml`.

## Model Overrides

Set a preferred model for specific tap agents in your config:

```toml
# In config.toml
[taps]
"my-agents:assistant" = "octohub:auto"
```

This replaces only the main model name for `octomind run my-agents:assistant`; other parameters inherit `[model]` unless
the role overrides them. Explicit runtime fields win, then the manifest role profile, then the tap name mapping, then
`[model]`.

## Using Tap Agents

### From the CLI

Run a tap agent with `category:variant` format:

```bash
octomind run my-agents:assistant
```

This uses the starter agent created above. Discover tags installed on your machine with:

```bash
octomind complete run
```

When you specify a tag containing `:`, Octomind runs the full resolution pipeline:

1. **Fetch** the matching agent manifest from taps (first-wins; cached locally — see [Manifest
  Caching](#manifest-caching))

2. **Expand capabilities** — any `capabilities = [...]` declared in the manifest are resolved and merged in (see
  [Referencing Capabilities](#referencing-capabilities))

3. **Resolve placeholders** — `{{INPUT:KEY}}` (prompt once, cached) and `{{ENV:KEY}}` (environment value, including an
  explicitly empty string, or prompted `.env` fallback) are substituted (see [Manifest
  Placeholders](#manifest-placeholders))

4. **Run dependency scripts** — any `[deps] require = [...]` scripts run before MCP init (see
  [Dependencies](#dependencies-deps))

5. **Inject the tag** as the role name (`category:variant` becomes the role's `name`)
6. **Merge** the manifest into config and start the session

If a manifest needs a missing value, resolution can prompt on stderr before the session starts. Supply credentials
before automating ACP/tap subprocesses; do not expect their stdin to be available for interactive credential entry.

### From within a session — the `tap` orchestration tool

Inside a running session, the model can launch a tap role using the `tap` tool from the `orchestration` builtin server.
These are JSON argument objects for separate MCP calls:

```json
{"action": "discover", "intent": "review source code"}
```

```json
{"action": "run", "role": "my-agents:assistant", "prompt": "Summarize the project instructions", "workdir": "/tmp"}
```

```json
{"action": "list"}
```

Use the actual `id` returned by `run` in place of the illustrative ID below:

```json
{"action": "stop", "session": "tap-my-agents-assistant-9b2c1d"}
```

After a run has stopped or finished, resume it within the same parent session:

```json
{"action": "run", "session": "tap-my-agents-assistant-9b2c1d", "prompt": "Summarize your conclusions"}
```

`workdir` defaults to the parent session's directory. Resume reuses the original role and workdir and rejects an
already-running job. Jobs are tracked in the parent session, so `list` is a run list, not the installed role catalog.

To activate capabilities matching an intent in the current session, use the additional `capability` action:

```json
{"action": "capability", "prompt": "Search the codebase for authentication handlers"}
```

Each `run` returns a run id of the form `tap-<role-with-dashes>-<6hex>` (e.g. `tap-my-agents-assistant-9b2c1d` for
`my-agents:assistant`). Use that id to `stop` a run or to resume it (pass it back as `session` on a subsequent `run`).

`discover` matches your `intent` semantically against each agent's title + description (cosine score must exceed `0.2`,
top 5 returned) and requires the local embedding model to be ready, erroring if it is not. Runs always execute in the
background; the reply lands as a user message in the next turn. See [MCP Tools —
`tap`](../usage/07-mcp-tools.md#tap-tool-run-specialist-roles-from-taps) for the full schema.

## Common questions

| Symptom | What to check |
|---------|---------------|
| GitHub clone fails | Tap `user/repo` maps to `user/octomind-repo`; check that repository and your Git credentials. |
| Local edits do not appear | Symlinks are live, but a fresh manifest cache wins. Check the tag's cached file. |
| An untapped agent still resolves | Removing a tap does not delete cached manifests. |
| Discovery fails on metadata | Add non-empty `# Title:` and `# Description:` header comments before the first TOML table. |
| Background run waits before initialization | Pre-populate INPUT values and ENV values interactively before using ACP. |
| A capability is unavailable | Check the selected provider file, exact name/prefix, and runtime domain gate. |
| A model override seems ignored | A role's explicit model name and runtime override take priority over `[taps]`. |

For short-lived cache experiments, use this in `config/config.toml`:

```toml
[registry]
cache_ttl_hours = 0
```

An existing file is still returned before background refresh. To force a synchronous read for one locally edited
manifest, remove only its cached copy:

```bash
# macOS/Linux default; set OCTOMIND_DATA_DIR here if you use another data root.
rm -f "${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/agents/developer/general.toml"
octomind run developer:general
```

## Storage

All paths below are relative to the data root: `OCTOMIND_DATA_DIR` when set, otherwise `~/.local/share/octomind` on
macOS/Linux or `%LOCALAPPDATA%/octomind` on Windows. Tap registrations are stored in `taps.toml`; `[taps]` in
`config/config.toml` holds model overrides, not the registry source list. Taps live in `taps/`. A tap named `user/repo`
lives at `taps/<user>/octomind-<repo>/` — the first path segment is the username, and every repo directory is prefixed
with `octomind-`:

```text
taps/
  myorg/
    octomind-my-agents/   # git clone or symlink (tap myorg/my-agents)
  muvon/
    octomind-tap/         # built-in default (muvon/tap)
```

GitHub taps clone from `github.com/<user>/octomind-<repo>` (note the `octomind-` prefix on the repo name) and are pulled
during full tap loading. Hot-path discovery uses already-local tap data without a pull. Local taps are live symlinks,
but agent resolution still uses the separate manifest cache below. The default `muvon/tap` is auto-cloned on first use.

### Manifest Caching

Fetched agent manifests are cached separately from the tap repos, at:

```text
agents/<category>/<variant>.toml
```

Cache lifetime is controlled by `[registry] cache_ttl_hours` in your config (default `24`):

```toml
# In config.toml
[registry]
cache_ttl_hours = 24
```

When a cached manifest is **fresh**, it is used directly. When it is **stale-but-present**, the cached copy is returned
immediately and refreshed in the background — so an edit can remain hidden until expiry and a successful background
refresh. Changing tap priority or removing a tap does not invalidate this tag-keyed cache. When there is no cache, the
manifest is fetched synchronously from the taps (first-wins) and written to the cache.

Persisted `{{INPUT:KEY}}` answers live alongside the cache in `inputs.toml` under the data root.

> A `category:variant@version` tag is accepted (the `@version` segment is parsed) but currently unused — version pinning
> is not yet enforced.

## See also

- [Tap management](../../src/agent/taps.rs), [scaffolding](../../src/agent/tap_scaffold.rs), and [manifest
  resolution](../../src/agent/resolver.rs).

- [Registry and capability parsing](../../src/agent/registry.rs), [input substitution](../../src/agent/inputs.rs),
  [dependency scripts](../../src/agent/deps.rs), and [tap tool](../../src/mcp/orchestration/tap.rs).

- [MCP Tools](../usage/07-mcp-tools.md) — tap, runtime, and agent tool arguments.
- [Skills](../usage/15-skills.md) — skill authoring and activation rules.
- [Workflows](../usage/09-workflows.md) — tap workflow files and CLI execution.
- [Configuration Reference](../reference/03-config-reference.md) — role and model settings.
- [ACP Protocol](02-acp-protocol.md) — subprocess transport used by tap runs.
