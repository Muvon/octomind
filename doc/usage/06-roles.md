# Roles and Permissions

Roles control what the AI can do in a session: which tools are available, what system prompt is used, and how the AI behaves.

## How Roles Work

Every session runs with a role. The role determines:
- **System prompt** -- instructions for the AI
- **MCP server access** -- which tool servers are available
- **Tool permissions** -- which specific tools can be used
- **Model parameters** -- `temperature`, `top_p`, `top_k` (and an optional `model` override)

> **Role vs. tap agent.** A **role** is a plain `[[roles]]` entry in your config, addressed by its bare name (e.g. `assistant`). A **tap agent** is a ready-made manifest published in a tap (a registry of agents), addressed by a `category:variant` **tag** (e.g. `developer:general`). Any tag containing `:` is resolved through the registry, fetching the manifest and merging it on top of your config. See [Tap System](../integration/04-tap-system.md) for details.

## Shipped Config Roles vs. Tap Agents

It helps to know what actually exists out of the box. The default config ships four plain roles, and the default tap (`muvon/tap`, which resolves to the GitHub repo `github.com/muvon/octomind-tap`) provides tap agents addressed by tag:

| Kind | Identifier | What it is |
|------|------------|------------|
| Config role | `assistant` | Full-tool built-in role (core/runtime/filesystem/agent, `*:*`) |
| Config role | `task_refiner` | Lightweight, tool-free query refinement |
| Config role | `task_researcher` | Tool-free research helper |
| Config role | `reduce` | Tool-free summarization/reduction |
| Tap agent | `assistant:concierge` | Default tag in the shipped config (chat-style concierge) |
| Tap agent | `developer:general` | Full development agent from the `developer` tap category |
| Tap agent | `assistant:*` | Chat-oriented variants (e.g. the chat-only `octomind:assistant`) |

```bash
octomind run assistant:concierge   # Tap agent (default tag in shipped config)
octomind run developer:general     # Full development tap agent
octomind run assistant             # Plain config role with full tool access
```

> The bare `assistant` config role is **not** chat-only — it has full tool access. The chat-only variant is the tap agent `octomind:assistant`.

## Defining Custom Roles

Define roles in `[[roles]]` config sections (always `[[roles]]` — never `[role_name]`). If a tap manifest's `[[roles]]` entry has the same `name` as a config role, the **config role wins** and the manifest role is skipped (see [Role Priority](#role-priority)).

```toml
[[roles]]
name = "assistant"
temperature = 0.3
top_p = 0.7
top_k = 20
system = """
You are helpful and knowledgeable assistant.
Working directory: {{CWD}}
"""
welcome = "Hello! Working in {{CWD}} (Role: {{ROLE}})"

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

### Role Fields

> **Every field except `model` is mandatory.** `system`, `welcome`, `temperature`, `top_p`, and `top_k` have no defaults — a `[[roles]]` entry missing any of them fails to parse with a deserialization error. Only `model` is optional.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Role identifier |
| `model` | string | no | Model override (`provider:model` format) — only field that may be omitted |
| `system` | string | yes | System prompt (supports [template variables](../reference/04-environment-variables.md#template-variables)) |
| `welcome` | string | yes | Welcome message on session start |
| `temperature` | f32 | yes | Sampling temperature (0.0-2.0) |
| `top_p` | f32 | yes | Nucleus sampling (0.0-1.0) |
| `top_k` | u32 | yes | Top-k token limit (1-1000) |

**Validation ranges (enforced at config load).** Values outside these bounds abort loading with an error like `Role '<name>' temperature must be between 0.0 and 2.0`:
- `temperature` — `0.0` to `2.0`
- `top_p` — `0.0` to `1.0`
- `top_k` — `1` to `1000`

**Model resolution priority.** When more than one source sets a model, the effective model is chosen in this order (highest first):

```
CLI --model  >  role.model  >  config.model
```

A role's `model` (whether a plain `[[roles]]` entry or a tap agent's manifest role) is honored directly over the root `config.model`; CLI `--model` still wins. For a tap agent (`category:variant`), a `[taps]` override for that tag replaces the `config.model` tier. For the full model-selection story see [Providers](04-providers.md).

> **Multi-step AI workflows** are no longer bound to roles. Use the external `octomind workflow <file.toml>` CLI instead — see [Workflows](09-workflows.md).

## Tool Permissions

### Server References

`server_refs` lists which MCP servers this role can access:

```toml
[roles.mcp]
server_refs = ["core", "filesystem"]  # Only core and filesystem servers
```

> **Empty `server_refs = []` disables MCP for the role entirely** — it gets no tool servers at all. This is how the tool-free roles (`task_refiner`, `task_researcher`, `reduce`) run. Internally `RoleMcpConfig::is_enabled()` returns `false` when `server_refs` is empty.

A `server_refs` entry that names a server not present in the global registry is **silently dropped** (it only produces a debug log: `referenced by role but not found in global registry`). See the note on `filesystem` below.

### Allowed Tools

`allowed_tools` controls which tools within those servers are available:

```toml
[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = [
  "core:*",              # plan, tap
  "runtime:mcp",         # only the mcp tool from runtime (skip agent / skill / schedule / capability)
  "filesystem:view",     # Only view from filesystem
  "filesystem:shell",    # Only shell from filesystem
  "agent:*",             # All agent_<name> sub-agent tools on the agent server
]
```

**Builtin (config-declared) servers** — only these three are declared as `[[mcp.servers]]` in the default config:
- `core` -- high-level day-to-day tools: `plan`, `tap`.
- `runtime` -- low-level harness control: `mcp` (register servers), `agent` (register dynamic agents), `skill` (load skills), `schedule`, `capability`. Most roles don't need this.
- `agent` -- dispatches to `[[agents]]`-defined ACP sub-agents (`agent_<name>` per entry).

> **`filesystem` is not a built-in declared server.** The filesystem tools (`view`, `text_editor`, `batch_edit`, `extract_lines`, `shell`, `ast_grep`, `list_files`, `workdir`) come from the **octofs** companion server supplied by the tap/capability layer, not from a `[[mcp.servers]]` entry. If you copy the examples below into a bare standalone config that has no tap/capability providing `filesystem`, the reference is silently dropped and no filesystem tools appear. To see exactly which tools a server exposes, run `/mcp list` (or `/mcp full`) in a session.

**Pattern syntax:**
- `"server:*"` -- all tools from a server (e.g. `agent:*` grants every `agent_<name>` execution tool on the `agent` server)
- `"server:prefix_*"` -- prefix match within a server (e.g. `filesystem:text_*` matches `text_editor`)
- `"server:tool_name"` -- one specific tool
- `"tool_name"` (no colon) -- backward-compat form: matches that tool name across **all** referenced servers
- Empty array `[]` -- all tools from all referenced servers

> Some tap manifests use a bare-glob form like `agent_*` (no colon) instead of `agent:*`. Both work, via different matching mechanisms; prefer the `server:*` form for clarity and consistency.

### Global Fallback

If a role doesn't specify `allowed_tools`, the global `[mcp].allowed_tools` is used:

```toml
[mcp]
allowed_tools = []  # No global restrictions (default)
```

## Example Roles

> These examples reference `filesystem`, which (as noted above) is provided by the tap/capability layer. They work when a tap or capability supplies the `filesystem` (octofs) server; in a bare standalone config the `filesystem` references are silently dropped.

### Full Developer Access

```toml
[[roles]]
name = "developer"
temperature = 0.3
top_p = 0.7
top_k = 20
system = """
You are an expert software developer.
Working directory: {{CWD}}
Git status: {{GIT_STATUS}}
"""
welcome = "Developer role ready in {{CWD}}"

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

### Read-Only Analyst

```toml
[[roles]]
name = "analyst"
temperature = 0.2
top_p = 0.7
top_k = 20
system = "You analyze code and provide insights. Do not modify files."
welcome = "Analyst role ready (read-only)."

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"]
```

### Documentation Writer

```toml
[[roles]]
name = "docs"
model = "openrouter:openai/gpt-4o"
temperature = 0.4
top_p = 0.7
top_k = 20
system = "You write clear documentation."
welcome = "Docs role ready."

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:text_editor"]
```


## Using Roles

### Starting a Session

```bash
# Use the default tag (config `default` field).
# In the shipped config this is `assistant:concierge` — a TAP AGENT
# resolved via the registry, not a plain [[roles]] entry.
octomind run

# Specify a plain config role by name
octomind run assistant            # full-tool built-in role

# Use a tap agent by category:variant tag
octomind run developer:general    # full development tap agent
octomind run assistant:concierge  # chat concierge tap agent
```

> There is no plain `developer` role in the default config. `developer` exists only as a tap **category** (variants include `general`, `doc`, `spec`, `readme`, …). Use the tag form `developer:general`. Running `octomind run developer` (a bare, unknown role name) does **not** error — see the caution under [Role Priority](#role-priority).

### Switching Roles Mid-Session

```
/role analyst
/role assistant
```

### Role Priority

Two distinct cases — don't conflate them:

1. **Manifest vs. config name collision.** When a tap manifest contains a `[[roles]]` entry whose `name` duplicates a role already defined in your base config, the **base (config) role wins** and the manifest's same-named role is skipped (manifest merge dedups by name, base-wins).
2. **A `category:variant` tag is always a tap agent.** A tag containing `:` (e.g. `developer:general`) is resolved directly through the registry and merged on top of your config — it never contends with a same-prefixed local role. There is no precedence "contest" for tags.

> **Unknown plain role names do not error.** If you pass a bare role name that isn't in your config (e.g. `octomind run developer`), the session does **not** fail — `get_role_config` logs a loud error and falls back to the **first role in the config** (HashMap order, effectively arbitrary) to keep the session alive. Only a `category:variant` tap tag errors, and only if its manifest cannot be fetched. Always spell role names exactly to avoid silently running the wrong role.

## Auto-Bind Servers

MCP servers can auto-attach to specific roles:

```toml
[[mcp.servers]]
name = "octocode"
type = "stdio"
command = "octocode"
args = ["mcp", "--path=."]
auto_bind = ["assistant"]  # Automatically available in the assistant role
```

`auto_bind` matches the role tag by **exact** string (`"developer"` ≠ `"developer:general"`).

The bound server is automatically added to the role's `server_refs` even if not explicitly listed. There is a second, easy-to-miss half:

- If the role uses a **restricted** `allowed_tools` (a non-empty list), auto-bind also appends `"<server>:*"` to `allowed_tools`, so the bound server's tools are actually permitted. Without this, an auto-bound server in a restricted role would have zero usable tools.
- If `allowed_tools` is **empty** (unrestricted), no patch is needed — everything from the bound server is already allowed.
