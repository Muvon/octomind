# Commands, Layers, Agents, and Prompts

Octomind provides four mechanisms for extending AI capabilities beyond the base session:

- **Layers** — orchestration stages invoked programmatically (`[[layers]]`).
- **Commands** — the same thing as layers, but triggered interactively with `/run <name>` (`[[commands]]`).
- **Agents** — specialized AI instances exposed as MCP tools (`[[agents]]`, plus runtime dynamic agents).
- **Prompts** — reusable prompt templates queued with `/prompt <name>` (`[[prompts]]`).

All of these are user-defined (or provided by a tap). Octomind does not ship any built-in `[[layers]]`; the default config ships one command (`reduce`) and one agent (`context_gatherer`).

## Layers

Layers execute via ACP (Agent Client Protocol). Model, system prompt, and MCP tool access live in `[[roles]]` config — layers reference roles via the `command` field. Layers back the `[[commands]]` slash-command system (`/run <name>`).

### Configuration

The example below is an illustrative custom layer — it is not shipped by default and requires a matching `analysis` role in `[[roles]]` (see the role example below):

```toml
[[layers]]
name = "analysis"
description = "Performs detailed analysis of code, systems, or requirements"
command = "octomind acp analysis"
input_mode = "last"
output_mode = "append"
output_role = "assistant"
```

`input_mode`, `output_mode`, and `output_role` are **all mandatory** — they have no serde defaults, so omitting any of them is a TOML parse error. Only `workdir` defaults (to `"."`).

### Input Modes

How the layer receives conversation input:

| Mode | Description |
|------|-------------|
| `"last"` | The last assistant message from the session (falls back to the last user message if there are no assistant messages) |
| `"all"` | Entire conversation history from the session |
| `"summary"` | A summarized version of the conversation history |

### Output Modes

How the layer's output affects the session:

| Mode | Description |
|------|-------------|
| `"none"` | Intermediate processing, doesn't modify session |
| `"append"` | Adds output as a new message to the session |
| `"replace"` | Replaces entire session content with layer output (reducer functionality) |
| `"last"` | Append only the last response to session (ignore multiple outputs) |
| `"restart"` | Replace session with only the last response (fresh start with last message) |

> **No built-in layers.** The default config (`octomind config`) defines no `[[layers]]` at all — the layer block in the template is commented out as an `analysis` example. It ships one command (`[[commands]]` `reduce`) and one agent (`[[agents]]` `context_gatherer`). Names like `task_refiner`, `task_researcher`, `reduce`, and `assistant` are **roles** (`[[roles]]`), not layers; you reference them from a layer via the `command` field.

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

The mode fields (`input_mode`, `output_mode`, `output_role`) all use custom deserializers with no serde default, so each one must appear in every layer/command/agent. This is why the example values always set `output_role` explicitly.

**Key Architecture**: Layers don't contain model/system/mcp config. Those live in `[[roles]]`. The `command` field references which role to spawn via ACP.

Example role definition (in config or from taps) that the `analysis` layer above would target:
```toml
[[roles]]
name = "analysis"
model = "openrouter:openai/gpt-4.1-mini"
system = "You are a code and systems analyst..."
temperature = 0.3

[roles.mcp]
server_refs = []
allowed_tools = []
```
## Custom Commands

Commands are layers triggered interactively via `/run <name>`. Same configuration as layers.

```toml
[[commands]]
name = "reduce"
description = "Compress session history for cost optimization during ongoing work"
command = "octomind acp reduce"
input_mode = "all"
output_mode = "replace"
output_role = "assistant"
```

### Usage

```
/run              # List available commands
/run reduce       # Execute the reduce command
```

`/run` always lists the global `[[commands]]` set — commands are not role-scoped, so the same list appears regardless of the active role.

### Layers vs Commands

`[[layers]]` and `[[commands]]` deserialize into the **same** Rust struct (`LayerConfig`) with the **same** TOML field set — there are no schema differences between them. The only difference is how they are triggered:

| Feature | Layer | Command |
|---------|-------|---------|
| Triggered by | Code / orchestration | User via `/run` |
| Config section | `[[layers]]` | `[[commands]]` |
| Interactive | No | Yes |
| Typical use | Pipeline stages | User-initiated actions |

## Agents

Agents are specialized AI instances that run as separate processes via ACP (Agent Client Protocol). Each agent becomes an MCP tool.

### Configuration

```toml
[[agents]]
name = "context_gatherer"
description = "Gather detailed context from files and codebase."
command = "octomind acp context_gatherer"
workdir = "."
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
| `command` | string | yes | Shell command starting ACP server over stdio |
| `workdir` | string | no | Working directory (default: `"."`) |

### Agent Tool Parameters

Each agent tool accepts:
- `task` (string, required): Task description in human language
- `async` (boolean, default: false): Run asynchronously

### Async Agents

`async: true` returns immediately. The result is injected into the conversation as a user message when complete, prefixed `[Async agent '<name>' completed]` (or `[Async agent '<name>' failed]` on error).

Use async when:
- Task takes 30+ seconds
- You can continue other work
- You don't need the result immediately

Max concurrent async jobs is fixed at the machine's CPU core count (fallback `4` if it can't be detected); it is not configurable. Starting a job past that limit does not queue — the call returns an immediate `Async job limit reached (N/M active)...` error. All jobs are cancelled on session exit.

### Dynamic Agents

Create agents at runtime using the `agent` MCP tool. Unlike config `[[agents]]` (which spawn an ACP subprocess), dynamic agents execute **in-process** using the session's own `ChatSession` infrastructure:

```json
{"action": "add", "name": "reviewer", "description": "Code reviewer", "system": "You review code..."}
{"action": "enable", "name": "reviewer"}
```

`add` registers an agent but does **not** enable it — call `enable` to make `agent_<name>` available for execution. Actions: `add`, `enable`, `disable`, `remove`, `list`.

The `add` action requires `name`, `system`, and `description`, and accepts these optional fields: `model`, `temperature`, `top_p`, `top_k`, `welcome`, `server_refs`, `allowed_tools`, and `workdir` (default `"."`). Without `server_refs` the agent runs with MCP disabled; if `allowed_tools` is given without `server_refs`, the matching servers are inferred automatically.

See [MCP Tools Reference](07-mcp-tools.md#agent----dynamic-agent-management).

## Prompt Templates

Reusable prompts sent into the session via `/prompt <name>`. The prompt text is queued into the session inbox and picked up by the main loop as a normal **user message** on the next turn — so the AI responds to it as a fresh user turn, it is not silently appended. The template is sent verbatim: prompt-template variable substitution (`{role}`, `{model}`, etc.) is not currently implemented.

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

```
/prompt              # List available prompts
/prompt review       # Queue the review prompt as the next user message
```
