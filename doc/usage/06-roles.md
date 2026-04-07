# Roles and Permissions

Roles control what the AI can do in a session: which tools are available, what system prompt is used, and how the AI behaves.

## How Roles Work

Every session runs with a role. The role determines:
- **System prompt** -- instructions for the AI
- **MCP server access** -- which tool servers are available
- **Tool permissions** -- which specific tools can be used
- **Model parameters** -- temperature, top_p, top_k
- **Pipeline** -- optional deterministic script pre-processing (runs before workflow)
- **Workflow** -- optional AI-driven pre-processing pipeline

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
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]
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
| `workflow` | string | no | Workflow to activate for this role |
| `pipeline` | string | no | Pipeline to activate for this role (runs before workflow) |

When both `pipeline` and `workflow` are set, the execution order is: **user message → pipeline (scripts) → workflow (AI) → main model**. The pipeline's output replaces the user message as input to the workflow. See [Pipelines](14-pipelines.md) for details.

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
server_refs = ["core", "filesystem", "agent"]
allowed_tools = [
  "core:*",              # All tools from core server
  "filesystem:view",     # Only view from filesystem
  "filesystem:shell",    # Only shell from filesystem
  "agent:*",             # All agent tools
]
```

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
server_refs = ["core", "filesystem", "agent"]
allowed_tools = ["core:*", "filesystem:*", "agent:*"]
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

### Role with Workflow

```toml
[[roles]]
name = "planner"
workflow = "developer_workflow"
system = "You are a planning assistant."

[roles.mcp]
server_refs = ["core", "filesystem"]
allowed_tools = ["core:*", "filesystem:view"]
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
octomind run developer:rust
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
