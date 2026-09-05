# Migration Guide

Use this guide when upgrading an existing Octomind installation or porting a hand-written config.
It covers schema `12`, changes to model and tool configuration, and recovery when an upgrade cannot load.

## Back Up and Upgrade

Config loading can upgrade the selected file before any command handler runs, including `config --show` and
`config --validate`. Back up the whole config directory first so sibling files are included.
On Linux/macOS, with the standard config location:

```bash
migration_config_dir="${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/config"
cp -R "$migration_config_dir" "${migration_config_dir}.before-upgrade"
octomind config --upgrade
octomind config --validate
```

Choose an unused backup destination if `.before-upgrade` already exists. The registered migration chain upgrades
older files to schema `12`; a missing version is treated as `0`. It creates a backup before replacing the selected
file atomically and prints the backup path. It only transforms fields owned by its migration steps: an upgraded
version stamp does not prove that a historical config has every currently required field.

Do not edit `version` to force or skip a migration. Already-current files are not repaired by `--upgrade`.

### Config File Location

| Setting | What it selects |
|---|---|
| Default on Linux/macOS | `~/.local/share/octomind/config/config.toml` |
| Default on Windows | Local AppData `octomind\config\config.toml` |
| `OCTOMIND_DATA_DIR` | Data root for config, sessions, credentials, logs, and other persistent state |
| `OCTOMIND_CONFIG_PATH` | Config file to load/auto-upgrade; its parent supplies sibling TOML files |

There is no search of older config locations. To migrate a file already stored elsewhere, set its full path.
For example, if your old file is `~/.config/octomind/config.toml`:

```bash
cp -R "$HOME/.config/octomind" "$HOME/.config/octomind.before-upgrade"
OCTOMIND_CONFIG_PATH="$HOME/.config/octomind/config.toml" octomind config --validate
```

That startup auto-upgrades the selected file before validating the merged configuration. The explicit
`config --upgrade` handler targets `<data-root>/config/config.toml`, even when `OCTOMIND_CONFIG_PATH` selects
another file; use the validation command above for a custom path. The user-scope `.env` remains under
`<data-root>/config/`.

### Splitting a Monolithic Config

The loader merges `config.toml` first, other `*.toml` files alphabetically, and `mcp-*.toml` files alphabetically
last. Tables merge recursively; same-named entries in arrays such as `[[roles]]` and `[[mcp.servers]]` are replaced
by the last entry as a whole. Scalar arrays such as `server_refs` are replaced.

Keep backups outside that directory or with a non-`.toml` suffix. Move complete server entries to a sibling
`mcp-servers.toml`, for example:

```toml
[[mcp.servers]]
name = "runtime"
type = "builtin"
timeout_seconds = 30
tools = []
```

This replaces any earlier `runtime` entry. The versioned upgrade chain runs on the selected config file, not every
sibling file; review overlays for old keys, duplicate roles, and server types too.

## Model Profile and Provider Format

Version `12` moves the old root model string and flat generation settings into `[model]`.
This complete example keeps an explicit provider/model choice:

```toml
[model]
name = "openrouter:anthropic/claude-sonnet-4-6"
reasoning_effort = "medium"
max_tokens = 32768
temperature = 0.3
top_p = 0.7
top_k = 20
max_retries = 1
retry_timeout = 30
request_timeout_seconds = 300
```

All model names require `provider:model` format. There are exactly three request purposes:

| Profile | Purpose and inheritance |
|---|---|
| `[model]` | Required complete main baseline |
| `[roles.model]` | Partial override for that role's main requests; omitted fields inherit main |
| `[supervisor.model]` | Shared supervisor profile, including learning; omitted fields inherit main |
| `[compression.model]` | Compression decisions and summaries; omitted fields inherit main |

The version `12` migration renames `[compression.decision]` to `[compression.model]` and its `model` key to `name`.
Separate learning/gate/plan/condense model settings are removed in favor of `[supervisor.model]`.
The migrated supervisor and compression blocks fill missing parameters from the template; review these explicit
values if you intended them to track main-profile changes.

To share main settings while overriding just these model names, replace the existing owner blocks with:

```toml
[supervisor.model]
name = "octohub:auto"

[compression.model]
name = "octohub:auto"
```

Tap mappings and workflow step `model` fields remain strings, not profile tables. A config tap override looks like:

```toml
[taps]
"developer:general" = "octohub:auto"
```

The shipped main, supervisor, and compression profiles use `octohub:auto`. Migration does not require replacing
an intentional model choice with that default. Authenticate the default with:

```bash
octomind login
```

See [Providers](../usage/04-providers.md) for provider-specific credentials.

### API keys are environment-only

Provider credentials are read from the environment. Remove legacy provider blocks carrying an `api_key` after
moving credentials to your environment. `octomind login` stores `OCTOHUB_API_KEY` in the user-scope `.env`;
`config --api-key` only reports that config-file keys are unsupported. For an alternate provider, substitute your
provider-issued key:

```bash
export OPENROUTER_API_KEY="your-provider-issued-key"
```

Project `.env` values override user-scope `.env` values, which override the process environment. See
[Common Issues](01-common-issues.md#key-not-found) if an old key keeps winning.

## Role Configuration

Replace legacy named-role tables with a complete `[[roles]]` entry. For a local role using the builtin servers:

```toml
[[roles]]
name = "developer"
system = "You are the project developer. Work in {{CWD}}."
welcome = ""

[roles.model]
name = "openrouter:anthropic/claude-sonnet-4-6"

[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "agent:*"]
```

| Legacy setting | Current action |
|---|---|
| Named tables such as `[developer]` | Use `[[roles]]` with `name`, `system`, and `welcome` |
| Role `enabled` | Remove it; defined roles are selectable |
| Role `enable_layers` or `workflow` | Remove it; use the external workflow CLI for multi-step orchestration |
| Flat role model/sampling settings | Put overrides in `[roles.model]` |
| MCP `enabled` | Select servers with `[roles.mcp].server_refs` and grant tools with `allowed_tools` |

Start the local role above with:

```bash
octomind run developer
```

The root `default` is `assistant:concierge` in the template and applies when `run`, `acp`, or `server` receives no tag.
To make your newly defined local role the default, set this before any table header:

```toml
default = "developer"
```

## MCP Configuration

Use explicit server entries instead of legacy `[mcp].enabled` or `providers` lists:

```toml
[mcp]
allowed_tools = []

[[mcp.servers]]
name = "core"
type = "builtin"
timeout_seconds = 30
tools = []
```

Each server has its own type, timeout, and tool list. Select static servers through `server_refs` or an exact
`auto_bind` match. An empty role `allowed_tools` means no allowlist filtering; a non-empty list needs matching grants.

### Runtime Namespace Move

The current builtin surface is split across four servers:

| Server | Current tools |
|---|---|
| `core` | `recall`, advertised when compression attention or its governance is enabled |
| `runtime` | `mcp`, `agent`, `skill`, `capability` |
| `orchestration` | `tap`, `schedule`, `monitor` |
| `agent` | Generated `agent_<name>` execution tools |

`plan` is supervisor-internal and is not an MCP tool. `/plan` displays its state.
Add missing server blocks to a hand-written config; these are already in the template:

```toml
[[mcp.servers]]
name = "runtime"
type = "builtin"
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "orchestration"
type = "builtin"
timeout_seconds = 30
tools = []

[[mcp.servers]]
name = "agent"
type = "builtin"
timeout_seconds = 30
tools = []
```

The local-role example above grants these servers. Remove references and grants for servers you do not need.
The `runtime` server's `agent` management tool and the separate `agent` execution server serve different purposes;
include both when managing and calling agents. Confirm the tools in a new session:

```text
/mcp list
/plan
```

### Filesystem Is External; Process Servers Use `stdio`

`filesystem` is not a builtin. The default config has no server entry for it; tap/capability configuration can
supply an external server. A role's `server_refs` alone does not create that server.

If your config declares `filesystem` as `type = "builtin"`, remove that block when your selected tap supplies the
server. If you own the server configuration and have `octofs` installed, replace it with:

```toml
[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
args = ["mcp"]
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

Use the exact role name in `auto_bind`; for the local role above, change it to `["developer"]`.
The auto-bind matcher does not treat `developer` as a prefix of `developer:general`.
Auto-bound servers also receive a wildcard grant when the role's allowlist is non-empty.

The TOML server type is `"stdio"`, never `"stdin"`. The upgrade chain does not rewrite server types or replace
builtin filesystem declarations. See [Common Issues](01-common-issues.md#tool-not-found) for static role wiring.

## Command and Layer Configuration

Put user-invoked command layers in `[[commands]]`. This is the shipped `reduce` entry; replace a stale entry
rather than appending another with the same name:

```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

`[[layers]]` and `[[commands]]` deserialize to the same `LayerConfig`, but `/run` only looks up `[[commands]]`.
The `command` selects an ACP process and role; model, system prompt, and MCP settings belong to that role.
The template includes the `reduce` role used above.

| Field | Requirement |
|---|---|
| `name`, `description`, `command` | Required strings |
| `input_mode`, `output_mode`, `output_role` | Required; here, all history is replaced with assistant output |
| `workdir` | Optional; defaults to `"."` |

Remove layer-only legacy `builtin`, `enabled`, `enable_tools`, `model`, and `max_tokens` fields. Run the command
inside a session with:

```text
/run reduce
```

### In-Session Workflows Removed

`[[workflows]]` in the main config and role `workflow` fields are not the current workflow schema.
`/workflow` is unsupported by shared session dispatch. Port the workflow to a standalone file using
[Workflows](../usage/09-workflows.md). Once you have saved that file as `workflow.toml` in your project:

```bash
octomind workflow workflow.toml --dry-run
printf '%s\n' 'Review the project and summarize the findings.' | octomind workflow workflow.toml --format jsonl
```

`--dry-run` validates the definition and prints a plan without executing steps. Execution requires non-empty stdin;
`--format jsonl` exposes step results and aggregate cost on stdout. Keep workflow files outside the config directory,
where every `.toml` would otherwise be treated as configuration.

## CLI and Session Commands

`octomind run` takes an optional tag, not a message argument. To port a one-shot invocation, pipe the prompt:

```bash
printf '%s\n' 'Explain this project.' | octomind run --format plain
```

| Surface | Current behavior |
|---|---|
| `/save` | Unsupported; sessions save automatically, including on normal CLI exit |
| `/skill` | Lists skills; a number selects a page, `*` filters, an exact name toggles activation |
| `/plan` | Read-only supervisor plan display |
| `/done` | Forces compression at an explicit task boundary |
| `/help` | Shows the session command list |

For discovery and inspection:

```text
/help
/skill
/skill 2
/skill *review*
/plan
```

To toggle a skill, replace `NAME` with an exact name returned by `/skill`:

```text
/skill NAME
```

See [Session Commands](../reference/02-session-commands.md) for the complete command surface.

## Common Upgrade Questions

### Why Does Validation Fail Before `--upgrade` Runs?

The CLI loads and validates config before dispatching the command. Invalid TOML or an incompatible sibling file can
stop it there. Fix the file named in the error, compare required fields with
[the template](../../config-templates/default.toml), then retry:

```bash
octomind config --validate
```

A file already stamped `12` is not migrated again. Do not lower its version to repair missing fields; restore the
missing fields from the template in their proper tables. In particular, put scalar compression keys before nested
`[compression.attention]` or `[compression.model]` headers.

### Why Did a Role or Server Lose Settings?

A later same-named array entry replaces the whole earlier entry. Inspect all sibling `.toml` files, especially
`mcp-*.toml`, and keep the complete winning role/server entry. Then inspect the loaded configuration and a new session:

```bash
octomind config --show
octomind run
```

```text
/mcp full
```

### How Do I Recover the Previous Config?

Use the directory copy made before upgrading. On Linux/macOS, after stopping sessions that might save configuration,
move the upgraded directory aside and copy the backup into place (choose an unused `.after-upgrade` destination):

```bash
migration_config_dir="${OCTOMIND_DATA_DIR:-$HOME/.local/share/octomind}/config"
mv "$migration_config_dir" "${migration_config_dir}.after-upgrade"
cp -R "${migration_config_dir}.before-upgrade" "$migration_config_dir"
```

For a custom location, use the directory you backed up instead. Starting the current binary against the restored
older config upgrades it again. A config newer than the binary's supported schema is rejected; use a compatible
binary or its matching backup instead of changing the version number.

## Compression and Supervisor Migration Reference

These are transformations implemented by the registered chain; there is no need to apply them by hand to a
supported older primary config:

| Target version | Transformation |
|---|---|
| `3` | Adds `compression.analysis_findings_max_tokens` and attention configuration when missing |
| `4` | Replaces `compression.pressure_levels` with `threshold` |
| `5` | Adds missing supervisor gate and plan settings |
| `6` | Removes obsolete compression hint settings |
| `7` | Removes obsolete supervisor judges, delegate gate, detector knobs, and gate/plan budget settings |
| `8` | Removes `compression.decision.ignore_cost` |
| `9` | Adds `supervisor.condense.adaptive` (template default `false`) |
| `10` | Removes learning `backend`, `store`, and `retrieve`; supervisor learning uses files |
| `11` | Adds `supervisor.learning.evolution.enabled` (template default `false`) |
| `12` | Nests model profiles and removes separate supervisor-subsystem model settings |

Version `4` preserves an existing threshold; otherwise it takes the lowest old pressure-level threshold, or the
template value when no old threshold exists.

The shipped compression trigger is `70000` tokens. To restore the current compression and optional supervisor
settings, update these keys in their existing tables; these are fragments, not a complete config:

```toml
[compression]
threshold = 70000

[supervisor.condense]
adaptive = false

[supervisor.learning.evolution]
enabled = false
```

Supervisor learning records live under `<data-root>/learning/`; external memory MCP tools are not alternate
supervisor learning stores. See [Learning](../usage/13-learning.md) and [Supervisor](../usage/14-supervisor.md)
for their operating behavior.

## See also

- [Common Issues](01-common-issues.md) — credential, MCP, session, and transport troubleshooting
- [Configuration](../usage/03-configuration.md) — config layout and merging
- [Config Reference](../reference/03-config-reference.md) — current fields and defaults
- [MCP Tools](../usage/07-mcp-tools.md) — configuring servers and tool access
- [Workflows](../usage/09-workflows.md) — standalone workflow definitions
