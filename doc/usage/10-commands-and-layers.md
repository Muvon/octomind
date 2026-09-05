# Commands and Layers

Use this guide to add reusable commands, ACP sub-agents, and prompt templates to your sessions. It also explains the
layer configuration shared by custom commands and programmatic processing.

## Get Started

The shipped `reduce` command compresses conversation history through the local `reduce` role. In an existing session:

```text
/run
/run reduce
```

Add custom definitions to a TOML file in your config directory, then start a new session to load them:

```bash
octomind run assistant
```

See [Configuration reference](../reference/03-config-reference.md) for config locations and loading rules.

## Extension Types

- **Layers** — orchestration stages invoked programmatically (`[[layers]]`).
- **Commands** — the same thing as layers, but triggered interactively with `/run <name>` (`[[commands]]`).
- **Agents** — specialized AI instances exposed as MCP tools (`[[agents]]`, plus runtime dynamic agents).
- **Prompts** — reusable prompt templates queued with `/prompt <name>` (`[[prompts]]`).

All of these are user-defined (or provided by a tap). Octomind does not ship any built-in `[[layers]]`; the default
config ships one command (`reduce`) and one agent (`context_gatherer`).

## Layers

Layers execute via ACP (Agent Client Protocol). Model, system prompt, and MCP tool access live in `[[roles]]` config —
layers reference roles via the `command` field. Layers back the `[[commands]]` slash-command system (`/run <name>`).

### Configuration

The example below is an illustrative custom layer — it is not shipped by default and requires a matching `analysis` role
in `[[roles]]` (see the role example below):

```toml
[[layers]]
name = "analysis"
description = "Performs detailed analysis of code, systems, or requirements"
command = "octomind acp analysis"
input_mode = "last"
output_mode = "append"
output_role = "assistant"
```

`input_mode`, `output_mode`, and `output_role` are **all mandatory** — they have no serde defaults, so omitting any of
them is a TOML parse error. Only `workdir` defaults (to `"."`).

### Input Modes

How the layer receives conversation input:

| Mode | Description |
|------|-------------|
| `"last"` | Explicit input plus the last assistant response; with empty input, the last assistant response or last genuine user task |
| `"all"` | Chronological non-system transcript followed by the current task |
| `"summary"` | Concatenated assistant text truncated to 2000 characters, plus current input and a request to summarize |

### Output Modes

How the layer's output affects the session:

| Mode | Description |
|------|-------------|
| `"none"` | Intermediate processing, doesn't modify session |
| `"append"` | Adds output as a new message to the session |
| `"replace"` | Keeps the first system message, rebuilds welcome/instructions, and inserts all layer outputs |
| `"last"` | Append only the last response to session (ignore multiple outputs) |
| `"restart"` | Clears all messages and inserts only the last layer output |

### Layer Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Layer identifier |
| `description` | string | yes | Human-readable purpose (shown in help) |
| `command` | string | yes | ACP command to execute: `octomind acp <role_name>` |
| `workdir` | string | no | Working directory (the only field with a default: `"."`). Relative paths resolve against the session's working directory. |
| `input_mode` | string | yes | `"last"`, `"all"`, or `"summary"` |
| `output_mode` | string | yes | `"none"`, `"append"`, `"replace"`, `"last"`, `"restart"` |
| `output_role` | string | yes | `"assistant"` or `"user"` — role for output messages. No default; must be set explicitly. |

The `command` field is split on whitespace into an executable and arguments; it is not evaluated by a shell. Do not
embed shell quoting, pipes, or environment assignments. For complex launch logic, point `command` at a wrapper executable that starts the ACP server. Model, system
prompt, and tool access are resolved by the ACP child role.

Example role definition (in config or from taps) that the `analysis` layer above would target:
```toml
[[roles]]
name = "analysis"
system = "Analyze the supplied code or requirements. Identify assumptions, concrete risks, and next steps."
welcome = ""

[roles.model]
name = "openai:gpt-5.6-luna"
temperature = 0.3

[roles.mcp]
server_refs = []
allowed_tools = []
```

## Custom Commands

Commands use the same configuration as layers and are triggered with `/run <name> [input]`. Defining a `[[layers]]`
entry alone does not create a `/run` command. To invoke the `analysis` role above, add:

```toml
[[commands]]
name = "analysis"
description = "Analyze the previous response and current request"
command = "octomind acp analysis"
input_mode = "last"
output_mode = "append"
output_role = "assistant"
```

### Usage

```text
/run
/run analysis Check the assumptions in this design
```

`/run` resolves commands through `Config::get_role_config`; current config merging supplies the global `[[commands]]`
set to roles. The parent uses the current role's resolved command set; the `command` executable selects the child role.
Additional words after the command name become its explicit input. Without them, `/run` supplies the last genuine user
task. Session and request spending checks can decline execution before the child starts.

### Layers vs Commands

`[[layers]]` and `[[commands]]` deserialize into the **same** Rust struct (`LayerConfig`) with the **same** TOML field
set — there are no schema differences between them. The only difference is how they are triggered:

| Feature | Layer | Command |
|---------|-------|---------|
| Triggered by | Code / orchestration | User via `/run` |
| Config section | `[[layers]]` | `[[commands]]` |
| Interactive | No | Yes |
| Typical use | Pipeline stages | User-initiated actions |

## Agents

Agents are specialized AI instances that run as separate processes via ACP (Agent Client Protocol). Each agent becomes
an MCP tool.

### Configuration

```toml
[[agents]]
name = "context_gatherer"
description = "Analyze supplied code or requirements."
command = "octomind acp analysis"
workdir = "."
```

This custom example uses the `analysis` role defined above. The shipped `context_gatherer` agent instead invokes
`octomind acp context_gatherer`, but the shipped local `[[roles]]` list has no role with that name. Define that role or
change the agent command before relying on the shipped entry.

Enable the `agent` server for the parent role. For example, a minimal parent role is:

```toml
[[roles]]
name = "delegator"
system = "Delegate analysis tasks to agent_context_gatherer and explain its findings."
welcome = "Ready to delegate."

[roles.mcp]
server_refs = ["agent"]
allowed_tools = ["agent:*"]
```

```bash
octomind run delegator
```

### How Agents Work

1. Define agent in `[[agents]]` with `name`, `description`, and `command`
2. Agent becomes MCP tool `agent_<name>` (e.g., `agent_context_gatherer`)
3. When called, Octomind spawns the command as a child process
4. Communication happens via JSON-RPC over stdio (ACP protocol)
5. Agent's final response is returned as the tool result

### Agent Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Unique ID. Tool becomes `agent_<name>`. |
| `description` | string | yes | MCP tool description shown to AI |
| `command` | string | yes | Whitespace-split executable and arguments starting an ACP server over stdio |
| `workdir` | string | no | Working directory (default: `"."`) |

### Agent Tool Parameters

| Parameter | Type | Meaning |
|-----------|------|---------|
| `task` | string, required | Task description |
| `async` | boolean, default `false` | Return immediately and inject the result later |

Call `agent_context_gatherer` with supplied text (the sample `analysis` role has no file tools):

```json
{"task": "Analyze this requirement: retries must stop after three failed attempts.", "async": true}
```

### Async Agents

`async: true` returns immediately. The result is injected into the conversation as a user message when complete,
prefixed `[Async agent '<name>' completed]` (or `[Async agent '<name>' failed]` on error).

Use async when:

- Task takes 30+ seconds
- You can continue other work
- You don't need the result immediately

Max concurrent async jobs is fixed at the machine's CPU core count (fallback `4` if it can't be detected); it is not
configurable. Starting a job past that limit does not queue — the call returns an immediate error such as `Async job limit reached (4/4 active). Wait for existing jobs to complete.` All jobs are cancelled on session exit.

### Dynamic Agents

Use the runtime `agent` tool for temporary agents that execute in-process. See [Dynamic agent setup and
invocation](07-mcp-tools.md#agent-tool-dynamic-agent-management) for the complete example and parameter reference.

## Prompt Templates

Reusable prompts sent into the session via `/prompt <name>`. The prompt text is queued into the session inbox and picked
up by the main loop as a normal **user message** on the next turn — so the AI responds to it as a fresh user turn, it is
not silently appended. The template is sent verbatim: prompt-template variable substitution (`{role}`, `{model}`, etc.)
is not currently implemented.

The `description` field is optional; the examples below set it, but it can be omitted.

```toml
[[prompts]]
name = "review"
description = "Request code review with focus on best practices"
prompt = """Please review the code above focusing on:
- Code quality and best practices
- Security considerations
- Performance implications"""

[[prompts]]
name = "explain"
description = "Ask for detailed explanation"
prompt = "Please provide a detailed explanation of the code/concept above."

[[prompts]]
name = "test"
description = "Request test cases"
prompt = """Please help create comprehensive tests:
- Unit test cases
- Edge cases and error conditions
- Integration test considerations"""
```

### Usage

```text
/prompt
/prompt review
```

## Common Questions

**Why is my command missing from `/run`?** Put it in `[[commands]]`, not only `[[layers]]`, and reload by starting a new
session. `/run` lists the loaded command names.

**Why does a layer fail to start?** Confirm its `command` uses an installed executable and a resolvable role. An
`[[agents]]` name or `[[layers]]` name does not define a matching `[[roles]]` entry automatically.

```bash
octomind acp analysis --help
```

This checks the command-line syntax; it does not launch or validate the custom role. Start a session and invoke the
command with a small input to check execution:

```text
/run analysis Summarize the assumptions in the previous answer
```

**Why did replace remove my conversation?** `output_mode = "replace"` keeps the first system message, rebuilds
welcome/instructions, and replaces the old conversation with layer outputs. Use `append` when you need to retain the
conversation.

## Source Reference

- [Shipped definitions](../../config-templates/default.toml)
- [Layer fields and input preparation](../../src/session/layers/layer_trait.rs)
- [Command output modes](../../src/session/chat/command_executor.rs)
- [Agent execution](../../src/mcp/agent/functions.rs)
- [Prompt queueing](../../src/session/chat/session/commands/prompt.rs)

## See also

- [MCP tools](07-mcp-tools.md)
- [Context compression](08-compression.md)
- [Workflows](09-workflows.md)
- [Roles](06-roles.md)
- [Editor integration](12-editor-integration.md)
