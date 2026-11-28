# Tap System: Share and Install Reusable AI Agents

Install and author reusable Octomind agents, skills, capabilities, workflows, and plugins with taps, the shareable agent
package system. This guide covers tap management, manifest configuration, and the cache behavior you need when
developing a tap.

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
octomind tap init myorg/my-agents --agent my-agents
```

Bootstraps a ready-to-use tap from the default tap's scaffold (`scaffolds/tap/` in the locally downloaded built-in tap).
In one command it:

1. Renders the scaffold into `./octomind-my-agents/` (override with `--dir`), substituting `__TOKEN__` placeholders (such as `__AGENT_NAME__`, the starter agent's name) in
  file paths and contents. Rendering fails if any token remains unresolved, and refuses a non-empty destination.

2. Runs the optional command configured by `[post_create] validate` in `scaffold.toml`, using `sh -c`.
3. Initializes a Git repository.
4. Registers the directory as a local tap.

`--agent` names the starter agent and defaults to the repo name. The starter is written to `agents/<name>.toml`, and its
tag is `<name>`. Start it with that name:

```bash
octomind run my-agents
```

To choose the output directory and starter agent together:

```bash
octomind tap init myorg/review-tools --dir ./review-tools --agent review-tools
```

Use a tap ID under your own GitHub account when publishing. Remote installation expects a repository named
`octomind-<repo>`; for `myorg/my-agents`, that is `myorg/octomind-my-agents`.

### Built-in Tap

The default tap `muvon/tap` is always present as the **last-priority fallback**. It is auto-cloned on first use, and it
cannot be added (`tap muvon/tap` is rejected) or removed (`untap muvon/tap` is rejected).

The `[registry]` section of your config names it and the revision to serve:

```toml
# In config.toml
[registry]
tap = "muvon/tap"   # built-in tap, in user/repo form
ref = "<branch|tag|commit>"   # revision of the built-in tap to serve
```

Both keys come from your config; Octomind has no built-in fallback for them. The config upgrade writes them from the template. Taps you add with `octomind tap` follow their default branch.

## Tap Priority

On an uncached manifest lookup, priority is:

1. User-added taps (in order added)
2. Built-in default tap (`muvon/tap`)

When more than one tap provides the same agent name, the **first-listed tap wins** and a debug-level log line is
emitted (`'…' found in multiple taps — using first match`). Skills also use first-wins lookup. Bare capability
references in manifests try the agent's own tap first; pinned references and provider overrides further constrain
capability lookup, as described below.

## Tap Layout

A tap is a Git repository (or local directory) containing:

```text
agents/
  name.toml              # Agent manifest (tag = name)
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

Agent manifests are TOML files at `agents/<name>.toml`, directly under `agents/`: no subdirectories and no symbolic
links. The name is lowercase letters, digits and hyphens. For discovery, `# Title:` and `# Description:`
header comments are required and feed the `tap` tool's agent matching (they are the embedding corpus). They are
**not** used by `octomind run` shell autocomplete, which derives the tag purely from the file name:

```toml
# agents/developer.toml
# Agent: developer
# Title: Developer
# Description: Review source code and explain implementation choices.

[[roles]]
name = "developer"
system = "Review source code and explain implementation choices. Working directory: {{CWD}}."
welcome = ""

[roles.mcp]
server_refs = ["core", "runtime", "orchestration"]
allowed_tools = ["core:*", "runtime:*", "orchestration:*"]
```

The role's `name` is **always force-injected from the tag** — the agent name is written into the first `[[roles]]`
entry's `name`, overwriting any value you declare there. Declare `name` for readability, while treating the filename tag
as authoritative. Manifests can include any config sections: roles, commands, MCP servers, etc.

`octomind run <name>` starts that agent, so `octomind run developer` starts `agents/developer.toml`. A tag written with `:` is an error that shows the flat form. A specialty of an agent is a skill bound to it with `domains: <agent name>`; see [Skills](#skills).

If the role needs tools to manage its configuration (`mcp` / `agent` / `skill` / `capability`), include `"runtime"` in
`server_refs`. Include `"orchestration"` for `tap`, `schedule`, or `monitor`. Planning is done by a plugin, not a tool, and
`core` is only needed for conditional `recall` when compression attention or governance is enabled.

## Skills

Skills are reusable instruction packs. Each specialty procedure of an agent is a skill bound to that agent with
`domains: <agent name>`. A skill loads when the user's message matches its `rules:` in SKILL.md frontmatter, when a
workflow step lists it in `skills = [...]`, when `OCTOMIND_SKILLS=a,b octomind run <name>` preloads it, or when
`/skill use <name>` loads it by hand. See [Skills](../usage/15-skills.md) for full documentation.

Skills are **not tap-only**. They are resolved (first-wins, deduped by name) from, in order: taps, then
`<workdir>/.agents/skills/`, then `~/.agents/skills/`. Agent Plugin skills are searched next, followed by skills
that a plugin such as Besydr generates. Later sources cannot shadow an authored skill already found by name.

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

When an agent calls `tap(action="run", prompt=…)`, the runtime picks the target agent from the agents that caller may reach. The `# Uses:` header in the manifest sets that set: a comma-separated list of agent names, or `*` for every other agent. An agent with the header declares the `orchestration` capability. A caller that is not a tap agent, such as a role from your local config, reaches every installed agent.

```toml
# agents/assistant.toml
# Uses: *
```

In the community tap only `assistant` carries the header. An agent without the header can still start copies of itself with `tap` (see [From within a session](#from-within-a-session--the-tap-orchestration-tool)).

For example, use a stored project label and an environment-supplied service URL in a manifest:

```toml
# agents/service.toml
# Title: Service Reviewer
# Description: Review the service identified by your configured project label and URL.
[[roles]]
name = "service"
system = "Project {{INPUT:PROJECT_LABEL}} at {{ENV:SERVICE_URL}}. Working directory: {{CWD}}."
welcome = ""
```

Run once interactively to answer `PROJECT_LABEL`; an existing `.env` is loaded at CLI startup and overrides process
environment values, so keep its `SERVICE_URL` consistent with your intended value:

```bash
SERVICE_URL=http://127.0.0.1:9000 octomind run service
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

- Run in order, via `bash`, **on every resolution** of an agent tag (not just the first run) — they must be
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
| `domains` | Optional array; empty means universal | Runtime activation requires an exact match with the role domain, such as `developer` for the `developer` agent. |

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

For example, replace the earlier `agents/developer.toml` in your local tap with this manifest. Put capability
references at the top level, before any table header:

```toml
# agents/developer.toml
# Title: Developer
# Description: Review source code after checking that Git is installed.
capabilities = ["myorg/git-tools"]

[[roles]]
name = "developer"
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
OCTOMIND_CAPABILITIES=myorg/git-tools octomind run developer
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
"my-agents" = "octohub:auto"
```

This replaces only the main model name for `octomind run my-agents`; other parameters inherit `[model]` unless
the role overrides them. Explicit runtime fields win, then the manifest role profile, then the tap name mapping, then
`[model]`.

## Using Tap Agents

### From the CLI

Run a tap agent by its name:

```bash
octomind run my-agents
```

This uses the starter agent created above. Discover tags installed on your machine with:

```bash
octomind complete run
```

`octomind run` with no argument starts the config `default`, which is `assistant`. If your config defines a role with the same name as a tap agent, that role takes priority over the tap agent. The same applies to `octomind acp` and the `/role` command. The `role` parameter of the `tap` tool below reaches only tap agents, so a config role never wins there. A tag written with `:` is an error that shows the flat form.

To start an agent in a read-only session, add `--readonly`; see [Read-Only Sessions](../usage/05-sessions.md#read-only-sessions).

When a name resolves to a tap agent, Octomind runs the full resolution pipeline:

1. **Fetch** the matching agent manifest from taps (first-wins; cached locally — see [Manifest
  Caching](#manifest-caching))

2. **Expand capabilities** — any `capabilities = [...]` declared in the manifest are resolved and merged in (see
  [Referencing Capabilities](#referencing-capabilities)). A capability whose wiring references an unset or empty
  `{{ENV:KEY}}` is skipped with its deps and servers, the same gate auto-activation applies, so the agent starts
  without that tool instead of prompting for its key.

3. **Resolve placeholders** — `{{INPUT:KEY}}` (prompt once, cached) and `{{ENV:KEY}}` (environment value, including an
  explicitly empty string, or prompted `.env` fallback) are substituted (see [Manifest
  Placeholders](#manifest-placeholders))

4. **Run dependency scripts** — any `[deps] require = [...]` scripts run before MCP init (see
  [Dependencies](#dependencies-deps))

5. **Inject the tag** as the role name (the agent name becomes the role's `name`)
6. **Merge** the manifest into config and start the session

If a manifest needs a missing value, resolution can prompt on stderr before the session starts. Supply credentials
before automating ACP/tap subprocesses; do not expect their stdin to be available for interactive credential entry.

### From within a session — the `tap` orchestration tool

Inside a running session, the model can hand a task to another agent using the `tap` tool from the `orchestration` builtin server.
The runtime matches the prompt to an agent. The model can also name one with `role`, an agent name. An agent can start copies of itself this way (`role` set to its own name, or no `role` when it has no `# Uses:` header), and `readonly: true` starts a read-only copy.
These are JSON argument objects for separate MCP calls:

```json
{"action": "run", "prompt": "Summarize the project instructions", "workdir": "/tmp"}
```

```json
{"action": "list"}
```

Use the actual `id` returned by `run` in place of the illustrative ID below:

```json
{"action": "stop", "session": "tap-my-agents-9b2c1d"}
```

After a run has stopped or finished, resume it within the same parent session:

```json
{"action": "run", "session": "tap-my-agents-9b2c1d", "prompt": "Summarize your conclusions"}
```

`workdir` defaults to the parent session's directory. Resume reuses the original role and workdir and rejects an
already-running job. Jobs are tracked in the parent session, so `list` is a run list, not the installed role catalog.

To activate capabilities matching an intent in the current session, use the additional `capability` action:

```json
{"action": "capability", "prompt": "Search the codebase for authentication handlers"}
```

Each `run` returns a run id of the form `tap-<role-with-dashes>-<6hex>` (e.g. `tap-my-agents-9b2c1d` for
`my-agents`). Use that id to `stop` a run or to resume it (pass it back as `session` on a subsequent `run`).

`run` ranks the agents the caller may reach by cosine similarity between the prompt and each agent's title and
description. The best one gets the task when it clears a score threshold and a margin over the second best. If nothing matches, `run` starts nothing and returns an
error that lists the closest agents; the same call with `role` set to one of those names starts that agent without
matching. Matching needs the local embedding
model. Runs always execute in the
background; the reply lands as a user message in the next turn. See [MCP Tools —
`tap`](../usage/07-mcp-tools.md#tap-tool-run-agents-from-taps) for the full schema.

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
rm -f "${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}"/agents/developer.*.toml
octomind run developer
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

GitHub taps clone from `github.com/<user>/octomind-<repo>` (note the `octomind-` prefix on the repo name) and are synced
during full tap loading, at most every 15 minutes and at once on the first run of a new octomind version. Hot-path
discovery uses already-local tap data without a sync. Local taps are live symlinks, but agent resolution still uses the
separate manifest cache below. The default `muvon/tap` is auto-cloned on first use.

A GitHub tap's clone is a read-only mirror: each sync resets it to the upstream default branch (the built-in tap: to `[registry] ref`), discarding local edits,
local commits and new files in it. If a sync of the built-in tap fails, Octomind prints one line on stderr (`Could not update tap …; the previous checkout is still in use`) and tries again at the next start. To customise agents, use a local tap or push to the tap's repository.

The sync runs in the background. An agent name that the clone does not have yet waits for the sync that run
started, up to 30 seconds, and is then looked up once more, so an agent published since the last sync resolves on the
first try.

### Manifest Caching

Fetched agent manifests are cached separately from the tap repos, at:

```text
agents/<name>.<source>.toml
```

`<source>` is the tap the manifest came from; for the built-in tap it also holds the `[registry] ref`, with characters other than letters, digits, `-` and `.` written as `_`. A manifest cached from another tap or ref is a different file, so changing `[registry] tap` or `ref` never serves an old one.

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

> A `<name>@version` tag is accepted (the `@version` segment is parsed) but currently unused — version pinning
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
