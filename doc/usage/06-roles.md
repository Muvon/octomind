# Roles and Permissions

Configure roles to select system prompts, model settings, and tool grants. This guide is for users creating local roles
or inspecting the effective permissions of a tap agent.

## How Roles Work

Every session runs with a role. The role determines:

- **System prompt** — instructions for the AI
- **MCP server access** — which tool servers are available
- **Tool permissions** — which specific tools can be used
- **Model profile** — optional `[roles.model]` overrides inherited from `[model]`

> **Role vs. tap agent.** A **role** is a plain `[[roles]]` entry in your config, addressed by its bare name (e.g.
> `assistant`). A **tap agent** is a ready-made manifest published in a tap (a registry of agents), addressed by a
> `category:variant` **tag** (e.g. `developer:general`). Any tag containing `:` is resolved through the registry,
> fetching the manifest and merging it on top of your config. See [Tap System](../integration/04-tap-system.md) for
> details.

## Shipped Config Roles vs. Tap Agents

It helps to know what actually exists out of the box. The default config ships four plain roles, and the default tap
(`muvon/tap`, which resolves to the GitHub repo `github.com/muvon/octomind-tap`) provides tap agents addressed by tag:

| Kind | Identifier | What it is |
|------|------------|------------|
| Config role | `assistant` | References `core`, `orchestration`, `runtime`, `filesystem`, and `agent`; availability depends on registration |
| Config role | `task_refiner` | Query refinement with no explicit server references |
| Config role | `task_researcher` | Research helper explicitly granting `view` when `filesystem` resolves |
| Config role | `reduce` | Summarization/reduction with no explicit server references |
| Tap agent | `assistant:concierge` | Default tag; prompt and tools come from its resolved manifest |
| Tap agent | `developer:general` | Example development tag; prompt and tools come from its resolved manifest |

```bash
octomind run assistant:concierge   # Tap agent (default tag in shipped config)
octomind run developer:general     # Development tap agent
octomind run assistant             # Plain config role
```

Tap variants are not wildcard selectors: pass an exact tag. After startup, inspect the tools actually exposed:

```text
/role
/mcp list
```

## Defining Custom Roles

Add this complete role entry in `roles.toml` alongside the generated `config.toml`. Use `[[roles]]` with a `name`, not a
table named after the role. Choose a unique plain name; see [Role Priority](#role-priority) for collisions.

```toml
[[roles]]
name = "helper"
system = """
You are helpful and knowledgeable assistant.
Working directory: {{CWD}}
"""
welcome = "Hello! Working in {{CWD}} (Role: {{ROLE}})"

[roles.model]
temperature = 0.3
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["core"]
allowed_tools = ["core:recall"]
```

```bash
octomind config --validate
octomind run helper
```

### Role Fields

`name`, `system`, and `welcome` define the role. `[roles.model]` is optional; every field inside it inherits from
`[model]` when omitted.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Role identifier |
| `system` | string | yes | System prompt (supports [template variables](../reference/04-environment-variables.md#template-variables)) |
| `welcome` | string | yes | Welcome message on session start |
| `model` | table | no | Partial model profile; omitted fields inherit from `[model]` |
| `mcp` | table | no | If present, include both `server_refs` and `allowed_tools`; if absent, both start empty |

`[roles.model]` accepts every field from the main `[model]` table: `name`, `reasoning_effort`, `max_tokens`,
`temperature`, `top_p`, `top_k`, `max_retries`, `retry_timeout`, and `request_timeout_seconds`.

**Validation ranges (enforced after inheritance).** Values outside these bounds abort loading with an error naming the
profile:

- `temperature` — `0.0` to `2.0`
- `top_p` — `0.0` to `1.0`
- `top_k` — `0` to `1000` (`0` disables it)

**Model resolution priority.** When more than one source sets a model, the effective model is chosen in this order
(highest first):

```text
runtime override  >  role model profile  >  tap name mapping  >  [model]
```

A role's `[roles.model]` may override any subset of the complete profile. Missing fields inherit through the chain;
omitting the block uses the inherited profile unchanged. For the full model-selection story see
[Providers](04-providers.md).

For multi-step execution outside a session, see [Workflows](09-workflows.md).

## Tool Permissions

### Server References

`server_refs` lists explicit server grants. For example, replace the `helper` role's MCP table with:

```toml
[roles.mcp]
server_refs = ["core"]
allowed_tools = ["core:recall"]
```

Empty `server_refs` removes explicit grants, but exact-match `auto_bind` servers still join the role. Interactive CLI
sessions also receive `schedule` and `monitor` from `orchestration`, even when the role has no server references. That
interactive addition does not itself grant `tap`.

A `server_refs` entry that names a server not present in the global registry is **silently dropped** (it only produces a
debug log: `referenced by role but not found in global registry`). See the note on `filesystem` below.

### Allowed Tools

`allowed_tools` controls tools within explicitly referenced servers. This alternative table uses only servers declared
in the default template:

```toml
[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "agent"]
allowed_tools = [
  "core:*",              # recall, when attention or governance is enabled
  "orchestration:*",     # tap, schedule, monitor
  "runtime:mcp",         # only mcp from runtime (schedule belongs to orchestration)
  "agent:*",             # All agent_<name> sub-agent tools on the agent server
]
```

See [Configuration](03-configuration.md#mcp-servers) for the four builtin servers and their tool groups.

`filesystem` is not declared in the default server registry. Supply it through your external MCP configuration or a tap
capability before using the examples below. Tool names belong to that external server's schema; inspect them before
choosing a filter:

```text
/mcp list
/mcp full
```

**Pattern syntax:**

- `"server:*"` — all tools from a server (e.g. `agent:*` grants every `agent_<name>` execution tool on the `agent`
  server)
- `"server:prefix_*"` — prefix match within a server (e.g. `filesystem:text_*` matches `text_editor`)
- `"server:tool_name"` — one specific tool
- `"tool_name"` (no colon) — backward-compat form: matches that tool name across **all** referenced servers
- Empty array `[]` — no role-level filter; each registered server's `tools` filter still applies

Bare prefixes such as `agent_*` also work; `agent:*` makes the intended server explicit. A nonempty role allowlist with
no matching pattern drops an explicitly referenced server. A role filter replaces that server's `tools` filter, so
`server:*` exposes all its tools even if the registry entry had a narrower filter.

### Effective Permissions

There is no global allowlist fallback during role merging. When you include `[roles.mcp]`, specify both fields:

```toml
[roles.mcp]
server_refs = []
allowed_tools = []
```

Tool grants are not an OS sandbox. Interactive `monitor` can execute commands; runtime tools and automatic capability
activation can add tools. Project executables under `.agents/tools/` are also added independently of role allowlists
when the session has MCP servers. Inspect `/mcp list` after activation or role changes.

To disable automatic capability activation, edit this root key before all table headers:

```toml
auto_capabilities = false
```

This does not remove interactive session tools or project-local tools. See [MCP Tools](07-mcp-tools.md) for the runtime
tool surface.

## Auto-Bind Servers

MCP servers can auto-attach to specific roles. For example, save this as `mcp-core.toml` in the config directory to
replace the existing `core` server definition and bind it to the exact tap tag:

```toml
[[mcp.servers]]
name = "core"
type = "builtin"
timeout_seconds = 30
tools = []
auto_bind = ["developer:general"]
```

`auto_bind` matches the role tag by **exact** string (`"developer"` ≠ `"developer:general"`).

The bound server is automatically added to the role's `server_refs` even if not explicitly listed. There is a second,
easy-to-miss half:

- For newly added server references, if the role uses a **restricted** `allowed_tools` (a non-empty list), auto-bind
  also appends `"<server>:*"` to `allowed_tools`, keeping downstream consumers consistent with the added server.
- If `allowed_tools` is **empty** (unrestricted), no patch is needed — everything from the bound server is already
  allowed.

If an explicit reference was excluded by its role filter, auto-bind can add that server back. The server's own `tools`
filter still applies to the auto-bound definition. Inspect the result:

```bash
octomind run developer:general
```

```text
/mcp list
```

## Example Roles

Add these entries to `roles.toml`. They assume you have registered a `filesystem` server exposing `view` and
`text_editor`; use the [external server setup](03-configuration.md#mcp-servers) with that server name and your installed
executable. They demonstrate server grants, not a complete sandbox.

### Full Developer Access

```toml
[[roles]]
name = "developer"
system = """
You are an expert software developer.
Working directory: {{CWD}}
Git status: {{GIT_STATUS}}
"""
welcome = "Developer role ready in {{CWD}}"

[roles.model]
temperature = 0.3
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["core", "orchestration", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "orchestration:*", "runtime:*", "filesystem:*", "agent:*"]
```

### Analyst with a View Grant

```toml
[[roles]]
name = "analyst"
system = "You analyze code and provide insights. Do not modify files."
welcome = "Analyst role ready."

[roles.model]
temperature = 0.2
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"]
```

### Documentation Writer

```toml
[[roles]]
name = "docs"
system = "You write clear documentation."
welcome = "Docs role ready."

[roles.model]
name = "openai:gpt-5.6-luna"
temperature = 0.4
top_p = 0.7
top_k = 20

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:text_editor"]
```


## Use Custom Roles

After adding the examples and configuring the filesystem server, start each by its local name:

```bash
octomind run developer
octomind run analyst
octomind run docs
```

The `docs` example requires an OpenAI credential. There is no plain `developer` role in the shipped config; the example
above creates it. Use `developer:general` to resolve a tap agent instead.

### Switching Roles Mid-Session

These bare names refer explicitly to local `[[roles]]` entries from the examples above:

```text
/role
/role analyst
/role assistant
```

A successful `/role` switch rebuilds the system prompt, reinitializes MCP, applies the new role's resolved model
profile, and saves the role to the session. It leaves the global config unchanged. The profile application can replace
an earlier `/model` or `/effort` choice; reapply those commands after switching if needed.

### Role Priority

Two distinct cases — don't conflate them:

1. **Manifest vs. config name collision.** When a tap manifest contains a `[[roles]]` entry whose `name` duplicates a
  role already defined in your base config, the **base (config) role wins** and the manifest's same-named role is
  skipped (manifest merge dedups by name, base-wins).
2. **A `category:variant` tag always triggers tap resolution.** The resolver injects the full tag as the manifest's
  first role name, then requires a newly added role. Defining a local role with that exact tag can prevent normal
  resolution when the merge skips it. Use a unique plain name for a local customization; a local `developer`
  does not collide with `developer:general`.

> **Unknown plain role names are rejected.** User-supplied bare names are validated before session setup or `/role`
> switching, while a `category:variant` tag fails if its manifest cannot be resolved. Spell local role names exactly or
> prefer a verified tap tag.

## Troubleshooting

**Why is a tool missing?** Check that its server is registered, referenced or auto-bound, and matched by the allowlist.
Then inspect connection status and schemas:

```text
/mcp health
/mcp full
```

**Why are tools present with empty `server_refs`?** Check auto-bind, the interactive `schedule`/`monitor` addition,
automatic capabilities, and project-local tools. Empty explicit grants do not describe the entire runtime.

**Why does a role edit remove old settings?** Multi-file config merging keeps the last whole same-named role entry.
Repeat its required fields; tap merging instead preserves base entries. Validate after editing:

```bash
octomind config --validate
```

## See also

- [Configuration](03-configuration.md)
- [AI Providers](04-providers.md)
- [Sessions](05-sessions.md)
- [MCP Tools](07-mcp-tools.md)
- [Tap System](../integration/04-tap-system.md)
