# Roles and Permissions

Roles control what the AI can do in a session: which tools are available, what system prompt is used, and how the AI behaves.

## How Roles Work

Every session runs with a role. The role determines:
- **System prompt** -- instructions for the AI
- **MCP server access** -- which tool servers are available
- **Tool permissions** -- which specific tools can be used
- **Model parameters** -- temperature, top_p, top_k

## Built-in Roles

Octomind's default tap (`muvon/octomind-tap`) provides production-ready roles:

```bash
octomind run octomind:assistant    # Chat-only with tools
octomind run octomind:developer    # Full development tools
```

## Defining Custom Roles

Define roles in `[[roles]]` config sections. Custom roles override tap-provided roles with the same name.

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

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Role identifier |
| `model` | string | no | Model override (`provider:model` format) |
| `system` | string | no | System prompt (supports [template variables](../reference/04-environment-variables.md#template-variables)) |
| `welcome` | string | no | Welcome message on session start |
| `temperature` | f64 | no | Sampling temperature (0.0-2.0) |
| `top_p` | f64 | no | Nucleus sampling (0.0-1.0) |
| `top_k` | u32 | no | Top-k token limit (1-1000) |

> **Multi-step AI workflows** are no longer bound to roles. Use the external `octomind workflow <file.toml>` CLI instead — see [Workflows](09-workflows.md).

## Tool Permissions

### Server References

`server_refs` lists which MCP servers this role can access:

```toml
[roles.mcp]
server_refs = ["core", "filesystem"]  # Only core and filesystem servers
```

### Allowed Tools

`allowed_tools` controls which tools within those servers are available:

```toml
[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = [
  "core:*",              # plan, tap
  "runtime:mcp",         # only the mcp tool from runtime (skip agent / skill)
  "filesystem:view",     # Only view from filesystem
  "filesystem:shell",    # Only shell from filesystem
  "agent:*",             # All agent_* sub-agent tools
]
```

**Builtin servers:**
- `core` -- high-level day-to-day tools: `plan`, `tap`.
- `runtime` -- low-level harness control: `mcp` (register servers), `agent` (register dynamic agents), `skill` (load skills), `schedule`, `capability`. Most roles don't need this.
- `agent` -- dispatches to `[[agents]]`-defined ACP sub-agents (`agent_<name>` per entry).

**Pattern syntax:**
- `"server:*"` -- all tools from a server
- `"server:tool_name"` -- specific tool
- Empty array `[]` -- all tools from all referenced servers

### Global Fallback

If a role doesn't specify `allowed_tools`, the global `[mcp].allowed_tools` is used:

```toml
[mcp]
allowed_tools = []  # No global restrictions (default)
```

## Example Roles

### Full Developer Access

```toml
[[roles]]
name = "developer"
temperature = 0.3
system = """
You are an expert software developer.
Working directory: {{CWD}}
Git status: {{GIT_STATUS}}
"""

[roles.mcp]
server_refs = ["core", "runtime", "filesystem", "agent"]
allowed_tools = ["core:*", "runtime:*", "filesystem:*", "agent:*"]
```

### Read-Only Analyst

```toml
[[roles]]
name = "analyst"
temperature = 0.2
system = "You analyze code and provide insights. Do not modify files."

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
system = "You write clear documentation."

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:text_editor"]
```


## Using Roles

### Starting a Session

```bash
# Use default role (from config `default` field)
octomind run

# Specify role by name
octomind run developer
octomind run assistant

# Use tap agent
octomind run octomind:developer
octomind run developer:general
```

### Switching Roles Mid-Session

```
/role analyst
/role developer
```

### Role Priority

When a role name matches both a config-defined role and a tap agent:
1. Config-defined `[[roles]]` take priority
2. Tap agents are used as fallback

## Auto-Bind Servers

MCP servers can auto-attach to specific roles:

```toml
[[mcp.servers]]
name = "octocode"
type = "stdio"
command = "octocode"
args = ["mcp", "--path=."]
auto_bind = ["developer"]  # Automatically available in developer role
```

The server is automatically added to the role's `server_refs` even if not explicitly listed.
