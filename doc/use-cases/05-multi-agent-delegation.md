# Multi-Agent Task Delegation

Use this guide to delegate development tasks to local, tap-provided, or runtime-created specialists. It covers working
role/tool configuration, asynchronous results, and troubleshooting for the coordinating session.

## Choose a delegation mechanism

| Mechanism | Tool/server | Execution | Use it for |
|-----------|-------------|-----------|------------|
| Static `[[agents]]` | `agent_<name>` / `agent` | ACP subprocess; synchronous by default, optional `async: true` | Stable local specialists |
| Tap role | `tap` / `orchestration` | Background ACP run; resumable by returned session ID | Installed specialists found by intent |
| Dynamic agent | `agent` / `runtime`, then `agent_<name>` | In-process; synchronous by default, optional `async: true` | Temporary specialists created during a session |

New specialists receive their task text, not the parent's conversation. Include the goal, known facts, exact paths,
constraints, and expected result in each delegation.

## Configure the filesystem server

The roles below require an installed `octofs` filesystem server, with both `octofs` and `octomind` on the subprocess
PATH. Check availability and authenticate before continuing:

```bash
command -v octofs
command -v octomind
octomind login
```

Add this server and the following roles/agents to a TOML file in Octomind's config directory. On Linux/macOS the default
directory is `~/.local/share/octomind/config`; on Windows it is `%LOCALAPPDATA%/octomind/config`. `OCTOMIND_DATA_DIR`
overrides the data root, with config beneath its `config` subdirectory. Keep one entry per name within a file; update an
existing entry when one is already present.

```toml
[[mcp.servers]]
name = "filesystem"
type = "stdio"
command = "octofs"
args = ["mcp"]
timeout_seconds = 30
tools = []
```

`filesystem` is a configured name, not a builtin. The shipped builtins are `core`, `orchestration`, `runtime`, and
`agent`. A missing server reference does not provision a filesystem server. The `octofs mcp` subprocess defaults to its
current working directory.

## Define specialist roles

A role's `system` and `welcome` remain explicit role behavior. Its `[roles.model]` block is optional: omitted model
fields inherit from the required main `[model]` profile. Set `welcome = ""` for sub-agent roles you never start
interactively.

```toml
# Roles for each agent (in config.toml)

[[roles]]
name = "context_gatherer"
welcome = ""
system = """
You are a codebase researcher. Your job is to:
1. Find all relevant files for the given task
2. Read key interfaces and function signatures
3. Note patterns, conventions, and dependencies
4. Report findings concisely

Use tools to search and read code. Be thorough but focused.
{{CWD}}
"""

[roles.model]
temperature = 0.2

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view", "filesystem:workdir"]

[[roles]]
name = "code_reviewer"
welcome = ""
system = """
You are a senior code reviewer. Analyze code for:
- Security vulnerabilities
- Performance issues
- Design pattern violations
- Error handling gaps

Be specific: file, line, issue, suggestion.
{{CWD}}
"""

[roles.model]
temperature = 0.1

[roles.mcp]
server_refs = ["filesystem"]
allowed_tools = ["filesystem:view"]
```

## Wire agents to roles

```toml
[[agents]]
name = "context_gatherer"
description = "Gathers codebase context: files, interfaces, patterns, dependencies."
command = "octomind acp context_gatherer"
workdir = "."

[[agents]]
name = "code_reviewer"
description = "Reviews code for security, performance, and design issues."
command = "octomind acp code_reviewer"
workdir = "."
```

With the `agent` server granted to the coordinator, each `[[agents]]` entry is exposed to the main session as a tool
named `agent_<name>`. The positional argument in `command` (`octomind acp context_gatherer`) **is the role name** — the
agent inherits its model, system prompt, temperature, and tools from the matching `[[roles]]` entry. The names need not
match, but keeping them identical makes the wiring obvious. `workdir` defaults to `"."` and resolves relative to the
parent session working directory.

Config agents run as ACP subprocesses with their stderr suppressed, so a child-side crash or misconfiguration surfaces
only as an error or empty result returned from the `agent_<name>` call — not as console output.

## Start a coordinator

Define a local coordinator with explicit access to the delegation tools and filesystem server:

```toml
[[roles]]
name = "coordinator"
welcome = "Ready to delegate."
system = "Coordinate focused specialists. Give each a self-contained task, verify their findings, and report results."

[roles.mcp]
server_refs = ["agent", "orchestration", "runtime", "filesystem"]
allowed_tools = ["agent:*", "orchestration:tap", "runtime:agent", "filesystem:*"]
```

Start it and inspect its tools:

```bash
octomind run coordinator
```

```text
/mcp info
/mcp list
```

The shipped default tag remains `assistant:concierge`; this example selects the local role explicitly so the required
tool grants are visible in the configuration.

The main AI can now use these agents as tools (illustrative transcript):

```text
> Refactor the authentication module to support OAuth2

AI thinking: "This is complex. Let me gather context first."

# AI calls agent_context_gatherer(task="Find all auth-related files,
#   interfaces, and patterns in the codebase")
# Agent runs independently, reads files, returns findings

# AI calls agent_code_reviewer(task="Review src/auth/ for security
#   issues that should be addressed during the refactor")
# Agent runs independently, reviews code, returns issues

# Main AI now has:
# - Full context from context_gatherer
# - Security issues from code_reviewer
# - Can produce a comprehensive refactoring plan
```

### Run independent tasks asynchronously

For large tasks, run agents in parallel (illustrative transcript):

```text
> Analyze the entire codebase for the quarterly security audit

AI:
# Dispatches agents concurrently:
agent_context_gatherer(task="Map all external API endpoints", async=true)
agent_code_reviewer(task="Scan for OWASP Top 10 vulnerabilities", async=true)

# While agents work, AI continues with other analysis
# Results appear as inbox messages when agents complete:
# "[Async agent 'context_gatherer' completed]"
# "[Async agent 'code_reviewer' completed]"
```

Use `/status` for a concise view of every active agent alongside MCP jobs and command monitors. `/status agents` expands
the agent view with recent tap-run history, live actions, model usage, and cost where the runtime provides it:

```text
/status
/status agents
```

The per-session async agent job limit uses `available_parallelism`, falling back to 4 if unavailable. Reaching the limit
returns an error; split work into smaller batches. Results enter the inbox, and consecutive system-managed results can
be processed together. Session exit cancels active agent jobs. Async execution provides no filesystem isolation: give
concurrent writers distinct paths or separate working directories.

## Delegate to tap roles

If a tap registry already provides a specialist role for the sub-task, use the `tap` tool from the `orchestration`
builtin server instead of defining your own `[[agents]]`:

Ask the coordinator to call `tap` with these arguments:

```json
{"action": "discover", "intent": "review code for OWASP Top 10 issues"}
```

Use an exact role returned by discovery. If `developer:general` is returned and fits the task, the next `tap` call is:

```json
{"action": "run", "role": "developer:general", "prompt": "Audit src/auth/ for security bugs. Do not edit files. Return file:line evidence and concrete failure scenarios."}
```

`run` starts in the background and returns its ID. On completion, the inbox receives a tap-run result labeled with the
ID and role. It is a system-managed continuation, distinct from the `[Async agent 'NAME' completed]` label used by
`agent_*`. Use these `tap` arguments to inspect, resume, or stop a run; replace `RUN_ID` with the returned ID:

```json
{"action": "list"}
```

```json
{"action": "run", "session": "RUN_ID", "prompt": "Recheck the highest-severity finding and report the evidence."}
```

```json
{"action": "stop", "session": "RUN_ID"}
```

`list` lists runs in the current session, not the installed role catalog. Omitting `session` on a follow-up starts a
fresh specialist. Resuming a still-running tap returns a busy error.

Discovery requires initialized embeddings and returns up to five matches with cosine score above 0.2. A vague intent may
return none. The separate `tap` capability action attempts capability auto-activation and can return an empty activation
list; it does not share discovery's explicit embedding error contract:

```json
{"action": "capability", "prompt": "I need to search the project's source code."}
```

## Create dynamic agents

Create agents on the fly during a session using the `agent` tool from the `runtime` server (`tap` delegation lives on
the separate `orchestration` server):

Call `agent` with the following payloads in order. The filesystem server must already be registered:

```json
{
  "action": "add",
  "name": "test_writer",
  "description": "Writes unit tests for given code",
  "system": "Write focused unit tests for the supplied task. Report changed paths and validation results.",
  "server_refs": ["filesystem"],
  "allowed_tools": ["filesystem:view", "filesystem:text_editor"]
}
```

```json
{"action": "enable", "name": "test_writer"}
```

Now call `agent_test_writer`, whose task arguments use the same schema as static agent tools:

```json
{"task": "Read src/auth/ and add tests for rejected credentials using the existing test conventions. Do not modify production code or run commands. Report the changed paths.", "async": true}
```

Adding alone does not enable an agent. Dynamic definitions live in session memory and are not written to config.
Omitting `server_refs` permits inference from resolvable entries in `allowed_tools`; inference uses the tool map, so it
does not create missing servers. Explicit references make dependencies easier to diagnose.

For a fixed refine/research/execute sequence, see [custom development workflows](03-custom-development-workflow.md).

## Common questions

- **Why is `agent_code_reviewer` missing?** Check `/mcp list`: the coordinator needs the `agent` server and
  a matching tool grant. For a dynamic agent, call `enable` after `add`.
- **Why can the specialist not read files?** Its role needs a real server definition plus matching tool names. Declaring
  `server_refs = ["filesystem"]` alone does not install or start an undefined server.
- **Why does an ACP child return an error without logs?** Child stderr is suppressed by the ACP runner. Start the same
  role interactively to inspect its configuration and credentials:

  ```bash
  octomind run code_reviewer
  ```

- **Why did a read filter allow more tools than expected?** Auto-bound servers can add wildcard grants, and project
  tools in `.agents/tools/` are always appended. Inspect the final tool surface; a role filter is not an OS sandbox.
  `auto_bind` tags are exact matches: `developer` does not match `developer:general`.

## Model override reference

The complete role examples in [specialist roles](#define-specialist-roles) show the required identity and prompt fields.
A role's model block is optional and inherits every omitted field from `[model]`.

When a specialist needs a different main-purpose model, set a concrete override in its existing model block (do not add
a duplicate table). These examples require the named provider credentials and model access. For example:

```toml
# Inside the context_gatherer role
[roles.model]
name = "openai:gpt-5.6-luna"
temperature = 0.2
```

For a separate review specialist:

```toml
# Inside the code_reviewer role
[roles.model]
name = "anthropic:claude-sonnet-4-6"
temperature = 0.1
```

Octomind has exactly three model purposes: main, supervisor, and compression. Agent role overrides belong to the main
purpose; they do not introduce another purpose. The shipped default for all three is `octohub:auto` after `octomind
login`. Omit `name` from `[roles.model]` to inherit the main profile's model name, as the specialist roles above do. The
`filesystem:view` grant exposes the read tool from that server; the dynamic example adds `filesystem:text_editor` for
edits. Verify the complete effective tool surface before relying on a restricted role.

## See also

- [MCP tools](../usage/07-mcp-tools.md)
- [Tap system](../integration/04-tap-system.md)
- [Roles](../usage/06-roles.md)
- [Custom development workflows](03-custom-development-workflow.md)
- [Event-driven webhooks](02-event-driven-agent.md)
